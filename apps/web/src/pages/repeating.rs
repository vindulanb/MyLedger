//! The Recurring and Subscriptions screens.
//!
//! One implementation, two pages. The spec asks for the same panels on each,
//! filtered by `kind` — bills on one, subscriptions on the other — and
//! writing it twice would mean two things to keep in step for no gain. Which
//! page you are on is a parameter, and the words change with it.
//!
//! The totals are the part worth being careful about. The spec asks for a
//! combined monthly and annual commitment "across confirmed items plus
//! visible suggestions, without double-counting". That holds because the
//! server leaves a pattern out of the suggestions once it has been confirmed
//! — the two lists are disjoint by construction, not by a filter applied
//! here that could drift.
//!
//! Detection itself is the backend's: this screen reads
//! `GET /api/v1/recurring/suggestions` and never decides for itself what
//! repeats.

use dioxus::prelude::*;
use myledger_core::recurring::{Confidence, RecurringSuggestion, monthly_equivalent_cents};
use myledger_core::{
    Cadence, KeepSuggestionRequest, RecurringItem, RecurringKind, StateResponse, money,
};
use uuid::Uuid;

use crate::Route;
use crate::api;
use crate::components::EmptyState;
use crate::store::{Load, Store};

#[component]
pub fn Recurring() -> Element {
    rsx! { Repeating { kind: RecurringKind::Recurring } }
}

#[component]
pub fn Subscriptions() -> Element {
    rsx! { Repeating { kind: RecurringKind::Subscription } }
}

/// The words that differ between the two pages.
struct Wording {
    /// What one of these is called, lowercase and singular.
    thing: &'static str,
    /// Plural.
    things: &'static str,
    other_label: &'static str,
    other_route: Route,
}

fn wording(kind: RecurringKind) -> Wording {
    match kind {
        RecurringKind::Recurring => Wording {
            thing: "recurring charge",
            things: "recurring charges",
            other_label: "Subscriptions",
            other_route: Route::Subscriptions {},
        },
        RecurringKind::Subscription => Wording {
            thing: "subscription",
            things: "subscriptions",
            other_label: "Recurring",
            other_route: Route::Recurring {},
        },
    }
}

