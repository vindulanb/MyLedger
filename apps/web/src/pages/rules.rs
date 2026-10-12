//! The Rules & Tags screen.
//!
//! Three sections, in the order they matter: categorisation rules, tag
//! management, and SMS parsing rules.
//!
//! The SMS section is an **authoring surface only**. This app has no access to
//! SMS and applies these rules to nothing — the Android app does that, and
//! these rules sync down to it. The reason they are written here is that a
//! regular expression is miserable to type on a phone. The "Test rule" box
//! runs `core::textrules`, the same engine the phone will use and the same one
//! that reads uploaded documents, so what the box shows is what will actually
//! happen rather than a separate approximation of it.

use dioxus::prelude::*;
use myledger_core::textrules::{Context, MatchOutcome, check_rule, match_text};
use myledger_core::{
    NamedInput, PreferencesRequest, Rule, RuleInput, SmsKindHint, SmsParseRule, StateResponse,
    signed_money,
};
use uuid::Uuid;

use crate::api;
use crate::components::EmptyState;
use crate::store::{Load, Store, today};

#[component]
pub fn RulesAndTags() -> Element {
    let store = Store::use_store();
    let ledger = store.ledger.read();

    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Rules & Tags" }
                p { class: "pending-blurb",
                    if matches!(*store.load.read(), Load::Loading) {
                        "Loading your ledger…"
                    } else {
                        "No ledger loaded yet."
                    }
                }
            }
        };
    };

    rsx! {
        section { class: "stack",
            CategoryRules { state: state.clone() }
            TagManager { state: state.clone() }
            SmsRules { state: state.clone() }
        }
    }
}

// ---- categorisation rules --------------------------------------------------

