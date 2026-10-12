//! The Settings screen.
//!
//! Five sections, in order of how often they are touched: net worth, the
//! managed lists, what automatic detection does, pairing a phone, and the
//! danger zone last.
//!
//! Two of these hold the only genuinely irreversible actions in the whole
//! app — minting a pairing token, which is shown once and never again, and
//! erasing everything. Both are built so the irreversible step is the one
//! thing on screen that takes a deliberate act: a typed phrase, or a button
//! that says the secret is about to disappear.

use dioxus::prelude::*;
use myledger_core::{
    CreateDeviceTokenResponse, DeviceTokenSummary, PreferencesRequest, StateResponse, money,
    pairing_payload, parse_money, qr,
};
use uuid::Uuid;

use crate::api;
use crate::components::{DeleteTagConfirm, EmptyState, ListKind, and_list, usage_words};
use crate::store::{Load, Store};

#[component]
pub fn Settings() -> Element {
    let store = Store::use_store();
    let ledger = store.ledger.read();

    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Settings" }
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
            NetWorth { state: state.clone() }
            ManagedList { state: state.clone(), kind: ListKind::Categories }
            ManagedList { state: state.clone(), kind: ListKind::Accounts }
            ManagedList { state: state.clone(), kind: ListKind::Tags }
            Detection { state: state.clone() }
            MobilePairing {}
            DangerZone {}
        }
    }
}

// ---- net worth -------------------------------------------------------------

/// Assets and liabilities, typed by the user and believed.
///
/// Nothing here is derived. A ledger records what moved, not what you own: it
/// has no idea about the equity in a house or the balance left on a car loan,
/// and a figure invented from income minus expenses would be wrong in a way
/// that looks authoritative. So net worth is input, and the panel says so
/// rather than leaving the user to wonder why it doesn't match the dashboard.
#[component]
fn NetWorth(state: StateResponse) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let configured = state.settings.net_worth_configured;

    let mut assets = use_signal(|| cents_to_field(state.settings.assets_cents, configured));
    let mut liabilities =
        use_signal(|| cents_to_field(state.settings.liabilities_cents, configured));
    let mut error = use_signal(|| None::<String>);

    // The preview tracks what is typed, not what is saved, so the figure
    // moves as the user corrects a digit. Both have to parse before there is
    // anything honest to show.
    let typed_assets = parse_money(&assets());
    let typed_liabilities = parse_money(&liabilities());
    let preview = match (typed_assets, typed_liabilities) {
        (Some(a), Some(l)) => Some(a - l),
        _ => None,
    };

    let save = move |_| {
        let (Some(a), Some(l)) = (parse_money(&assets()), parse_money(&liabilities())) else {
            error.set(Some(
                "Both figures have to be amounts — digits, and at most two decimal places. \
                 A debt is entered as a positive number under liabilities."
                    .to_string(),
            ));
            return;
        };
        error.set(None);
        spawn(async move {
            let request = PreferencesRequest {
                assets_cents: Some(a),
                liabilities_cents: Some(l),
                net_worth_configured: Some(true),
                ..Default::default()
            };
            store.save_preferences(request, "Net worth saved.").await;
        });
    };

    let stop = move |_| {
        spawn(async move {
            let request =
                PreferencesRequest { net_worth_configured: Some(false), ..Default::default() };
            store.save_preferences(request, "Net worth is no longer shown.").await;
        });
    };

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Net worth" }
                span { class: "muted small",
                    if configured { "Shown on the dashboard" } else { "Not set" }
                }
            }

            p { class: "muted small",
                "Two figures you keep up to date yourself: what you own, and what you owe. "
                strong { "This is not worked out from your income and spending." }
                " A ledger knows what moved through your accounts, not what a house is worth or how much is left on a loan — so a number derived from it would be confidently wrong."
            }

            div { class: "formgrid",
                label { class: "field",
                    span { class: "field-label", "Assets — what you own" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        inputmode: "decimal",
                        name: "assets",
                        placeholder: "0.00",
                        value: "{assets}",
                        disabled: busy,
                        oninput: move |event| assets.set(event.value()),
                    }
                    span {
                        class: if typed_assets.is_none() && !assets().trim().is_empty() {
                            "field-hint field-hint-bad"
                        } else {
                            "field-hint"
                        },
                        "Savings, investments, property, anything you'd count."
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Liabilities — what you owe" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        inputmode: "decimal",
                        name: "liabilities",
                        placeholder: "0.00",
                        value: "{liabilities}",
                        disabled: busy,
                        oninput: move |event| liabilities.set(event.value()),
                    }
                    span {
                        class: if typed_liabilities.is_none() && !liabilities().trim().is_empty() {
                            "field-hint field-hint-bad"
                        } else {
                            "field-hint"
                        },
                        "Loans, mortgages, card balances. Entered as positive numbers."
                    }
                }
            }

            div { class: "netpreview",
                span { class: "summary-label", "Assets − liabilities" }
                match preview {
                    Some(cents) => rsx! {
                        strong { class: "summary-value", "{money(cents)}" }
                        span { class: "summary-detail",
                            "{money(typed_assets.unwrap_or(0))} owned, {money(typed_liabilities.unwrap_or(0))} owed"
                        }
                    },
                    None => rsx! {
                        strong { class: "summary-value summary-unset", "—" }
                        span { class: "summary-detail", "Fill both figures in to see the total." }
                    },
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            div { class: "formactions",
                if configured {
                    button {
                        class: "button button-quiet",
                        r#type: "button",
                        disabled: busy,
                        onclick: stop,
                        "Stop tracking net worth"
                    }
                }
                button {
                    class: "button button-primary",
                    r#type: "button",
                    disabled: busy || preview.is_none(),
                    onclick: save,
                    if configured { "Save figures" } else { "Start tracking net worth" }
                }
            }
            if configured {
                p { class: "muted small",
                    "Stopping puts the dashboard back to “Not set”. The figures above stay saved, so turning it on again doesn't mean typing them a second time."
                }
            }
        }
    }
}