#[component]
fn Repeating(kind: RecurringKind) -> Element {
    let store = Store::use_store();

    // The suggestions are not part of `/state` — detection is a computation
    // over the history, not a stored list — so this screen fetches them
    // itself and refetches whenever the ledger changes underneath it.
    let mut suggestions = use_signal(Vec::<RecurringSuggestion>::new);
    let mut hidden_kept = use_signal(|| 0usize);
    let mut hidden_ignored = use_signal(|| 0usize);
    let mut as_of = use_signal(String::new);
    let mut detecting = use_signal(|| true);
    let mut detection_failed = use_signal(|| None::<String>);

    let reload_suggestions = move || {
        spawn(async move {
            detecting.set(true);
            match api::recurring_suggestions().await {
                Ok(found) => {
                    suggestions.set(found.suggestions);
                    hidden_kept.set(found.hidden_as_kept);
                    hidden_ignored.set(found.hidden_as_ignored);
                    as_of.set(found.as_of);
                    detection_failed.set(None);
                }
                Err(failure) => {
                    if failure.is_unauthorized() {
                        store.fail(failure);
                    } else {
                        detection_failed.set(Some(failure.message));
                    }
                }
            }
            detecting.set(false);
        });
    };

    use_hook(reload_suggestions);

    let ledger = store.ledger.read();
    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "{wording(kind).things}" }
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

    let words = wording(kind);

    let confirmed: Vec<RecurringItem> = state
        .recurring_items
        .iter()
        .filter(|item| item.deleted_at.is_none() && item.kind == kind)
        .cloned()
        .collect();

    let mine: Vec<RecurringSuggestion> =
        suggestions().into_iter().filter(|s| s.kind == kind).collect();

    // Confirmed and suggested are disjoint — see the module note — so these
    // add without any risk of counting a pattern twice.
    let confirmed_monthly: i64 = confirmed
        .iter()
        .filter(|item| item.active)
        .map(|item| monthly_equivalent_cents(item.amount_cents, item.cadence))
        .sum();
    let suggested_monthly: i64 = mine.iter().map(|s| s.monthly_equivalent_cents).sum();

    rsx! {
        section { class: "stack",
            DetectionBanner {
                detecting: detecting(),
                failed: detection_failed(),
                as_of: as_of(),
                found: mine.len(),
                hidden_kept: hidden_kept(),
                hidden_ignored: hidden_ignored(),
                thing: words.things,
            }

            Commitment {
                confirmed_monthly,
                suggested_monthly,
                confirmed_count: confirmed.iter().filter(|i| i.active).count(),
                suggested_count: mine.len(),
                things: words.things,
            }

            div { class: "card",
                header { class: "card-head",
                    h2 { class: "card-title", "Detected" }
                    button {
                        class: "button button-quiet button-small",
                        r#type: "button",
                        disabled: detecting(),
                        onclick: move |_| reload_suggestions(),
                        if detecting() { "Looking…" } else { "Run detection again" }
                    }
                }

                if mine.is_empty() {
                    EmptyState {
                        title: "Nothing new detected",
                        blurb: if state.transactions.is_empty() {
                            "Detection reads your transaction history. Once there are a few months of it, repeating charges turn up here."
                        } else {
                            "No unconfirmed repeating pattern looks like one of these yet. Charges need to repeat a few times before detection will propose them."
                        },
                        Link { to: words.other_route.clone(), class: "button button-quiet",
                            "Check {words.other_label}"
                        }
                    }
                } else {
                    div { class: "suggestions",
                        for suggestion in mine.iter() {
                            SuggestionCard {
                                key: "{suggestion.pattern_key}",
                                suggestion: suggestion.clone(),
                                on_answered: move |_| reload_suggestions(),
                            }
                        }
                    }
                }
            }

            ConfirmedList { state: state.clone(), kind, items: confirmed }
        }
    }
}

/// "Active detection", and what it found.
#[component]
fn DetectionBanner(
    detecting: bool,
    failed: Option<String>,
    as_of: String,
    found: usize,
    hidden_kept: usize,
    hidden_ignored: usize,
    thing: &'static str,
) -> Element {
    if let Some(message) = failed {
        return rsx! {
            div { class: "banner banner-error", role: "alert",
                span { "Detection could not run: {message}" }
            }
        };
    }

    rsx! {
        div { class: "detection",
            div { class: "detection-state",
                span { class: if detecting { "dot is-busy" } else { "dot is-on" }, "aria-hidden": "true" }
                strong { if detecting { "Detection running" } else { "Detection active" } }
            }
            p { class: "muted small",
                "MyLedger reads the last three years of transactions and looks for charges that repeat on a regular cadence. "
                if !as_of.is_empty() {
                    "Last run against {as_of}. "
                }
                if detecting {
                    "Looking now…"
                } else if found == 0 {
                    "No unconfirmed {thing} right now."
                } else if found == 1 {
                    "1 pattern is waiting for your decision."
                } else {
                    "{found} patterns are waiting for your decision."
                }
            }
            // Said out loud, so a missing subscription is explained rather
            // than mysterious.
            if hidden_kept > 0 || hidden_ignored > 0 {
                p { class: "muted small",
                    if hidden_kept > 0 {
                        "{hidden_kept} already confirmed. "
                    }
                    if hidden_ignored > 0 {
                        "{hidden_ignored} ignored — they can be restored in Settings."
                    }
                }
            }
        }
    }
}

