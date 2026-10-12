//! The Goals screen.
//!
//! A goal is a target amount and how much of it has been put aside. MyLedger
//! does not try to work the second number out from the transactions: money
//! earmarked for a new boiler looks exactly like money sitting in the current
//! account, and guessing would produce a progress bar that moves on its own
//! for reasons the user cannot see. The amount saved is theirs to state.
//!
//! Goals persist through `PUT /api/v1/preferences`, whose `goals` group is a
//! *sync*: the list sent is taken as the complete list. Every save therefore
//! resends every goal being kept, and sends no other group, so nothing
//! unrelated is disturbed.

use dioxus::prelude::*;
use myledger_core::budgets::goal_progress;
use myledger_core::{Goal, GoalInput, PreferencesRequest, StateResponse, money, parse_money};
use uuid::Uuid;

use crate::components::EmptyState;
use crate::store::{Load, Store, today};

#[component]
pub fn Goals() -> Element {
    let store = Store::use_store();
    let mut creating = use_signal(|| false);
    let mut editing = use_signal(|| None::<Uuid>);

    let ledger = store.ledger.read();
    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Goals" }
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

    let live: Vec<Goal> = state.goals.iter().filter(|g| g.deleted_at.is_none()).cloned().collect();
    let total_target: i64 = live.iter().map(|g| g.target_cents).sum();
    let total_saved: i64 = live.iter().map(|g| g.current_cents.max(0)).sum();

    rsx! {
        section { class: "stack",
            header { class: "pagehead",
                div {
                    p { class: "muted small",
                        "What you're saving towards, and how far along each one is."
                    }
                }
                if !live.is_empty() {
                    button {
                        class: "button button-primary",
                        r#type: "button",
                        onclick: move |_| {
                            let open = creating();
                            creating.set(!open);
                            editing.set(None);
                        },
                        if creating() { "Cancel" } else { "Create goal" }
                    }
                }
            }

            if !live.is_empty() {
                div { class: "card",
                    h2 { class: "card-title", "Altogether" }
                    div { class: "summaries",
                        div { class: "summary",
                            span { class: "summary-label", "Saved" }
                            strong { class: "summary-value", "{money(total_saved)}" }
                            span { class: "summary-detail", "across {live.len()} goals" }
                        }
                        div { class: "summary",
                            span { class: "summary-label", "Targets" }
                            strong { class: "summary-value", "{money(total_target)}" }
                        }
                        div { class: "summary summary-muted",
                            span { class: "summary-label", "Still to find" }
                            strong { class: "summary-value",
                                "{money((total_target - total_saved).max(0))}"
                            }
                        }
                    }
                }
            }

            if creating() {
                div { class: "card",
                    h2 { class: "card-title", "New goal" }
                    GoalForm {
                        state: state.clone(),
                        existing: None,
                        on_done: move |_| creating.set(false),
                    }
                }
            }

            if live.is_empty() {
                div { class: "card",
                    EmptyState {
                        title: "No goals yet",
                        blurb: "A goal is something you're putting money aside for — a deposit, a holiday, a new boiler. You say how much is saved; MyLedger doesn't guess, because earmarked money looks like any other balance.",
                        button {
                            class: "button button-primary",
                            r#type: "button",
                            onclick: move |_| creating.set(true),
                            "Create goal"
                        }
                    }
                }
            } else {
                div { class: "budgetlist",
                    for goal in live.iter() {
                        GoalCard {
                            key: "{goal.id}",
                            state: state.clone(),
                            goal: goal.clone(),
                            editing,
                        }
                    }
                }
            }
        }
    }
}

/// One goal, with its progress.
#[component]
fn GoalCard(state: StateResponse, goal: Goal, editing: Signal<Option<Uuid>>) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let standing = goal_progress(&goal);
    let id = goal.id;

    let overdue = goal
        .due_date
        .as_deref()
        .is_some_and(|due| !standing.is_reached() && due < today().as_str());

    rsx! {
        div { class: "card budgetcard",
            header { class: "card-head",
                div {
                    h3 { class: "budget-name",
                        "{goal.name}"
                        if standing.is_reached() {
                            span { class: "badge badge-confidence-high", title: "Target reached",
                                "Reached"
                            }
                        }
                    }
                    span { class: "muted small",
                        "{money(standing.current_cents)} of {money(standing.target_cents)}"
                        if let Some(due) = &goal.due_date {
                            if overdue {
                                " · was due {due}"
                            } else {
                                " · by {due}"
                            }
                        }
                    }
                }
                span {
                    class: if standing.is_reached() { "budget-remaining is-fine" } else { "budget-remaining" },
                    if standing.is_reached() {
                        "Done"
                    } else {
                        "{money(standing.remaining_cents())} to go"
                    }
                }
            }

            div {
                class: "bar",
                role: "progressbar",
                "aria-valuenow": "{standing.bar_percent()}",
                "aria-valuemin": "0",
                "aria-valuemax": "100",
                "aria-label": "{goal.name}: {standing.percent_saved()}% saved",
                div {
                    class: if standing.is_reached() { "bar-fill is-fine" } else { "bar-fill" },
                    style: "width: {standing.bar_percent()}%",
                }
            }
            span { class: "muted small", "{standing.percent_saved()}% saved" }

            if let Some(note) = &goal.note {
                p { class: "muted small goalnote", "{note}" }
            }

            div { class: "rowactions",
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| {
                        if editing() == Some(id) {
                            editing.set(None);
                        } else {
                            editing.set(Some(id));
                        }
                    },
                    if editing() == Some(id) { "Close" } else { "Edit" }
                }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: {
                        let state = state.clone();
                        move |_| {
                            let remaining = surviving_goals(&state, Some(id));
                            spawn(async move {
                                let request = PreferencesRequest {
                                    goals: Some(remaining),
                                    ..Default::default()
                                };
                                store.save_preferences(request, "Goal deleted.").await;
                            });
                        }
                    },
                    "Delete"
                }
            }

            if editing() == Some(id) {
                GoalForm {
                    state: state.clone(),
                    existing: Some(goal.clone()),
                    on_done: move |_| editing.set(None),
                }
            }
        }
    }
}