/// `When a merchant contains … then file it under …`
#[component]
fn CategoryRules(state: StateResponse) -> Element {
    let store = Store::use_store();
    let mut creating = use_signal(|| false);
    let mut editing = use_signal(|| None::<Uuid>);
    let busy = *store.saving.read();

    let rules = &state.rules;

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Categorisation rules" }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    onclick: move |_| {
                        let open = creating();
                        creating.set(!open);
                        editing.set(None);
                    },
                    if creating() { "Cancel" } else { "Create rule" }
                }
            }
            p { class: "muted small",
                "A rule files a transaction automatically when its merchant contains some text. Rules run on new entries and imports only, and only after duplicate detection — so they never change what counts as the same purchase, and never rewrite transactions already in your ledger."
            }

            if creating() {
                RuleForm {
                    state: state.clone(),
                    existing: None,
                    on_done: move |_| creating.set(false),
                }
            }

            if rules.is_empty() {
                EmptyState {
                    title: "No rules yet",
                    blurb: "None are provided — a rule has to match the way your own statements name things.",
                }
            } else {
                ul { class: "minilist",
                    for rule in rules.iter() {
                        li { key: "{rule.id}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "rulewhen",
                                        span { class: "rulekeyword", "When" }
                                        " a merchant contains "
                                        code { class: "ruletext", "{rule.when_text}" }
                                    }
                                    span { class: "rulewhen",
                                        span { class: "rulekeyword", "then" }
                                        " {then_text(&state, rule)}"
                                    }
                                }
                                div { class: "rowactions",
                                    label { class: "checkline small",
                                        input {
                                            r#type: "checkbox",
                                            checked: rule.enabled,
                                            disabled: busy,
                                            onchange: {
                                                let state = state.clone();
                                                let id = rule.id;
                                                move |event: FormEvent| {
                                                    let on = event.checked();
                                                    // A sync: resend every
                                                    // rule, with this one's
                                                    // flag changed.
                                                    let wanted = rules_with(&state, id, |input| {
                                                        input.enabled = on;
                                                    });
                                                    spawn(async move {
                                                        let request = PreferencesRequest {
                                                            rules: Some(wanted),
                                                            ..Default::default()
                                                        };
                                                        store
                                                            .save_preferences(
                                                                request,
                                                                if on { "Rule enabled." } else { "Rule disabled." },
                                                            )
                                                            .await;
                                                    });
                                                }
                                            },
                                        }
                                        span { "Enabled" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = rule.id;
                                            move |_| {
                                                if editing() == Some(id) {
                                                    editing.set(None);
                                                } else {
                                                    editing.set(Some(id));
                                                    creating.set(false);
                                                }
                                            }
                                        },
                                        if editing() == Some(rule.id) { "Close" } else { "Edit" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let state = state.clone();
                                            let id = rule.id;
                                            move |_| {
                                                let remaining = surviving_rules(&state, Some(id));
                                                spawn(async move {
                                                    let request = PreferencesRequest {
                                                        rules: Some(remaining),
                                                        ..Default::default()
                                                    };
                                                    store
                                                        .save_preferences(request, "Rule deleted.")
                                                        .await;
                                                });
                                            }
                                        },
                                        "Delete"
                                    }
                                }
                            }

                            if editing() == Some(rule.id) {
                                RuleForm {
                                    state: state.clone(),
                                    existing: Some(rule.clone()),
                                    on_done: move |_| editing.set(None),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Creating or editing a categorisation rule.
#[component]
fn RuleForm(state: StateResponse, existing: Option<Rule>, on_done: EventHandler<()>) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let editing_id = existing.as_ref().map(|r| r.id);

    let mut when_text =
        use_signal(|| existing.as_ref().map(|r| r.when_text.clone()).unwrap_or_default());
    let mut category = use_signal(|| existing.as_ref().and_then(|r| r.then_category_id));
    let mut tag = use_signal(|| existing.as_ref().and_then(|r| r.then_tag_id));
    let mut error = use_signal(|| None::<String>);

    let submit = {
        let state = state.clone();
        move |_| {
            let text = when_text().trim().to_string();
            if text.is_empty() {
                error.set(Some("Say what the merchant has to contain.".to_string()));
                return;
            }
            // A rule that does nothing is worse than no rule: it looks like
            // it is working.
            if category().is_none() && tag().is_none() {
                error.set(Some(
                    "Choose a category, a tag, or both — otherwise the rule would match and then do nothing."
                        .to_string(),
                ));
                return;
            }
            error.set(None);

            let mut wanted = surviving_rules(&state, editing_id);
            wanted.push(RuleInput {
                id: editing_id,
                when_text: text,
                then_category_id: category(),
                then_tag_id: tag(),
                enabled: existing.as_ref().map(|r| r.enabled).unwrap_or(true),
            });

            let done = if editing_id.is_some() { "Rule saved." } else { "Rule created." };
            spawn(async move {
                let request = PreferencesRequest { rules: Some(wanted), ..Default::default() };
                if store.save_preferences(request, done).await {
                    on_done.call(());
                }
            });
        }
    };

    rsx! {
        div { class: "ruleform",
            div { class: "formgrid",
                label { class: "field field-wide",
                    span { class: "field-label", "When the merchant contains" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "rule-when",
                        placeholder: "e.g. grocer",
                        value: "{when_text}",
                        disabled: busy,
                        oninput: move |event| when_text.set(event.value()),
                    }
                    span { class: "field-hint", "Matched anywhere in the name, ignoring case." }
                }

                label { class: "field",
                    span { class: "field-label", "Then set the category to" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| category.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", selected: category().is_none(), "Leave it alone" }
                        for item in state.live_categories() {
                            option {
                                key: "{item.id}",
                                value: "{item.id}",
                                selected: category() == Some(item.id),
                                "{item.name}"
                            }
                        }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "And add the tag" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| tag.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", selected: tag().is_none(), "No tag" }
                        for item in state.live_tags() {
                            option {
                                key: "{item.id}",
                                value: "{item.id}",
                                selected: tag() == Some(item.id),
                                "{item.name}"
                            }
                        }
                    }
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            div { class: "formactions",
                button {
                    class: "button button-quiet",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_done.call(()),
                    "Cancel"
                }
                button {
                    class: "button button-primary",
                    r#type: "button",
                    disabled: busy,
                    onclick: submit,
                    if editing_id.is_some() { "Save changes" } else { "Create rule" }
                }
            }
        }
    }
}

/// The "then …" half of a rule, in words.
fn then_text(state: &StateResponse, rule: &Rule) -> String {
    let category = rule.then_category_id.and_then(|id| state.category_name(id));
    let tag = rule.then_tag_id.and_then(|id| state.tag_name(id));

    match (category, tag) {
        (Some(category), Some(tag)) => {
            format!("file it under {category} and tag it {tag}")
        }
        (Some(category), None) => format!("file it under {category}"),
        (None, Some(tag)) => format!("tag it {tag}"),
        // The form refuses this, but a rule written before that check — or by
        // the API — should read as what it is rather than as a blank.
        (None, None) => "do nothing".to_string(),
    }
}

/// Every rule being kept, as the sync endpoint wants them.
fn surviving_rules(state: &StateResponse, without: Option<Uuid>) -> Vec<RuleInput> {
    state
        .rules
        .iter()
        .filter(|rule| Some(rule.id) != without)
        .map(|rule| RuleInput {
            id: Some(rule.id),
            when_text: rule.when_text.clone(),
            then_category_id: rule.then_category_id,
            then_tag_id: rule.then_tag_id,
            enabled: rule.enabled,
        })
        .collect()
}

/// Every rule, with one of them changed.
fn rules_with(
    state: &StateResponse,
    id: Uuid,
    change: impl Fn(&mut RuleInput),
) -> Vec<RuleInput> {
    let mut all = surviving_rules(state, None);
    if let Some(found) = all.iter_mut().find(|input| input.id == Some(id)) {
        change(found);
    }
    all
}

// ---- tags ------------------------------------------------------------------

/// All tags, their usage, and what deleting one does.
#[component]
fn TagManager(state: StateResponse) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let mut typed = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // Which tag is being deleted, if the confirmation is open.
    let mut confirming = use_signal(|| None::<Uuid>);

    let live: Vec<_> = state.live_tags().cloned().collect();

    let mut create = {
        let state = state.clone();
        move |_| {
            let name = typed().trim().to_string();
            if name.is_empty() {
                return;
            }
            // Case-insensitive, because "Work" and "work" as two tags is
            // never what anyone wanted.
            if state.live_tags().any(|tag| tag.name.eq_ignore_ascii_case(&name)) {
                error.set(Some(format!("You already have a tag called {name:?}.")));
                return;
            }
            error.set(None);

            let mut wanted = surviving_tags(&state, None);
            wanted.push(NamedInput::new_named(&name));
            spawn(async move {
                let request = PreferencesRequest { tags: Some(wanted), ..Default::default() };
                if store.save_preferences(request, "Tag created.").await {
                    typed.set(String::new());
                }
            });
        }
    };

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Tags" }
                if !live.is_empty() {
                    span { class: "muted small",
                        "{live.len()} "
                        if live.len() == 1 { "tag" } else { "tags" }
                    }
                }
            }

            form {
                class: "tagpicker-new",
                onsubmit: move |event| {
                    event.prevent_default();
                    create(());
                },
                label { class: "field field-grow",
                    span { class: "field-label", "New tag" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "tag-name",
                        placeholder: "A name, nothing else",
                        value: "{typed}",
                        disabled: busy,
                        oninput: move |event| typed.set(event.value()),
                    }
                }
                button {
                    class: "button button-primary button-small",
                    r#type: "submit",
                    disabled: busy || typed().trim().is_empty(),
                    "Create tag"
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            if live.is_empty() {
                EmptyState {
                    title: "No tags yet",
                    blurb: "A tag is a free-form label you can put on any number of transactions — handy for a trip, a project, or anything a category doesn't capture.",
                }
            } else {
                ul { class: "minilist",
                    for tag in live.iter() {
                        li { key: "{tag.id}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "minirow-merchant", "{tag.name}" }
                                    span { class: "minirow-meta",
                                        {usage_words(state.tag_use_count(tag.id))}
                                    }
                                }
                                div { class: "rowactions",
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = tag.id;
                                            move |_| {
                                                if confirming() == Some(id) {
                                                    confirming.set(None);
                                                } else {
                                                    confirming.set(Some(id));
                                                }
                                            }
                                        },
                                        if confirming() == Some(tag.id) { "Keep it" } else { "Delete" }
                                    }
                                }
                            }

                            if confirming() == Some(tag.id) {
                                DeleteTagConfirm {
                                    state: state.clone(),
                                    tag_id: tag.id,
                                    tag_name: tag.name.clone(),
                                    uses: state.tag_use_count(tag.id),
                                    on_done: move |_| confirming.set(None),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Deleting a tag, and the choice about history.
///
/// The spec asks before stripping a removed tag from past transactions, and
/// the default is to keep them: an old entry that keeps its label keeps its
/// meaning, and the destructive option cannot be undone.
#[component]
fn DeleteTagConfirm(
    state: StateResponse,
    tag_id: Uuid,
    tag_name: String,
    uses: i64,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let mut strip = use_signal(|| false);

    let remove = move |_| {
        let wanted = surviving_tags(&state, Some(tag_id));
        let also_strip = strip();
        let done = if also_strip {
            "Tag deleted and taken off your history."
        } else {
            "Tag deleted. Past transactions keep it."
        };
        spawn(async move {
            let request = PreferencesRequest {
                tags: Some(wanted),
                strip_removed_tags: Some(also_strip),
                ..Default::default()
            };
            if store.save_preferences(request, done).await {
                on_done.call(());
            }
        });
    };

    rsx! {
        div { class: "confirmbox", role: "group", "aria-label": "Delete the tag {tag_name}",
            p { class: "confirmbox-title", "Delete “{tag_name}”?" }
            p { class: "muted small",
                if uses == 0 {
                    "Nothing is tagged with it, so nothing else changes."
                } else if uses == 1 {
                    "It stops being offered. One transaction already carries it."
                } else {
                    "It stops being offered. {uses} transactions already carry it."
                }
            }

            if uses > 0 {
                label { class: "checkline",
                    input {
                        r#type: "checkbox",
                        checked: strip(),
                        disabled: busy,
                        onchange: move |event| strip.set(event.checked()),
                    }
                    span {
                        "Also take it off those "
                        if uses == 1 { "transaction" } else { "{uses} transactions" }
                    }
                }
                if strip() {
                    p { class: "formerror", role: "alert",
                        "This cannot be undone. The tag will be gone from your history as well as from the list."
                    }
                } else {
                    p { class: "muted small",
                        "Left unticked, past transactions keep showing the tag — which is why they still make sense when you read them back."
                    }
                }
            }

            div { class: "formactions",
                button {
                    class: "button button-quiet",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_done.call(()),
                    "Keep it"
                }
                button {
                    class: "button button-primary",
                    r#type: "button",
                    disabled: busy,
                    onclick: remove,
                    "Delete tag"
                }
            }
        }
    }
}

fn usage_words(uses: i64) -> String {
    match uses {
        0 => "Not used yet".to_string(),
        1 => "On 1 transaction".to_string(),
        many => format!("On {many} transactions"),
    }
}

/// Every tag being kept, as the sync endpoint wants them.
fn surviving_tags(state: &StateResponse, without: Option<Uuid>) -> Vec<NamedInput> {
    state
        .live_tags()
        .filter(|tag| Some(tag.id) != without)
        .map(|tag| NamedInput::existing(tag.id, &tag.name))
        .collect()
}

// ---- SMS parsing rules -----------------------------------------------------

/// The SMS authoring surface. No SMS is read here; see the module note.
#[component]
fn SmsRules(state: StateResponse) -> Element {
    let store = Store::use_store();
    let mut creating = use_signal(|| false);
    let mut editing = use_signal(|| None::<Uuid>);
    let mut testing = use_signal(|| None::<Uuid>);
    let busy = *store.saving.read();

    let rules = &state.sms_parse_rules;

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "SMS parsing rules" }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    onclick: move |_| {
                        let open = creating();
                        creating.set(!open);
                        editing.set(None);
                    },
                    if creating() { "Cancel" } else { "Add a rule" }
                }
            }

            div { class: "notice",
                p {
                    strong { "These rules are written here and used on your phone." }
                }
                p { class: "muted small",
                    "MyLedger on the web has no access to SMS and never reads a message — it only keeps the rule definitions, because a regular expression is far easier to write on a proper keyboard. The Android app downloads them and does the matching, using the same engine the Test box below runs."
                }
            }

            if creating() {
                SmsRuleForm {
                    state: state.clone(),
                    existing: None,
                    on_done: move |_| creating.set(false),
                }
            }

            if rules.is_empty() {
                EmptyState {
                    title: "No SMS parsing rules",
                    blurb: "None are provided — a rule has to match the way your own bank writes its alerts. Add one, paste a real message into the Test box, and adjust until it reads correctly.",
                }
            } else {
                ul { class: "minilist",
                    for rule in rules.iter() {
                        li { key: "{rule.id}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "minirow-merchant", "{rule.label}" }
                                    span { class: "minirow-meta", "{sms_summary(rule)}" }
                                    code { class: "rulepattern", "{rule.body_regex}" }
                                }
                                div { class: "rowactions",
                                    label { class: "checkline small",
                                        input {
                                            r#type: "checkbox",
                                            checked: rule.enabled,
                                            disabled: busy,
                                            onchange: {
                                                let id = rule.id;
                                                move |event: FormEvent| {
                                                    let on = event.checked();
                                                    spawn(async move {
                                                        store
                                                            .act(
                                                                api::toggle_sms_rule(id, on),
                                                                if on { "Rule enabled." } else { "Rule disabled." },
                                                            )
                                                            .await;
                                                    });
                                                }
                                            },
                                        }
                                        span { "Enabled" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = rule.id;
                                            move |_| {
                                                if testing() == Some(id) {
                                                    testing.set(None);
                                                } else {
                                                    testing.set(Some(id));
                                                    editing.set(None);
                                                }
                                            }
                                        },
                                        if testing() == Some(rule.id) { "Close test" } else { "Test rule" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = rule.id;
                                            move |_| {
                                                if editing() == Some(id) {
                                                    editing.set(None);
                                                } else {
                                                    editing.set(Some(id));
                                                    testing.set(None);
                                                }
                                            }
                                        },
                                        if editing() == Some(rule.id) { "Close" } else { "Edit" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = rule.id;
                                            move |_| {
                                                spawn(async move {
                                                    store
                                                        .act(api::delete_sms_rule(id), "Rule deleted.")
                                                        .await;
                                                });
                                            }
                                        },
                                        "Delete"
                                    }
                                }
                            }

                            if editing() == Some(rule.id) {
                                SmsRuleForm {
                                    state: state.clone(),
                                    existing: Some(rule.clone()),
                                    on_done: move |_| editing.set(None),
                                }
                            }

                            if testing() == Some(rule.id) {
                                TestRule { state: state.clone(), rule: rule.clone() }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Pasting a sample message and seeing what a rule would make of it.
///
/// Matched here in the browser with `core::textrules` — the engine the phone
/// will use — so this is a preview of real behaviour rather than a second
/// implementation that could disagree with it.
#[component]
fn TestRule(state: StateResponse, rule: SmsParseRule) -> Element {
    let mut sender = use_signal(|| {
        rule.sender_pattern.clone().unwrap_or_else(|| "MYBANK".to_string())
    });
    let mut body = use_signal(String::new);

    let view = rule.as_view();
    let problem = check_rule(&view).err();

    let outcome = if body().trim().is_empty() {
        None
    } else {
        let today = today();
        let context = Context { source: &sender(), fallback_date: &today };
        // One rule, deliberately: this box answers "what does *this* rule
        // make of this message", not "which of my rules wins".
        Some(match_text(&[view], &body(), &context))
    };

    rsx! {
        div { class: "testbox",
            h4 { class: "review-title", "Test this rule" }

            if let Some(problem) = &problem {
                p { class: "formerror", role: "alert",
                    "This rule cannot match anything: {problem}"
                }
            }

            div { class: "formgrid",
                label { class: "field",
                    span { class: "field-label", "From" }
                    input {
                        class: "field-input mono",
                        r#type: "text",
                        name: "test-sender",
                        value: "{sender}",
                        oninput: move |event| sender.set(event.value()),
                    }
                    span { class: "field-hint", "Who the message came from." }
                }
                label { class: "field field-wide",
                    span { class: "field-label", "Message" }
                    textarea {
                        class: "field-input mono",
                        name: "test-body",
                        rows: "3",
                        placeholder: "Paste a real message from your bank",
                        value: "{body}",
                        oninput: move |event| body.set(event.value()),
                    }
                }
            }

            match &outcome {
                None => rsx! {
                    p { class: "muted small", "Paste a message to see what this rule reads from it." }
                },
                Some(MatchOutcome::NoMatch) => rsx! {
                    div { class: "importasks",
                        p { class: "importasks-title", "This rule would ignore that message" }
                        p { class: "muted",
                            "Either the sender does not match, the pattern does not match, or the amount could not be read from it."
                        }
                    }
                },
                Some(MatchOutcome::Ambiguous(_)) => rsx! {
                    // Can't happen with one rule, but the type says it could.
                    p { class: "muted small", "More than one reading of that message." }
                },
                Some(MatchOutcome::Matched(candidate)) => {
                    let is_income = candidate.kind == myledger_core::TransactionKind::Income;
                    let category = candidate.category_id.and_then(|id| state.category_name(id));
                    rsx! {
                        div { class: "importok", "This message would be read as:" }
                        dl { class: "factgrid factgrid-tight",
                            div { class: "fact",
                                dt { class: "fact-label", "Amount" }
                                dd { class: "fact-value small-value",
                                    "{signed_money(candidate.amount_cents, is_income)}"
                                }
                            }
                            div { class: "fact",
                                dt { class: "fact-label", "Merchant" }
                                dd { class: "fact-value small-value", "{candidate.merchant}" }
                            }
                            div { class: "fact",
                                dt { class: "fact-label", "Date" }
                                dd { class: "fact-value small-value", "{candidate.date}" }
                            }
                            div { class: "fact",
                                dt { class: "fact-label", "Direction" }
                                dd { class: "fact-value small-value",
                                    if is_income { "Money in" } else { "Money out" }
                                }
                            }
                        }
                        p { class: "muted small",
                            if !candidate.date_was_captured {
                                "The rule captured no date, so the message's own arrival date would be used — today's is shown. "
                            }
                            if let Some(name) = category {
                                "It would be filed under {name}. "
                            }
                            if let Some(account) = &candidate.account_hint {
                                "Account: {account}."
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A one-line description of what an SMS rule does.
fn sms_summary(rule: &SmsParseRule) -> String {
    let mut parts = Vec::new();
    parts.push(match rule.kind_hint {
        Some(SmsKindHint::Credit) => "money in".to_string(),
        Some(SmsKindHint::Auto) => "direction from the message".to_string(),
        _ => "money out".to_string(),
    });
    if let Some(pattern) = &rule.sender_pattern {
        parts.push(format!("from senders matching {pattern}"));
    } else {
        parts.push("any sender".to_string());
    }
    if let Some(group) = rule.amount_group {
        parts.push(format!("amount from group {group}"));
    }
    if !rule.enabled {
        parts.push("disabled".to_string());
    }
    parts.join(" · ")
}

/// Creating or editing an SMS parsing rule.
#[component]
fn SmsRuleForm(
    state: StateResponse,
    existing: Option<SmsParseRule>,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let editing_id = existing.as_ref().map(|r| r.id);

    let mut label = use_signal(|| existing.as_ref().map(|r| r.label.clone()).unwrap_or_default());
    let mut pattern =
        use_signal(|| existing.as_ref().map(|r| r.body_regex.clone()).unwrap_or_default());
    let mut from = use_signal(|| {
        existing.as_ref().and_then(|r| r.sender_pattern.clone()).unwrap_or_default()
    });
    let mut amount_group = use_signal(|| {
        existing.as_ref().and_then(|r| r.amount_group).map(|g| g.to_string()).unwrap_or_else(|| "1".to_string())
    });
    let mut merchant_group = use_signal(|| {
        existing
            .as_ref()
            .and_then(|r| r.merchant_group)
            .map(|g| g.to_string())
            .unwrap_or_default()
    });
    let mut date_group = use_signal(|| {
        existing.as_ref().and_then(|r| r.date_group).map(|g| g.to_string()).unwrap_or_default()
    });
    let mut kind = use_signal(|| {
        existing
            .as_ref()
            .and_then(|r| r.kind_hint)
            .map(|hint| hint.as_str().to_string())
            .unwrap_or_else(|| "debit".to_string())
    });
    let mut account = use_signal(|| {
        existing.as_ref().and_then(|r| r.account_hint.clone()).unwrap_or_default()
    });
    let mut category = use_signal(|| existing.as_ref().and_then(|r| r.default_category_id));
    let mut error = use_signal(|| None::<String>);

    let submit = move |_| {
        if label().trim().is_empty() {
            error.set(Some("Give the rule a name.".to_string()));
            return;
        }
        if pattern().trim().is_empty() {
            error.set(Some("A rule needs a pattern to match.".to_string()));
            return;
        }
        let Ok(amount) = amount_group().trim().parse::<i64>() else {
            error.set(Some("Say which capture group holds the amount.".to_string()));
            return;
        };
        error.set(None);

        let input = api::SmsRuleInput {
            label: label().trim().to_string(),
            sender_pattern: blank(from()),
            body_regex: pattern().trim().to_string(),
            amount_group: Some(amount),
            merchant_group: merchant_group().trim().parse().ok(),
            date_group: date_group().trim().parse().ok(),
            kind_hint: Some(kind()),
            account_hint: blank(account()),
            default_category_id: category(),
            enabled: existing.as_ref().map(|r| r.enabled).unwrap_or(true),
        };

        spawn(async move {
            let saved = match editing_id {
                Some(id) => store
                    .act(api::update_sms_rule(id, &input), "Rule saved.")
                    .await
                    .is_some(),
                None => store
                    .act(api::create_sms_rule(&input), "Rule saved.")
                    .await
                    .is_some(),
            };
            if saved {
                on_done.call(());
            }
        });
    };

    rsx! {
        div { class: "ruleform",
            div { class: "formgrid",
                label { class: "field field-wide",
                    span { class: "field-label", "Name" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "sms-label",
                        placeholder: "My bank's card alerts",
                        value: "{label}",
                        disabled: busy,
                        oninput: move |event| label.set(event.value()),
                    }
                }

                label { class: "field field-wide",
                    span { class: "field-label", "Message pattern" }
                    input {
                        class: "field-input mono",
                        r#type: "text",
                        name: "sms-pattern",
                        placeholder: r"spent ([0-9.]+) at (.+) on",
                        value: "{pattern}",
                        disabled: busy,
                        oninput: move |event| pattern.set(event.value()),
                    }
                    span { class: "field-hint",
                        "A regular expression. Each bracketed group can be used below."
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Amount is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        name: "sms-amount-group",
                        min: "1",
                        value: "{amount_group}",
                        disabled: busy,
                        oninput: move |event| amount_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Merchant is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        name: "sms-merchant-group",
                        min: "1",
                        placeholder: "optional",
                        value: "{merchant_group}",
                        disabled: busy,
                        oninput: move |event| merchant_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Date is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        name: "sms-date-group",
                        min: "1",
                        placeholder: "optional",
                        value: "{date_group}",
                        disabled: busy,
                        oninput: move |event| date_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Direction" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| kind.set(event.value()),
                        option { value: "debit", selected: kind() == "debit", "Money out" }
                        option { value: "credit", selected: kind() == "credit", "Money in" }
                        option {
                            value: "auto",
                            selected: kind() == "auto",
                            "Read it from the message"
                        }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Only from senders matching" }
                    input {
                        class: "field-input mono",
                        r#type: "text",
                        name: "sms-sender",
                        placeholder: "optional, e.g. MYBANK",
                        value: "{from}",
                        disabled: busy,
                        oninput: move |event| from.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Account name" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "sms-account",
                        placeholder: "optional",
                        value: "{account}",
                        disabled: busy,
                        oninput: move |event| account.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Category" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| category.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", "No category" }
                        for item in state.live_categories() {
                            option {
                                key: "{item.id}",
                                value: "{item.id}",
                                selected: category() == Some(item.id),
                                "{item.name}"
                            }
                        }
                    }
                }
            }

            details { class: "skipped",
                summary { "Examples of patterns that work" }
                ul { class: "skipped-list",
                    li {
                        code { r"(?i)spent ([0-9.,]+) at (.+?) on" }
                        " — amount in group 1, merchant in group 2."
                    }
                    li {
                        code { r"(?i)debited by ([0-9.,]+)" }
                        " — amount only; the sender's name becomes the merchant."
                    }
                    li {
                        code { r"(?i)(?<kind>credited|debited).*?([0-9.,]+)" }
                        " — with Direction set to read it from the message, the "
                        code { "kind" }
                        " group decides which way the money went."
                    }
                }
                p { class: "muted small",
                    "These are examples to copy and adapt, not rules that exist. Nothing is installed for you."
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            div { class: "formactions",
                button {
                    class: "button button-quiet",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_done.call(()),
                    "Cancel"
                }
                button {
                    class: "button button-primary",
                    r#type: "button",
                    disabled: busy,
                    onclick: submit,
                    if editing_id.is_some() { "Save changes" } else { "Save rule" }
                }
            }
        }
    }
}

fn blank(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