/// An amount as it should appear in a text field: plain digits and a point,
/// no currency symbol, because the field is parsed back by `parse_money`.
///
/// A stored zero that was never configured comes back as an *empty* field,
/// not as `0.00`. The difference matters: `0.00` is a claim that you own
/// nothing, and pre-filling it would put a confident `$0.00` in the preview
/// and arm the save button before the user has said anything at all. An
/// amount the user did enter is shown even once tracking is switched off,
/// because the panel promises the figures stay.
fn cents_to_field(cents: i64, configured: bool) -> String {
    if cents == 0 && !configured {
        return String::new();
    }
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}

// ---- the managed lists -----------------------------------------------------

/// Add to and remove from one of the three lookup lists.
///
/// Deleting is a soft delete on the backend, so this is far less dangerous
/// than it looks: the row stops being offered, and everything already filed
/// against it still renders with its name. The guard here is not about losing
/// data, it is about not leaving a budget or a rule pointing at something no
/// picker will show.
#[component]
fn ManagedList(state: StateResponse, kind: ListKind) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let mut typed = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut confirming = use_signal(|| None::<Uuid>);

    let rows = kind.rows(&state);
    let singular = kind.singular();

    // A dialog open over the app is a form the user is part-way through, and
    // it may well be holding the very row about to be deleted. Nothing here
    // can see inside it, so the honest move is to decline until it is closed.
    let form_open = store.modal.read().is_some();

    let mut create = {
        let state = state.clone();
        move |_| {
            let name = typed().trim().to_string();
            if name.is_empty() {
                return;
            }
            // Case-insensitive, because two rows differing only in case are
            // indistinguishable in a picker. The backend refuses this too;
            // checking here is what turns a 400 into a sentence.
            if kind.rows(&state).iter().any(|(_, existing)| existing.eq_ignore_ascii_case(&name)) {
                error.set(Some(format!("You already have a {singular} called {name:?}.")));
                return;
            }
            error.set(None);

            let mut wanted = kind.surviving(&state, None);
            wanted.push(myledger_core::NamedInput::new_named(&name));
            let done = match kind {
                ListKind::Categories => "Category added.",
                ListKind::Accounts => "Account added.",
                ListKind::Tags => "Tag added.",
            };
            spawn(async move {
                if store.save_preferences(kind.request(wanted), done).await {
                    typed.set(String::new());
                }
            });
        }
    };

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "{kind.title()}" }
                span { class: "muted small",
                    "{rows.len()} "
                    if rows.len() == 1 { "{singular}" } else { "in the list" }
                }
            }
            p { class: "muted small", "{kind.blurb()}" }

            form {
                class: "tagpicker-new",
                onsubmit: move |event| {
                    event.prevent_default();
                    create(());
                },
                label { class: "field field-grow",
                    span { class: "field-label", "New {singular}" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "new-{singular}",
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
                    "Add"
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            if rows.is_empty() {
                EmptyState {
                    title: "Nothing in this list",
                    blurb: "Add the first one above.",
                }
            } else {
                ul { class: "minilist",
                    for (id, name) in rows.iter().cloned() {
                        li { key: "{id}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "minirow-merchant", "{name}" }
                                    if kind == ListKind::Tags {
                                        span { class: "minirow-meta", {usage_words(state.tag_use_count(id))} }
                                    }
                                }
                                div { class: "rowactions",
                                    button {
                                        class: "button button-quiet button-small",
                                        r#type: "button",
                                        disabled: busy,
                                        onclick: move |_| {
                                            if confirming() == Some(id) {
                                                confirming.set(None);
                                            } else {
                                                confirming.set(Some(id));
                                            }
                                        },
                                        if confirming() == Some(id) { "Keep it" } else { "Delete" }
                                    }
                                }
                            }

                            if confirming() == Some(id) {
                                {
                                    let blockers = kind.blockers(&state, id);
                                    if form_open {
                                        rsx! {
                                            div { class: "confirmbox", role: "alert",
                                                p { class: "confirmbox-title", "Close the open form first" }
                                                p { class: "muted small",
                                                    "You have a form open over the app, and it may be using this {singular}. Finish or cancel it, then come back."
                                                }
                                            }
                                        }
                                    } else if !blockers.is_empty() {
                                        rsx! {
                                            div { class: "confirmbox", role: "alert",
                                                p { class: "confirmbox-title", "Something still uses “{name}”" }
                                                p { class: "muted small",
                                                    "It is named by {and_list(&blockers)}. Point "
                                                    if blockers.len() == 1 { "that" } else { "those" }
                                                    " somewhere else first, then delete it — otherwise you'd be left with a setting that names a {singular} no picker offers."
                                                }
                                            }
                                        }
                                    } else if kind == ListKind::Tags {
                                        rsx! {
                                            DeleteTagConfirm {
                                                state: state.clone(),
                                                tag_id: id,
                                                tag_name: name.clone(),
                                                uses: state.tag_use_count(id),
                                                on_done: move |_| confirming.set(None),
                                            }
                                        }
                                    } else {
                                        rsx! {
                                            DeleteLookupConfirm {
                                                state: state.clone(),
                                                kind,
                                                id,
                                                name: name.clone(),
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
        }
    }
}

/// Confirming the removal of a category or an account.
///
/// Shorter than the tag version because there is no choice to make: history
/// always keeps the name, and there is no option that would change that.
#[component]
fn DeleteLookupConfirm(
    state: StateResponse,
    kind: ListKind,
    id: Uuid,
    name: String,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let singular = kind.singular();

    let remove = move |_| {
        let wanted = kind.surviving(&state, Some(id));
        let done = match kind {
            ListKind::Categories => "Category deleted.",
            ListKind::Accounts => "Account deleted.",
            ListKind::Tags => "Tag deleted.",
        };
        spawn(async move {
            if store.save_preferences(kind.request(wanted), done).await {
                on_done.call(());
            }
        });
    };

    rsx! {
        div { class: "confirmbox", role: "group", "aria-label": "Delete the {singular} {name}",
            p { class: "confirmbox-title", "Delete “{name}”?" }
            p { class: "muted small",
                "It stops being offered when you file something. Transactions already filed against it keep showing the name, so nothing in your history becomes unreadable — and nothing is deleted from the ledger."
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
                    "Delete {singular}"
                }
            }
        }
    }
}

// ---- automatic detection ---------------------------------------------------

/// What the app works out on its own, said plainly, and the one switch that
/// undoes a decision made about it.
#[component]
fn Detection(state: StateResponse) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let ignored = state.dismissed_patterns.len();

    let restore = move |_| {
        spawn(async move {
            // A group present in a preferences request is taken as the whole
            // list, so an empty one is how you clear it.
            let request =
                PreferencesRequest { dismissed_patterns: Some(Vec::new()), ..Default::default() };
            store.save_preferences(request, "Ignored suggestions restored.").await;
        });
    };

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Automatic detection" }
            }

            p { class: "muted small",
                "MyLedger looks through your transactions for merchants that charge you on a schedule, and offers them on the Recurring and Subscriptions screens. It only ever "
                strong { "suggests" }
                " — nothing is added to your ledger, changed, or counted as a bill until you accept it."
            }
            ul { class: "plainlist muted small",
                li { "A merchant needs at least two charges before it can be a pattern at all, and the gaps between them have to be regular enough to name — weekly, monthly, yearly and so on." }
                li { "The price has to be steady. A subscription may drift by a fifth; a utility bill, which genuinely moves with the season, is allowed a third." }
                li { "A merchant whose name gives no clue — not a known subscription, not a word like “rent” or “insurance” — has to be near-identical in price and to have happened at least three times. That is deliberately strict, so a weekly grocery run is never called a subscription." }
                li { "A pattern goes quiet after three missed periods. A monthly charge unseen for three months has almost certainly been cancelled, and going on suggesting it is just noise." }
                li { "Differently-spelled versions of one merchant are collapsed together first, so “TESCO STORES 2831” and “Tesco Stores” count as the same shop." }
            }

            div { class: "minirow",
                div { class: "minirow-main",
                    span { class: "minirow-merchant", "Ignored suggestions" }
                    span { class: "minirow-meta",
                        if ignored == 0 {
                            "You haven't dismissed any."
                        } else if ignored == 1 {
                            "1 merchant is hidden from the suggestions."
                        } else {
                            "{ignored} merchants are hidden from the suggestions."
                        }
                    }
                }
                div { class: "rowactions",
                    button {
                        class: "button button-quiet button-small",
                        r#type: "button",
                        disabled: busy || ignored == 0,
                        onclick: restore,
                        "Restore ignored suggestions"
                    }
                }
            }
            if ignored > 0 {
                p { class: "muted small",
                    "Restoring brings every dismissed merchant back as a suggestion. It changes nothing in your ledger — a suggestion is only ever an offer."
                }
            }
        }
    }
}

// ---- mobile pairing --------------------------------------------------------

/// Paired phones, and minting a token for a new one.
///
/// The token is the one secret this app ever shows. It is held in a signal
/// that belongs to this component, so it is gone the moment the user
/// navigates away — and it is nowhere else: not in `localStorage`, not in the
/// URL, not in the ledger. The server only ever stored its hash, so there is
/// genuinely no way back to it.
#[component]
fn MobilePairing() -> Element {
    let store = Store::use_store();
    let mut devices = use_signal(Vec::<DeviceTokenSummary>::new);
    let mut loading = use_signal(|| true);
    let mut label = use_signal(String::new);
    let mut minting = use_signal(|| false);
    let mut issued = use_signal(|| None::<CreateDeviceTokenResponse>);

    // Fetched on mount. Device tokens are not part of `/state` — they are
    // credentials, not ledger data, and keeping them out of the one big read
    // means nothing can accidentally render them beside a transaction.
    let reload_devices = move || {
        spawn(async move {
            loading.set(true);
            match api::device_tokens().await {
                Ok(found) => devices.set(found),
                Err(failure) => store.fail(failure),
            }
            loading.set(false);
        });
    };
    use_hook(reload_devices);

    let mut generate = move |_| {
        let wanted = label().trim().to_string();
        minting.set(true);
        spawn(async move {
            match api::create_device_token(
                if wanted.is_empty() { None } else { Some(wanted) },
            )
            .await
            {
                Ok(response) => {
                    issued.set(Some(response));
                    label.set(String::new());
                    // The new device belongs in the list below the token.
                    if let Ok(found) = api::device_tokens().await {
                        devices.set(found);
                    }
                }
                Err(failure) => store.fail(failure),
            }
            minting.set(false);
        });
    };

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Mobile pairing" }
            }
            p { class: "muted small",
                "The Android app signs in with a pairing token instead of your password, so the phone never holds the credential that could change your settings. Generate one when you install it."
            }

            if let Some(token) = issued() {
                IssuedToken { token, on_done: move |_| issued.set(None) }
            } else {
                form {
                    class: "tagpicker-new",
                    onsubmit: move |event| {
                        event.prevent_default();
                        generate(());
                    },
                    label { class: "field field-grow",
                        span { class: "field-label", "Name this device (optional)" }
                        input {
                            class: "field-input",
                            r#type: "text",
                            name: "device-label",
                            placeholder: "e.g. My phone",
                            value: "{label}",
                            disabled: minting(),
                            oninput: move |event| label.set(event.value()),
                        }
                    }
                    button {
                        class: "button button-primary button-small",
                        r#type: "submit",
                        disabled: minting(),
                        if minting() { "Generating…" } else { "Generate pairing token" }
                    }
                }
            }

            h3 { class: "subheading", "Paired devices" }
            if loading() {
                p { class: "muted small", "Checking…" }
            } else if devices().is_empty() {
                EmptyState {
                    title: "No devices paired",
                    blurb: "Nothing has been paired with this ledger yet. Generate a token above when you're ready to set the phone up.",
                }
            } else {
                ul { class: "minilist",
                    for (index, device) in devices().iter().enumerate() {
                        li { key: "{index}", class: "itemrow",
                            div { class: "minirow",
                                div { class: "minirow-main",
                                    span { class: "minirow-merchant",
                                        {device.label.clone().unwrap_or_else(|| "Unnamed device".to_string())}
                                    }
                                    span { class: "minirow-meta",
                                        "Paired {short_date(&device.created_at)} · "
                                        match &device.last_used_at {
                                            Some(when) => rsx! { "last used {short_date(when)}" },
                                            None => rsx! { "never used yet" },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A freshly minted token, shown once.
#[component]
fn IssuedToken(token: CreateDeviceTokenResponse, on_done: EventHandler<()>) -> Element {
    // The origin the browser itself reached this server on, which is the
    // address the phone needs too. The server knows only what it bound to —
    // often `0.0.0.0` — which would be useless in a QR code.
    let origin = browser_origin();
    let payload = pairing_payload(&origin, &token.token);
    let code = qr::encode_svg(&payload);

    rsx! {
        div { class: "tokenbox", role: "group", "aria-label": "Your new pairing token",
            p { class: "confirmbox-title", "Scan this, or copy the token" }
            p { class: "formerror", role: "alert",
                "{token.warning} Once you leave this screen or press Done, it is gone — the server kept only a hash of it."
            }

            div { class: "tokenlayout",
                match code {
                    Some(rendered) => rsx! {
                        div { class: "qrframe", dangerous_inner_html: "{rendered.svg}" }
                    },
                    // Only reachable if the payload were too long to encode,
                    // which a pairing URL is not — but a missing QR is not a
                    // reason to withhold the token itself.
                    None => rsx! {
                        div { class: "qrframe qrframe-empty",
                            span { class: "muted small", "No QR code — use the token below." }
                        }
                    },
                }

                div { class: "tokendetail",
                    span { class: "field-label", "Pairing token" }
                    code { class: "tokenvalue", "{token.token}" }
                    span { class: "field-label", "Server address" }
                    code { class: "tokenvalue", "{origin}" }
                    p { class: "muted small",
                        "The QR code carries both, so scanning is the whole of setup. Typing them in by hand works just as well."
                    }
                }
            }

            div { class: "formactions",
                button {
                    class: "button button-primary",
                    r#type: "button",
                    onclick: move |_| on_done.call(()),
                    "Done — I've saved it"
                }
            }
        }
    }
}

/// The origin this page was served from, e.g. `http://192.168.1.5:8787`.
fn browser_origin() -> String {
    web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_else(|| "this server".to_string())
}

/// The date part of a stored timestamp.
///
/// Every timestamp in the database is RFC 3339 to the millisecond, so the
/// first ten characters are the date. Shown without the time because "paired
/// at 14:06:22.431" is more precision than anyone wants about a phone.
fn short_date(timestamp: &str) -> String {
    timestamp.get(..10).unwrap_or(timestamp).to_string()
}

// ---- the danger zone -------------------------------------------------------

/// What the typed confirmation has to be, exactly.
const WIPE_WORD: &str = "DELETE";

/// Erasing everything.
///
/// Last on the page, behind a typed word, and wordy on purpose: this is the
/// only action in MyLedger with no undo anywhere.
#[component]
fn DangerZone() -> Element {
    let store = Store::use_store();
    let mut typed = use_signal(String::new);
    let mut wiping = use_signal(|| false);

    let armed = typed().trim() == WIPE_WORD;

    let wipe = move |_| {
        if !armed {
            return;
        }
        wiping.set(true);
        spawn(async move {
            match api::wipe_state().await {
                Ok(report) => {
                    typed.set(String::new());
                    // Reload before reporting, so the screens behind this are
                    // already empty when the message appears.
                    store.reload().await;
                    let rows = report.deleted.total;
                    let files = report.document_files_removed;
                    store.say(format!(
                        "Erased {rows} {} and {files} stored {}. You are still signed in.",
                        if rows == 1 { "record" } else { "records" },
                        if files == 1 { "file" } else { "files" },
                    ));
                }
                Err(failure) => store.fail(failure),
            }
            wiping.set(false);
        });
    };

    rsx! {
        div { class: "card dangerzone",
            header { class: "card-head",
                h2 { class: "card-title", "Danger zone" }
            }

            p { strong { "Erase all MyLedger data." } }
            p { class: "muted small",
                "This deletes every transaction, tag, budget, goal, repeating item, rule and uploaded document from this server, along with the stored files behind those documents. There is no undo and no backup — if you want one, take a copy of the database file first."
            }
            ul { class: "plainlist muted small",
                li { "Your sign-in survives. You will not be logged out." }
                li { "Paired phones stay paired. Their tokens keep working." }
                li { "Your categories and accounts stay as you arranged them — they are a list you curated, not a record of money." }
                li {
                    strong { "Nothing on your phone's own SMS inbox is touched." }
                    " MyLedger has never had access to it from here, and a wipe reaches only this server's database and its document folder."
                }
            }

            label { class: "field field-wide",
                span { class: "field-label", "Type {WIPE_WORD} to confirm" }
                input {
                    class: "field-input",
                    r#type: "text",
                    name: "wipe-confirm",
                    autocomplete: "off",
                    placeholder: WIPE_WORD,
                    value: "{typed}",
                    disabled: wiping(),
                    oninput: move |event| typed.set(event.value()),
                }
                span { class: "field-hint",
                    if armed {
                        "The button below is now live."
                    } else {
                        "In capitals, exactly. The button stays disabled until it matches."
                    }
                }
            }

            div { class: "formactions",
                button {
                    class: "button button-danger",
                    r#type: "button",
                    disabled: !armed || wiping(),
                    onclick: wipe,
                    if wiping() { "Erasing…" } else { "Erase all MyLedger data" }
                }
            }
        }
    }
}
