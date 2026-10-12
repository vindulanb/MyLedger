//! The Budgets screen.
//!
//! Spending is measured over the **calendar month**, not the period selected
//! on the Dashboard. A monthly limit means this month; measuring it against
//! whatever period was last picked would make a budget look breached because
//! the user was looking at the year. The arithmetic is `core::budgets`, which
//! has tests that run on the host.
//!
//! Budgets persist through `PUT /api/v1/preferences`, whose `budgets` group
//! is a *sync*: the list sent is taken as the complete list. Every save
//! therefore sends every budget that should still exist, and sends no other
//! group — so nothing unrelated is disturbed.

use dioxus::prelude::*;
use myledger_core::budgets::{BudgetHealth, NEAR_LIMIT_PERCENT, budget_label, category_taken};
use myledger_core::{
    Budget, BudgetInput, Period, PreferencesRequest, StateResponse, budgets as budget_math, money,
    parse_money,
};
use uuid::Uuid;

use crate::components::EmptyState;
use crate::store::{Load, Store, today};

#[component]
pub fn Budgets() -> Element {
    let store = Store::use_store();
    let mut creating = use_signal(|| false);
    let mut adjusting = use_signal(|| false);

    let ledger = store.ledger.read();
    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Budgets" }
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

    let today = today();
    let month = Period::ThisMonth.range(&today);
    let live: Vec<Budget> =
        state.budgets.iter().filter(|b| b.deleted_at.is_none()).cloned().collect();
    let summary = budget_math::health(&state.transactions, &month, &live);

    rsx! {
        section { class: "stack",
            header { class: "pagehead",
                div {
                    p { class: "muted small",
                        "Measured over this calendar month, whatever period the Dashboard is showing."
                    }
                }
                if !live.is_empty() {
                    div { class: "pagehead-actions",
                        button {
                            class: "button button-quiet",
                            r#type: "button",
                            onclick: move |_| {
                                let open = adjusting();
                                adjusting.set(!open);
                                if !open {
                                    creating.set(false);
                                }
                            },
                            if adjusting() { "Done adjusting" } else { "Adjust budgets" }
                        }
                        button {
                            class: "button button-primary",
                            r#type: "button",
                            onclick: move |_| {
                                let open = creating();
                                creating.set(!open);
                            },
                            if creating() { "Cancel" } else { "Create budget" }
                        }
                    }
                }
            }

            if !live.is_empty() {
                HealthRing { summary }
            }

            if creating() {
                div { class: "card",
                    h2 { class: "card-title", "New budget" }
                    BudgetForm {
                        state: state.clone(),
                        existing: None,
                        on_done: move |_| creating.set(false),
                    }
                }
            }

            if live.is_empty() {
                div { class: "card",
                    EmptyState {
                        title: "No budgets yet",
                        blurb: "A budget is a monthly limit on one category — or on everything, if you'd rather watch a single number. Spending is counted from your real transactions.",
                        button {
                            class: "button button-primary",
                            r#type: "button",
                            onclick: move |_| creating.set(true),
                            "Create budget"
                        }
                    }
                }
            } else {
                div { class: "budgetlist",
                    for budget in live.iter() {
                        BudgetCard {
                            key: "{budget.id}",
                            state: state.clone(),
                            budget: budget.clone(),
                            adjusting: adjusting(),
                        }
                    }
                }
            }
        }
    }
}

