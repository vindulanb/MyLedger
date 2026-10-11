//! The Transactions screen.
//!
//! Filtering happens here over the ledger the store already holds, using
//! `core::filter` — no round trip, and one tested definition of what
//! "matches" means rather than one here and another on the server.
//!
//! Inline edits are the opposite: they go straight to the backend and the
//! whole ledger is refetched. The cell does not update until the database
//! has, because the alternative is a screen that shows a category the
//! database refused.

use dioxus::prelude::*;
use myledger_core::{
    AccountChoice, CategoryChoice, NamedInput, PreferencesRequest, StateResponse, Transaction,
    TransactionKind, TransactionSource, filter, money, signed_money,
};
use uuid::Uuid;

use crate::api;
use crate::components::{EmptyState, PeriodSelector};
use crate::store::{Load, Store, today};

#[component]
pub fn Transactions() -> Element {
    let store = Store::use_store();

    let mut query = use_signal(String::new);
    let mut category = use_signal(|| CategoryChoice::Any);
    let mut account = use_signal(|| AccountChoice::Any);

    // Which row has its category or tag cell open. One at a time, so two
    // dropdowns can never both be editing the same thing.
    let editing_category = use_signal(|| None::<Uuid>);
    let editing_tags = use_signal(|| None::<Uuid>);

    let ledger = store.ledger.read();
    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Transactions" }
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

    let active = filter::TransactionFilter {
        query: query(),
        category: category(),
        account: account(),
        range: store.period().range(&today()),
    };

    let rows = filter::apply(&state.transactions, &active, state);
    let (income, expense) = filter::totals(&rows);
    let shown = rows.len();
    let total = state.transactions.iter().filter(|t| t.deleted_at.is_none()).count();

    rsx! {
        section { class: "stack",
            header { class: "pagehead",
                PeriodSelector {}
            }

            div { class: "card",
                div { class: "filterbar",
                    label { class: "field field-grow",
                        span { class: "field-label", "Search" }
                        input {
                            class: "field-input",
                            r#type: "search",
                            placeholder: "Merchant, category or tag",
                            value: "{query}",
                            oninput: move |event| query.set(event.value()),
                        }
                    }

                    label { class: "field",
                        span { class: "field-label", "Category" }
                        select {
                            class: "field-input",
                            onchange: move |event| category.set(parse_category(&event.value())),
                            option { value: "any", selected: category() == CategoryChoice::Any, "All categories" }
                            option { value: "none", selected: category() == CategoryChoice::None, "No category" }
                            for item in state.live_categories() {
                                option {
                                    key: "{item.id}",
                                    value: "{item.id}",
                                    selected: category() == CategoryChoice::One(item.id),
                                    "{item.name}"
                                }
                            }
                        }
                    }

                    label { class: "field",
                        span { class: "field-label", "Account" }
                        select {
                            class: "field-input",
                            onchange: move |event| account.set(parse_account(&event.value())),
                            option { value: "any", selected: account() == AccountChoice::Any, "All accounts" }
                            option { value: "none", selected: account() == AccountChoice::None, "No account" }
                            for item in state.live_accounts() {
                                option {
                                    key: "{item.id}",
                                    value: "{item.id}",
                                    selected: account() == AccountChoice::One(item.id),
                                    "{item.name}"
                                }
                            }
                        }
                    }

                    if active.is_narrowing() {
                        button {
                            class: "button button-quiet",
                            r#type: "button",
                            onclick: move |_| {
                                query.set(String::new());
                                category.set(CategoryChoice::Any);
                                account.set(AccountChoice::Any);
                            },
                            "Clear filters"
                        }
                    }
                }

                p { class: "muted small filter-count",
                    if total == 0 {
                        "Nothing recorded yet."
                    } else if shown == total {
                        "Showing all {total} transactions."
                    } else {
                        "Showing {shown} of {total} transactions."
                    }
                }

                if rows.is_empty() {
                    if total == 0 {
                        EmptyState {
                            title: "No transactions yet",
                            blurb: "Use Add entry in the top bar to record one, or Import to bring in a bank statement.",
                        }
                    } else {
                        EmptyState {
                            title: "Nothing matches those filters",
                            blurb: "Widen the period, or clear the search and filters above.",
                        }
                    }
                } else {
                    TransactionTable {
                        state: state.clone(),
                        rows: rows.iter().map(|t| (*t).clone()).collect::<Vec<_>>(),
                        editing_category,
                        editing_tags,
                    }

                    div { class: "tablefoot",
                        span { "{shown} shown" }
                        span { class: "amount amount-in", "In {money(income)}" }
                        span { class: "amount amount-out", "Out {money(expense)}" }
                    }
                }

                if state.transactions_truncated {
                    p { class: "muted small",
                        "The ledger holds more than the most recent 5,000 transactions; only those are shown."
                    }
                }
            }
        }
    }
}