/// Creating or editing a goal.
#[component]
fn GoalForm(state: StateResponse, existing: Option<Goal>, on_done: EventHandler<()>) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let editing_id = existing.as_ref().map(|g| g.id);

    let mut name = use_signal(|| existing.as_ref().map(|g| g.name.clone()).unwrap_or_default());
    let mut target = use_signal(|| {
        existing
            .as_ref()
            .map(|g| format!("{}.{:02}", g.target_cents / 100, g.target_cents % 100))
            .unwrap_or_default()
    });
    let mut current = use_signal(|| {
        existing
            .as_ref()
            .map(|g| format!("{}.{:02}", g.current_cents / 100, g.current_cents % 100))
            .unwrap_or_else(|| "0.00".to_string())
    });
    let mut due = use_signal(|| existing.as_ref().and_then(|g| g.due_date.clone()).unwrap_or_default());
    let mut note = use_signal(|| existing.as_ref().and_then(|g| g.note.clone()).unwrap_or_default());
    let mut error = use_signal(|| None::<String>);

    let submit = {
        let state = state.clone();
        move |_| {
            let label = name().trim().to_string();
            if label.is_empty() {
                error.set(Some("Give the goal a name.".to_string()));
                return;
            }
            let Some(target_cents) = parse_money(&target()) else {
                error.set(Some("Enter a target amount, for example 2500.00.".to_string()));
                return;
            };
            if target_cents <= 0 {
                error.set(Some("A target has to be more than nothing.".to_string()));
                return;
            }
            let Some(current_cents) = parse_money(&current()) else {
                error.set(Some("Enter how much is saved so far, or 0.".to_string()));
                return;
            };
            error.set(None);

            let mut wanted = surviving_goals(&state, editing_id);
            wanted.push(GoalInput {
                id: editing_id,
                name: label,
                target_cents,
                current_cents,
                due_date: blank(due()),
                note: blank(note()),
            });

            let done = if editing_id.is_some() { "Goal saved." } else { "Goal created." };
            spawn(async move {
                let request = PreferencesRequest { goals: Some(wanted), ..Default::default() };
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
                    span { class: "field-label", "Name" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "goal-name",
                        placeholder: "What you're saving for",
                        value: "{name}",
                        disabled: busy,
                        oninput: move |event| name.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Target" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "goal-target",
                        inputmode: "decimal",
                        placeholder: "0.00",
                        value: "{target}",
                        disabled: busy,
                        oninput: move |event| target.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Saved so far" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "goal-current",
                        inputmode: "decimal",
                        placeholder: "0.00",
                        value: "{current}",
                        disabled: busy,
                        oninput: move |event| current.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Due by (optional)" }
                    input {
                        class: "field-input",
                        r#type: "date",
                        name: "goal-due",
                        value: "{due}",
                        disabled: busy,
                        oninput: move |event| due.set(event.value()),
                    }
                }

                label { class: "field field-wide",
                    span { class: "field-label", "Note (optional)" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "goal-note",
                        placeholder: "Anything worth remembering",
                        value: "{note}",
                        disabled: busy,
                        oninput: move |event| note.set(event.value()),
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
                    if editing_id.is_some() { "Save changes" } else { "Create goal" }
                }
            }
        }
    }
}

/// Every goal that should still exist, as the sync endpoint wants them.
///
/// `PUT /preferences` treats a present group as the complete list, so a save
/// resends everything being kept. `without` drops one, which is how a delete
/// and an edit are both expressed.
fn surviving_goals(state: &StateResponse, without: Option<Uuid>) -> Vec<GoalInput> {
    state
        .goals
        .iter()
        .filter(|goal| goal.deleted_at.is_none())
        .filter(|goal| Some(goal.id) != without)
        .map(|goal| GoalInput {
            id: Some(goal.id),
            name: goal.name.clone(),
            target_cents: goal.target_cents,
            current_cents: goal.current_cents,
            due_date: goal.due_date.clone(),
            note: goal.note.clone(),
        })
        .collect()
}

fn blank(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