/// The budget-health summary.
#[component]
fn HealthRing(summary: BudgetHealth) -> Element {
    // A ring drawn as a dashed circle, the same trick as the dashboard's
    // donut: one arc whose dash length is the share used.
    const RADIUS: f64 = 54.0;
    let circumference = 2.0 * std::f64::consts::PI * RADIUS;
    let used = circumference * (summary.bar_percent() as f64 / 100.0);

    let tone = if summary.over_count > 0 {
        "is-over"
    } else if summary.near_count > 0 {
        "is-near"
    } else {
        "is-fine"
    };

    rsx! {
        div { class: "card healthcard",
            div { class: "ringwrap",
                svg {
                    class: "ring ring-{tone}",
                    view_box: "0 0 140 140",
                    role: "img",
                    "aria-label": "{summary.percent_used()}% of {money(summary.total_limit_cents)} budgeted has been spent.",

                    g { transform: "rotate(-90 70 70)",
                        circle { class: "ring-track", cx: "70", cy: "70", r: "{RADIUS}", fill: "none" }
                        circle {
                            class: "ring-arc",
                            cx: "70",
                            cy: "70",
                            r: "{RADIUS}",
                            fill: "none",
                            stroke_dasharray: "{used} {circumference}",
                        }
                    }
                    text {
                        class: "ring-value",
                        x: "70",
                        y: "68",
                        text_anchor: "middle",
                        "aria-hidden": "true",
                        "{summary.percent_used()}%"
                    }
                    text {
                        class: "ring-caption",
                        x: "70",
                        y: "88",
                        text_anchor: "middle",
                        "aria-hidden": "true",
                        "of budget"
                    }
                }
            }

            div { class: "healthfacts",
                h2 { class: "card-title", "Budget health" }
                dl { class: "factgrid factgrid-tight",
                    div { class: "fact",
                        dt { class: "fact-label", "Budgeted" }
                        dd { class: "fact-value small-value", "{money(summary.total_limit_cents)}" }
                    }
                    div { class: "fact",
                        dt { class: "fact-label", "Spent" }
                        dd { class: "fact-value small-value", "{money(summary.total_spent_cents)}" }
                    }
                    div { class: "fact",
                        dt { class: "fact-label",
                            if summary.remaining_cents() < 0 { "Over by" } else { "Left" }
                        }
                        dd { class: "fact-value small-value",
                            "{money(summary.remaining_cents().abs())}"
                        }
                    }
                }
                p { class: "muted small",
                    if summary.over_count > 0 {
                        "{summary.over_count} of {summary.budgets} over the limit."
                    } else if summary.near_count > 0 {
                        "{summary.near_count} of {summary.budgets} past {NEAR_LIMIT_PERCENT}% of the limit."
                    } else {
                        "All {summary.budgets} within their limits."
                    }
                }
                if summary.budgets == 0 {
                    p { class: "muted small", "No active budgets are being measured." }
                }
            }
        }
    }
}

/// One budget, with its progress.
#[component]
fn BudgetCard(state: StateResponse, budget: Budget, adjusting: bool) -> Element {
    let store = Store::use_store();
    let mut editing = use_signal(|| false);
    let busy = *store.saving.read();

    let today = today();
    let month = Period::ThisMonth.range(&today);
    let standing = budget_math::progress(&state.transactions, &month, &budget);
    let name = budget_label(&budget, budget.category_id.and_then(|id| state.category_name(id)));

    let tone = if standing.is_over() {
        "is-over"
    } else if standing.percent_used() >= NEAR_LIMIT_PERCENT {
        "is-near"
    } else {
        "is-fine"
    };

    rsx! {
        div { class: "card budgetcard",
            header { class: "card-head",
                div {
                    h3 { class: "budget-name",
                        "{name}"
                        if !budget.active {
                            span { class: "badge", title: "Not measured or counted", "Paused" }
                        }
                    }
                    span { class: "muted small",
                        "{money(standing.spent_cents)} of {money(standing.limit_cents)}"
                        if standing.counted == 1 {
                            " · 1 transaction"
                        } else {
                            " · {standing.counted} transactions"
                        }
                    }
                }
                span { class: "budget-remaining {tone}",
                    if standing.is_over() {
                        "{money(standing.over_by_cents())} over"
                    } else {
                        "{money(standing.remaining_cents())} left"
                    }
                }
            }

            div {
                class: "bar",
                role: "progressbar",
                "aria-valuenow": "{standing.bar_percent()}",
                "aria-valuemin": "0",
                "aria-valuemax": "100",
                "aria-label": "{name}: {standing.percent_used()}% of the limit spent",
                div { class: "bar-fill {tone}", style: "width: {standing.bar_percent()}%" }
            }
            span { class: "muted small", "{standing.percent_used()}% used" }

            if adjusting {
                div { class: "rowactions",
                    button {
                        class: "button button-quiet button-small",
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| {
                            let open = editing();
                            editing.set(!open);
                        },
                        if editing() { "Close" } else { "Edit" }
                    }
                    button {
                        class: "button button-quiet button-small",
                        r#type: "button",
                        disabled: busy,
                        onclick: {
                            let state = state.clone();
                            let id = budget.id;
                            move |_| {
                                // A sync: send every budget except this one.
                                let remaining = surviving_budgets(&state, Some(id), None);
                                spawn(async move {
                                    let request = PreferencesRequest {
                                        budgets: Some(remaining),
                                        ..Default::default()
                                    };
                                    store.save_preferences(request, "Budget deleted.").await;
                                });
                            }
                        },
                        "Delete"
                    }
                }
            }

            if editing() {
                BudgetForm {
                    state: state.clone(),
                    existing: Some(budget.clone()),
                    on_done: move |_| editing.set(false),
                }
            }
        }
    }
}