/// The table. Falls back to stacked cards on a narrow screen, via CSS.
#[component]
fn TransactionTable(
    state: StateResponse,
    rows: Vec<Transaction>,
    editing_category: Signal<Option<Uuid>>,
    editing_tags: Signal<Option<Uuid>>,
) -> Element {
    rsx! {
        div { class: "tablewrap",
            table { class: "ledger",
                thead {
                    tr {
                        th { scope: "col", "Date" }
                        th { scope: "col", "Merchant" }
                        th { scope: "col", "Category" }
                        th { scope: "col", "Account" }
                        th { scope: "col", "Tags" }
                        th { scope: "col", class: "numeric", "Amount" }
                        th { scope: "col", class: "right", span { class: "visually-hidden", "Actions" } }
                    }
                }
                tbody {
                    for transaction in rows.iter() {
                        TransactionRow {
                            key: "{transaction.id}",
                            state: state.clone(),
                            transaction: transaction.clone(),
                            editing_category,
                            editing_tags,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TransactionRow(
    state: StateResponse,
    transaction: Transaction,
    editing_category: Signal<Option<Uuid>>,
    editing_tags: Signal<Option<Uuid>>,
) -> Element {
    let store = Store::use_store();
    let id = transaction.id;
    let is_income = transaction.kind == TransactionKind::Income;
    let busy = *store.saving.read();

    let category_name =
        transaction.category_id.and_then(|c| state.category_name(c)).map(str::to_string);
    let account_name =
        transaction.account_id.and_then(|a| state.account_name(a)).map(str::to_string);

    rsx! {
        tr {
            td { class: "cell-date", "data-label": "Date", "{transaction.date}" }

            td { class: "cell-merchant", "data-label": "Merchant",
                span { class: "merchant-name", "{transaction.merchant}" }
                div { class: "rowmarks",
                    SourceBadge { source: transaction.source }
                    if transaction.receipt {
                        // A paperclip, titled rather than left as a mystery
                        // glyph.
                        span { class: "receipt", title: "A receipt is attached",
                            "aria-label": "Receipt attached",
                            "📎"
                        }
                    }
                }
            }

            td { class: "cell-category", "data-label": "Category",
                if editing_category() == Some(id) {
                    select {
                        class: "field-input inline-select",
                        autofocus: true,
                        disabled: busy,
                        onchange: move |event| {
                            let chosen = parse_optional_uuid(&event.value());
                            editing_category.set(None);
                            spawn(async move {
                                store
                                    .patch_transaction(
                                        id,
                                        api::TransactionPatch {
                                            category_id: Some(chosen),
                                            ..Default::default()
                                        },
                                    )
                                    .await;
                            });
                        },
                        onblur: move |_| editing_category.set(None),
                        option { value: "", selected: transaction.category_id.is_none(), "No category" }
                        for item in state.live_categories() {
                            option {
                                key: "{item.id}",
                                value: "{item.id}",
                                selected: transaction.category_id == Some(item.id),
                                "{item.name}"
                            }
                        }
                    }
                } else {
                    button {
                        class: "cellbutton",
                        r#type: "button",
                        disabled: busy,
                        "aria-label": match &category_name {
                            Some(name) => format!("Category: {name}. Click to change."),
                            None => "No category. Click to set one.".to_string(),
                        },
                        onclick: move |_| editing_category.set(Some(id)),
                        match &category_name {
                            Some(name) => rsx! { "{name}" },
                            None => rsx! { span { class: "muted", "Set category" } },
                        }
                    }
                }
            }

            td { class: "cell-account", "data-label": "Account",
                match &account_name {
                    Some(name) => rsx! { "{name}" },
                    None => rsx! { span { class: "muted", "—" } },
                }
            }

            td { class: "cell-tags", "data-label": "Tags",
                div { class: "tagcell",
                    for tag in transaction.tags.iter() {
                        if let Some(name) = state.tag_name(*tag) {
                            Pill {
                                key: "{tag}",
                                label: name.to_string(),
                                on_remove: {
                                    let removed = *tag;
                                    let remaining: Vec<Uuid> = transaction
                                        .tags
                                        .iter()
                                        .copied()
                                        .filter(|t| *t != removed)
                                        .collect();
                                    EventHandler::new(move |_| {
                                        let remaining = remaining.clone();
                                        spawn(async move {
                                            store
                                                .patch_transaction(
                                                    id,
                                                    api::TransactionPatch {
                                                        tags: Some(remaining),
                                                        ..Default::default()
                                                    },
                                                )
                                                .await;
                                        });
                                    })
                                },
                            }
                        }
                    }
                    button {
                        class: "pill pill-add",
                        r#type: "button",
                        disabled: busy,
                        "aria-label": "Add a tag to {transaction.merchant}",
                        onclick: move |_| editing_tags.set(Some(id)),
                        "+"
                    }
                }
            }

            td {
                class: if is_income { "numeric amount amount-in" } else { "numeric amount amount-out" },
                "data-label": "Amount",
                "{signed_money(transaction.amount_cents, is_income)}"
            }

            td { class: "right",
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| {
                        spawn(async move {
                            store.remove_transaction(id).await;
                        });
                    },
                    "Delete"
                }
            }
        }

        // Rendered as a row of its own so the dialog is inside the table's
        // DOM order, which keeps the tab order sensible.
        if editing_tags() == Some(id) {
            tr { class: "tagrow",
                td { colspan: "7",
                    TagPicker {
                        state: state.clone(),
                        transaction: transaction.clone(),
                        on_close: move |_| editing_tags.set(None),
                    }
                }
            }
        }
    }
}

/// A removable tag pill.
#[component]
fn Pill(label: String, on_remove: EventHandler<()>) -> Element {
    rsx! {
        span { class: "pill",
            span { class: "pill-label", "{label}" }
            button {
                class: "pill-remove",
                r#type: "button",
                "aria-label": "Remove tag {label}",
                onclick: move |_| on_remove.call(()),
                "×"
            }
        }
    }
}

/// Attaching a tag. Tags only — it never asks for a category.
///
/// A name that doesn't exist yet is created first, through
/// `PUT /preferences`, and then attached; otherwise adding a tag would mean
/// leaving this screen to go and define it somewhere else.
///
/// The tag list and the currently attached set are held in signals rather
/// than read from the props inside each handler. Signals are `Copy`, so every
/// closure here captures only cheap values and none of them has to take
/// ownership of something another one still needs.
#[component]
fn TagPicker(
    state: StateResponse,
    transaction: Transaction,
    on_close: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let id = transaction.id;
    let busy = *store.saving.read();

    let mut typed = use_signal(String::new);
    let attached = use_signal(|| transaction.tags.clone());
    let known = use_signal(|| {
        state.live_tags().map(|tag| (tag.id, tag.name.clone())).collect::<Vec<_>>()
    });

    let available: Vec<(Uuid, String)> = known()
        .into_iter()
        .filter(|(tag, _)| !attached().contains(tag))
        .collect();

    // Attaches an existing tag.
    //
    // The picker closes *after* the save, not before it. A Dioxus task
    // belongs to the scope that spawned it, so closing first would unmount
    // this component and cancel the very request it had just started — the
    // same trap that lost the ledger fetch during sign-in. Staying visible
    // and disabled while it saves is also the more honest thing to show.
    let attach = move |tag: Uuid| {
        let mut tags = attached();
        if !tags.contains(&tag) {
            tags.push(tag);
        }
        spawn(async move {
            store
                .patch_transaction(
                    id,
                    api::TransactionPatch { tags: Some(tags), ..Default::default() },
                )
                .await;
            on_close.call(());
        });
    };

    // Creates a tag by name, then attaches it. An existing name — in any
    // casing — is reused rather than duplicated, so typing a tag that is
    // already there does not end up with two of it.
    //
    // Two round trips, because the server assigns the ID: create the tag,
    // read back what it was called, then attach it. The close waits for both,
    // for the reason given on `attach`.
    let create = move || {
        let name = typed().trim().to_string();
        if name.is_empty() {
            return;
        }

        if let Some((tag, _)) =
            known().into_iter().find(|(_, existing)| existing.eq_ignore_ascii_case(&name))
        {
            attach(tag);
            return;
        }

        // The tag group is a sync, so the request has to carry every tag that
        // should still exist, not just the new one.
        let mut wanted: Vec<NamedInput> = known()
            .into_iter()
            .map(|(tag, existing)| NamedInput::existing(tag, existing))
            .collect();
        wanted.push(NamedInput::new_named(&name));
        let tags_before = attached();

        spawn(async move {
            let request = PreferencesRequest { tags: Some(wanted), ..Default::default() };
            if !store.save_preferences(request, "").await {
                return;
            }
            // Read the ID the server gave it. The borrow is scoped so it is
            // released before the next await.
            let created = {
                let ledger = store.ledger.read();
                ledger.as_ref().and_then(|state| {
                    state
                        .live_tags()
                        .find(|tag| tag.name.eq_ignore_ascii_case(&name))
                        .map(|tag| tag.id)
                })
            };
            match created {
                Some(tag) => {
                    let mut tags = tags_before;
                    tags.push(tag);
                    store
                        .patch_transaction(
                            id,
                            api::TransactionPatch { tags: Some(tags), ..Default::default() },
                        )
                        .await;
                }
                None => store.fail(crate::api::ApiFailure {
                    status: 0,
                    message: format!("The tag {name:?} was saved but could not be found again."),
                }),
            }
            on_close.call(());
        });
    };

    rsx! {
        div { class: "tagpicker", role: "group", "aria-label": "Add a tag",
            if available.is_empty() {
                span { class: "muted small", "Every tag you have is already on this entry." }
            } else {
                div { class: "tagpicker-list",
                    for (tag, name) in available.iter() {
                        button {
                            key: "{tag}",
                            class: "pill pill-choice",
                            r#type: "button",
                            disabled: busy,
                            onclick: {
                                let tag = *tag;
                                move |_| attach(tag)
                            },
                            "{name}"
                        }
                    }
                }
            }

            form {
                class: "tagpicker-new",
                onsubmit: move |event| {
                    event.prevent_default();
                    create();
                },
                input {
                    class: "field-input",
                    r#type: "text",
                    placeholder: "New tag name",
                    value: "{typed}",
                    disabled: busy,
                    oninput: move |event| typed.set(event.value()),
                }
                button {
                    class: "button button-primary button-small",
                    r#type: "submit",
                    disabled: busy || typed().trim().is_empty(),
                    "Add"
                }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    "Close"
                }
            }
        }
    }
}

/// Where a transaction came from.
///
/// `sms` is included because the Android app syncs rows with that source; the
/// web app never creates one.
#[component]
fn SourceBadge(source: TransactionSource) -> Element {
    let (label, title) = match source {
        TransactionSource::Manual => ("Manual", "Entered by hand"),
        TransactionSource::Csv => ("CSV", "Imported from a statement"),
        TransactionSource::Document => ("Doc", "Extracted from an uploaded document"),
        TransactionSource::Sms => ("SMS", "Captured from a bank message by the phone app"),
    };

    rsx! {
        span { class: "badge badge-{source.as_str()}", title: title, "{label}" }
    }
}

fn parse_category(value: &str) -> CategoryChoice {
    match value {
        "any" => CategoryChoice::Any,
        "none" => CategoryChoice::None,
        other => Uuid::parse_str(other).map(CategoryChoice::One).unwrap_or(CategoryChoice::Any),
    }
}

fn parse_account(value: &str) -> AccountChoice {
    match value {
        "any" => AccountChoice::Any,
        "none" => AccountChoice::None,
        other => Uuid::parse_str(other).map(AccountChoice::One).unwrap_or(AccountChoice::Any),
    }
}

/// An empty `<option>` value means "none", not "unchanged".
fn parse_optional_uuid(value: &str) -> Option<Uuid> {
    if value.is_empty() {
        return None;
    }
    Uuid::parse_str(value).ok()
}