/// The combined monthly and annual commitment.
#[component]
fn Commitment(
    confirmed_monthly: i64,
    suggested_monthly: i64,
    confirmed_count: usize,
    suggested_count: usize,
    things: &'static str,
) -> Element {
    let total = confirmed_monthly.saturating_add(suggested_monthly);

    rsx! {
        div { class: "card",
            h2 { class: "card-title", "Estimated commitment" }
            div { class: "summaries",
                div { class: "summary",
                    span { class: "summary-label", "Per month" }
                    strong { class: "summary-value", "{money(total)}" }
                    span { class: "summary-detail",
                        "Confirmed {money(confirmed_monthly)} + detected {money(suggested_monthly)}"
                    }
                }
                div { class: "summary",
                    span { class: "summary-label", "Per year" }
                    // Twelve times the monthly equivalent, which is what
                    // levelling out a weekly or annual charge is for.
                    strong { class: "summary-value", "{money(total.saturating_mul(12))}" }
                    span { class: "summary-detail", "Twelve times the monthly figure" }
                }
                div { class: "summary summary-muted",
                    span { class: "summary-label", "Counted" }
                    strong { class: "summary-value", "{confirmed_count + suggested_count}" }
                    span { class: "summary-detail",
                        "{confirmed_count} confirmed, {suggested_count} detected"
                    }
                }
            }
            p { class: "muted small",
                "A detected pattern stops being counted here once you confirm it, so nothing is counted twice. Figures are estimates from past charges, not a forecast — a price rise shows up only after it has been charged."
            }
            if confirmed_count == 0 && suggested_count == 0 {
                p { class: "muted small", "Nothing to total yet: no {things} are confirmed or detected." }
            }
        }
    }
}

/// One detected pattern, with Keep and Ignore.
#[component]
fn SuggestionCard(
    suggestion: RecurringSuggestion,
    on_answered: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let key = suggestion.pattern_key.clone();

    rsx! {
        div { class: "suggestion",
            div { class: "suggestion-head",
                div {
                    strong { class: "suggestion-name", "{suggestion.merchant}" }
                    span { class: "suggestion-meta",
                        "{cadence_word(suggestion.cadence)}"
                        if let Some(name) = &suggestion.category_name {
                            " · {name}"
                        }
                    }
                }
                ConfidenceBadge { confidence: suggestion.confidence }
            }

            dl { class: "factgrid factgrid-tight",
                div { class: "fact",
                    dt { class: "fact-label", "Average charge" }
                    dd { class: "fact-value small-value", "{money(suggestion.average_amount_cents)}" }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Per month" }
                    dd { class: "fact-value small-value", "{money(suggestion.monthly_equivalent_cents)}" }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Seen" }
                    dd { class: "fact-value small-value",
                        "{suggestion.occurrences} times"
                    }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Next expected" }
                    dd { class: "fact-value small-value", "{suggestion.next_expected_date}" }
                }
            }

            p { class: "muted small",
                "Last charged {suggestion.last_seen}."
            }

            div { class: "suggestion-actions",
                button {
                    class: "button button-primary button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: {
                        let key = key.clone();
                        move |_| {
                            let key = key.clone();
                            spawn(async move {
                                // Sent empty: accepting it as detected is the
                                // normal case, and the server decides whether
                                // it is a bill or a subscription.
                                let request = KeepSuggestionRequest::default();
                                if store
                                    .act(api::keep_suggestion(&key, &request), "Confirmed.")
                                    .await
                                    .is_some()
                                {
                                    on_answered.call(());
                                }
                            });
                        }
                    },
                    "Keep"
                }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: {
                        let key = key.clone();
                        move |_| {
                            let key = key.clone();
                            spawn(async move {
                                if store
                                    .act(
                                        api::ignore_suggestion(&key),
                                        "Ignored. Restore it from Settings if you change your mind.",
                                    )
                                    .await
                                    .is_some()
                                {
                                    on_answered.call(());
                                }
                            });
                        }
                    },
                    "Ignore"
                }
            }
        }
    }
}