/// Creating or editing a budget.
#[component]
fn BudgetForm(
    state: StateResponse,
    existing: Option<Budget>,
    on_done: EventHandler<()>,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let editing_id = existing.as_ref().map(|b| b.id);

    let mut category = use_signal(|| existing.as_ref().and_then(|b| b.category_id));
    let mut limit = use_signal(|| {
        existing
            .as_ref()
            .map(|b| format!("{}.{:02}", b.monthly_limit_cents / 100, b.monthly_limit_cents % 100))
            .unwrap_or_default()
    });
    let mut active = use_signal(|| existing.as_ref().map(|b| b.active).unwrap_or(true));
    let mut error = use_signal(|| None::<String>);

    let submit = {
        let state = state.clone();
        move |_| {
            let Some(cents) = parse_money(&limit()) else {
                error.set(Some("Enter a monthly limit, for example 250.00.".to_string()));
                return;
            };
            if cents <= 0 {
                error.set(Some("A limit has to be more than nothing.".to_string()));
                return;
            }
            // Two budgets on one category would both measure the same
            // spending and both look breached.
            if category_taken(&state.budgets, category(), editing_id) {
                error.set(Some(match category() {
                    None => "There is already a budget for all spending.".to_string(),
                    Some(_) => "That category already has a budget. Edit that one instead."
                        .to_string(),
                }));
                return;
            }
            error.set(None);

            let mut wanted = surviving_budgets(&state, editing_id, None);
            wanted.push(BudgetInput {
                id: editing_id,
                category_id: category(),
                monthly_limit_cents: cents,
                active: active(),
            });

            let done = if editing_id.is_some() { "Budget saved." } else { "Budget created." };
            spawn(async move {
                let request =
                    PreferencesRequest { budgets: Some(wanted), ..Default::default() };
                if store.save_preferences(request, done).await {
                    on_done.call(());
                }
            });
        }
    };

    rsx! {
        div { class: "ruleform",
            div { class: "formgrid",
                label { class: "field",
                    span { class: "field-label", "Category" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| category.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", selected: category().is_none(), "All spending" }
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
                    span { class: "field-label", "Monthly limit" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "budget-limit",
                        inputmode: "decimal",
                        placeholder: "0.00",
                        value: "{limit}",
                        disabled: busy,
                        oninput: move |event| limit.set(event.value()),
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
                span { "Measure this budget" }
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
                    if editing_id.is_some() { "Save changes" } else { "Create budget" }
                }
            }
        }
    }
}

/// Every budget that should still exist, as the sync endpoint wants them.
///
/// `PUT /preferences` treats a present group as the complete list, so a save
/// has to resend everything being kept. `without` drops one — that is how a
/// delete and an edit are both expressed.
fn surviving_budgets(
    state: &StateResponse,
    without: Option<Uuid>,
    _reserved: Option<Uuid>,
) -> Vec<BudgetInput> {
    state
        .budgets
        .iter()
        .filter(|budget| budget.deleted_at.is_none())
        .filter(|budget| Some(budget.id) != without)
        .map(|budget| BudgetInput {
            id: Some(budget.id),
            category_id: budget.category_id,
            monthly_limit_cents: budget.monthly_limit_cents,
            active: budget.active,
        })
        .collect()
}