/// How sure detection is.
#[component]
fn ConfidenceBadge(confidence: Confidence) -> Element {
    // Detection reports two levels, not three: enough evidence, or thinner
    // evidence. There is no "low" — a pattern that weak is not suggested at
    // all, which is better than offering something nobody should act on.
    let (label, title) = match confidence {
        Confidence::High => ("High", "Enough charges, at a steady amount and on a steady schedule"),
        Confidence::Likely => {
            ("Likely", "Consistent with a repeating charge, but on thinner evidence")
        }
    };
    rsx! {
        span { class: "badge badge-confidence-{confidence.as_str()}", title: title,
            "{label} confidence"
        }
    }
}

/// The confirmed items, with add, edit and delete.
#[component]
fn ConfirmedList(state: StateResponse, kind: RecurringKind, items: Vec<RecurringItem>) -> Element {
    let store = Store::use_store();
    let mut adding = use_signal(|| false);
    let mut editing = use_signal(|| None::<Uuid>);
    let busy = *store.saving.read();
    let words = wording(kind);

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Confirmed" }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    onclick: move |_| {
                        let open = adding();
                        adding.set(!open);
                        if !open {
                            editing.set(None);
                        }
                    },
                    if adding() { "Cancel" } else { "Add a {words.thing}" }
                }
            }

            if adding() {
                ItemForm {
                    state: state.clone(),
                    kind,
                    existing: None,
                    on_done: move |_| adding.set(false),
                }
            }

            if items.is_empty() {
                EmptyState {
                    title: "No confirmed {words.things}",
                    blurb: "Keep one of the detected patterns above, or add it yourself.",
                }
            } else {
                ul { class: "minilist",
                    for item in items.iter() {
                        li { key: "{item.id}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "minirow-merchant",
                                        "{item.name}"
                                        if !item.active {
                                            span { class: "badge", title: "Not counted in the totals", "Paused" }
                                        }
                                    }
                                    span { class: "minirow-meta",
                                        "{cadence_word(item.cadence)}"
                                        if let Some(name) = item.category_id.and_then(|id| state.category_name(id)) {
                                            " · {name}"
                                        }
                                        if let Some(date) = &item.next_date {
                                            " · next {date}"
                                        }
                                    }
                                }
                                div { class: "rowactions",
                                    span { class: "amount amount-out", "{money(item.amount_cents)}" }
                                    span { class: "muted small",
                                        "{money(monthly_equivalent_cents(item.amount_cents, item.cadence))}/mo"
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = item.id;
                                            move |_| {
                                                if editing() == Some(id) {
                                                    editing.set(None);
                                                } else {
                                                    editing.set(Some(id));
                                                    adding.set(false);
                                                }
                                            }
                                        },
                                        if editing() == Some(item.id) { "Close" } else { "Edit" }
                                    }
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: {
                                            let id = item.id;
                                            move |_| {
                                                spawn(async move {
                                                    store
                                                        .act(
                                                            api::delete_recurring_item(id),
                                                            "Removed.",
                                                        )
                                                        .await;
                                                });
                                            }
                                        },
                                        "Delete"
                                    }
                                }
                            }

                            if editing() == Some(item.id) {
                                ItemForm {
                                    state: state.clone(),
                                    kind,
                                    existing: Some(item.clone()),
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

/// Adding or editing one item.
#[component]
fn ItemForm(
    state: StateResponse,
    kind: RecurringKind,
    existing: Option<RecurringItem>,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let editing_id = existing.as_ref().map(|item| item.id);

    let mut name = use_signal(|| existing.as_ref().map(|i| i.name.clone()).unwrap_or_default());
    let mut amount = use_signal(|| {
        existing
            .as_ref()
            .map(|i| format!("{}.{:02}", i.amount_cents / 100, i.amount_cents % 100))
            .unwrap_or_default()
    });
    let mut cadence = use_signal(|| {
        existing.as_ref().map(|i| i.cadence).unwrap_or(Cadence::Monthly)
    });
    let mut next_date =
        use_signal(|| existing.as_ref().and_then(|i| i.next_date.clone()).unwrap_or_default());
    let mut category = use_signal(|| existing.as_ref().and_then(|i| i.category_id));
    let mut account = use_signal(|| existing.as_ref().and_then(|i| i.account_id));
    let mut active = use_signal(|| existing.as_ref().map(|i| i.active).unwrap_or(true));
    let mut error = use_signal(|| None::<String>);

    let submit = move |_| {
        let label = name().trim().to_string();
        if label.is_empty() {
            error.set(Some("Give it a name.".to_string()));
            return;
        }
        let Some(cents) = myledger_core::parse_money(&amount()) else {
            error.set(Some("Enter an amount, for example 9.99.".to_string()));
            return;
        };
        if cents <= 0 {
            error.set(Some("The amount has to be more than nothing.".to_string()));
            return;
        }
        let date = next_date().trim().to_string();
        error.set(None);

        spawn(async move {
            let saved = match editing_id {
                Some(id) => {
                    let patch = api::RecurringItemPatch {
                        name: Some(label.clone()),
                        category_id: Some(category()),
                        amount_cents: Some(cents),
                        cadence: Some(cadence()),
                        next_date: Some(if date.is_empty() { None } else { Some(date.clone()) }),
                        account_id: Some(account()),
                        kind: Some(kind),
                        active: Some(active()),
                    };
                    store.act(api::patch_recurring_item(id, &patch), "Saved.").await.is_some()
                }
                None => {
                    let item = api::NewRecurringItem {
                        name: label.clone(),
                        category_id: category(),
                        amount_cents: cents,
                        cadence: cadence(),
                        next_date: if date.is_empty() { None } else { Some(date.clone()) },
                        account_id: account(),
                        kind,
                        active: active(),
                    };
                    store.act(api::create_recurring_item(&item), "Added.").await.is_some()
                }
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
                        name: "item-name",
                        placeholder: "What it is",
                        value: "{name}",
                        disabled: busy,
                        oninput: move |event| name.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Amount" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "item-amount",
                        inputmode: "decimal",
                        placeholder: "0.00",
                        value: "{amount}",
                        disabled: busy,
                        oninput: move |event| amount.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Cadence" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| {
                            if let Ok(chosen) = event.value().parse::<Cadence>() {
                                cadence.set(chosen);
                            }
                        },
                        for option in [
                            Cadence::Weekly,
                            Cadence::Biweekly,
                            Cadence::Monthly,
                            Cadence::Quarterly,
                            Cadence::Annual,
                        ] {
                            option {
                                key: "{option.as_str()}",
                                value: "{option.as_str()}",
                                selected: cadence() == option,
                                "{cadence_word(option)}"
                            }
                        }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Next due" }
                    input {
                        class: "field-input",
                        r#type: "date",
                        name: "item-next",
                        value: "{next_date}",
                        disabled: busy,
                        oninput: move |event| next_date.set(event.value()),
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

                label { class: "field",
                    span { class: "field-label", "Account" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| account.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", "No account" }
                        for item in state.live_accounts() {
                            option {
                                key: "{item.id}",
                                value: "{item.id}",
                                selected: account() == Some(item.id),
                                "{item.name}"
                            }
                        }
                    }
                }
            }

            label { class: "checkline",
                input {
                    r#type: "checkbox",
                    checked: active(),
                    disabled: busy,
                    onchange: move |event| active.set(event.checked()),
                }
                span { "Counted in the totals" }
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
                    if editing_id.is_some() { "Save changes" } else { "Add it" }
                }
            }
        }
    }
}

/// A cadence in words, rather than its stored token.
fn cadence_word(cadence: Cadence) -> &'static str {
    match cadence {
        Cadence::Weekly => "Weekly",
        Cadence::Biweekly => "Every two weeks",
        Cadence::Monthly => "Monthly",
        Cadence::Quarterly => "Quarterly",
        Cadence::Annual => "Yearly",
    }
}
