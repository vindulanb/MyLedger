//! The Dashboard screen.
//!
//! Every figure on this page comes out of a function in `core::analysis`,
//! which has tests that run on the host. That split is the whole design: this
//! file decides what to show and the shared crate decides what the numbers
//! are, because a `#[test]` written here would compile for `wasm32` and never
//! run.
//!
//! Two things this page deliberately does not do:
//!
//! - It never shows a trend arrow or a comparison unless both the current and
//!   the previous period genuinely hold transactions. One real data point and
//!   one assumed zero makes "up 100%", which is a fabrication dressed as
//!   analysis.
//! - It never offers advice. The single insight line is a count of rows that
//!   need a category — a fact, checkable against the Transactions screen.

use dioxus::prelude::*;
use myledger_core::{
    Period, StateResponse, Transaction, TransactionKind, analysis, money, signed_money,
};

use crate::Route;
use crate::components::{
    CashFlowChart, EmptyState, PeriodSelector, SpendingDonut, SummaryCard, period_blurb,
};
use crate::store::{Load, Store, today};

/// The spec's cap on the cash-flow chart.
const CHART_MONTHS: usize = 7;

/// How many recent transactions the dashboard lists.
const RECENT_COUNT: usize = 5;

/// How far ahead "Coming up" looks, in days.
const COMING_UP_DAYS: i64 = 30;

#[component]
pub fn Dashboard() -> Element {
    let store = Store::use_store();
    let ledger = store.ledger.read();

    // Nothing date-dependent renders until the saved period has arrived with
    // the ledger, so the page cannot flash totals for the wrong period.
    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Dashboard" }
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

    let period = store.period();
    let today = today();
    let range = period.range(&today);

    let summary = analysis::summarise(&state.transactions, &range);
    let flow = analysis::monthly_flow(&state.transactions, &range, &today, CHART_MONTHS);
    let slices = analysis::spending_by_category(&state.transactions, &range, state);
    let recent = analysis::most_recent(&state.transactions, &range, RECENT_COUNT);
    let review_count = analysis::needs_review_count(&state.transactions, &range, state);

    rsx! {
        section { class: "stack",
            header { class: "pagehead",
                div {
                    PeriodSelector {}
                    p { class: "muted small", "{period_blurb(period)}" }
                }
            }

            SummaryCards { state: state.clone(), period, today: today.clone() }

            if summary.is_empty() {
                EmptyState {
                    title: "Nothing recorded in this period",
                    blurb: if period == Period::AllTime {
                        "Add an entry from the top bar, or import a bank statement, and this page fills in."
                    } else {
                        "Try a wider period, or add an entry from the top bar."
                    },
                }
            }

            div { class: "duo",
                div { class: "card",
                    h2 { class: "card-title", "Cash flow" }
                    p { class: "muted small", "Money in against money out, by month." }
                    CashFlowChart { points: flow }
                }

                div { class: "card",
                    h2 { class: "card-title", "Spending by category" }
                    p { class: "muted small", "Expenses only, so income doesn't distort the split." }
                    SpendingDonut { slices, total_cents: summary.spending_cents }
                }
            }

            // One line, and it is a count of real rows. See the module note.
            div { class: "card insight",
                h2 { class: "card-title", "Worth a look" }
                if review_count == 0 {
                    p { class: "muted",
                        if summary.is_empty() {
                            "Nothing to review — there are no transactions in this period."
                        } else {
                            "Every transaction in this period has a category."
                        }
                    }
                } else {
                    p {
                        "{review_count} "
                        if review_count == 1 { "transaction needs" } else { "transactions need" }
                        " a category."
                        " "
                        Link { to: Route::Transactions {}, "Open the ledger" }
                        "."
                    }
                }
            }

            RecentTransactions { state: state.clone(), recent: recent.into_iter().cloned().collect::<Vec<_>>() }

            ComingUp { state: state.clone(), today: today.clone() }
        }
    }
}

/// The four headline figures.
#[component]
fn SummaryCards(state: StateResponse, period: Period, today: String) -> Element {
    let range = period.range(&today);
    let summary = analysis::summarise(&state.transactions, &range);

    // A comparison is shown only when there is a previous period *and* it has
    // transactions in it. Otherwise there is nothing honest to say, so
    // nothing is said.
    let comparison = analysis::previous_range(period, &today)
        .map(|previous| analysis::summarise(&state.transactions, &previous))
        .filter(|previous| !previous.is_empty());

    let net_worth = if state.settings.net_worth_configured {
        Some(state.settings.assets_cents - state.settings.liabilities_cents)
    } else {
        None
    };

    rsx! {
        div { class: "summaries",
            div { class: if net_worth.is_some() { "summary" } else { "summary summary-muted" },
                span { class: "summary-label", "Net worth" }
                match net_worth {
                    Some(cents) => rsx! {
                        strong { class: "summary-value", "{money(cents)}" }
                        span { class: "summary-detail",
                            "{money(state.settings.assets_cents)} assets − {money(state.settings.liabilities_cents)} liabilities"
                        }
                    },
                    // Not zero, and not a guess from income minus expenses:
                    // net worth is a figure only the user can supply.
                    None => rsx! {
                        strong { class: "summary-value summary-unset", "Not set" }
                        span { class: "summary-detail",
                            Link { to: Route::Settings {}, "Set it up in Settings" }
                        }
                    },
                }
            }

            SummaryCard {
                label: "Income",
                value: money(summary.income_cents),
                detail: comparison.map(|previous| change_line(summary.income_cents, previous.income_cents)),
            }

            SummaryCard {
                label: "Spending",
                value: money(summary.spending_cents),
                detail: comparison.map(|previous| change_line(summary.spending_cents, previous.spending_cents)),
            }

            SummaryCard {
                label: "Savings rate",
                value: format!("{}%", summary.savings_rate_percent()),
                detail: Some(if summary.income_cents == 0 {
                    "No income in this period.".to_string()
                } else {
                    format!("{} kept of {}", money(summary.net_cents()), money(summary.income_cents))
                }),
            }
        }
    }
}

/// How a figure compares with the same-length period before it.
///
/// Only ever called once the previous period is known to hold transactions.
fn change_line(now: i64, before: i64) -> String {
    if before == 0 {
        return "No comparable figure last period.".to_string();
    }
    let delta = now - before;
    let percent = (delta as i128 * 100 / before as i128) as i64;
    match delta {
        0 => "Unchanged from last period.".to_string(),
        d if d > 0 => format!("Up {percent}% on last period ({})", money(before)),
        _ => format!("Down {}% on last period ({})", percent.abs(), money(before)),
    }
}

/// The five most recent transactions in the period.
#[component]
fn RecentTransactions(state: StateResponse, recent: Vec<Transaction>) -> Element {
    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Recent" }
                Link { to: Route::Transactions {}, class: "button button-quiet", "See all" }
            }

            if recent.is_empty() {
                p { class: "muted", "No transactions in this period." }
            } else {
                ul { class: "minilist",
                    for transaction in recent.iter() {
                        li { class: "minirow", key: "{transaction.id}",
                            div { class: "minirow-main",
                                span { class: "minirow-merchant", "{transaction.merchant}" }
                                span { class: "minirow-meta",
                                    "{transaction.date}"
                                    if let Some(name) = transaction.category_id.and_then(|id| state.category_name(id)) {
                                        " · {name}"
                                    }
                                }
                            }
                            span {
                                class: if transaction.kind == TransactionKind::Income {
                                    "amount amount-in"
                                } else {
                                    "amount amount-out"
                                },
                                "{signed_money(transaction.amount_cents, transaction.kind == TransactionKind::Income)}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Confirmed recurring charges and subscriptions due in the next month.
#[component]
fn ComingUp(state: StateResponse, today: String) -> Element {
    let horizon = add_days(&today, COMING_UP_DAYS);

    let mut due: Vec<&myledger_core::RecurringItem> = state
        .recurring_items
        .iter()
        .filter(|item| item.deleted_at.is_none() && item.active)
        .filter(|item| match &item.next_date {
            // Dates are `YYYY-MM-DD`, so a string comparison is a date
            // comparison. An item already overdue still counts as coming up —
            // it hasn't been paid.
            Some(date) => horizon.as_deref().is_none_or(|limit| date.as_str() <= limit),
            None => false,
        })
        .collect();
    due.sort_by(|a, b| a.next_date.cmp(&b.next_date));

    let total: i64 = due.iter().map(|item| item.amount_cents).sum();

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Coming up" }
                Link { to: Route::Recurring {}, class: "button button-quiet", "Recurring" }
            }

            if due.is_empty() {
                EmptyState {
                    title: "Nothing due in the next 30 days",
                    blurb: "Confirmed recurring charges and subscriptions appear here with their next expected date.",
                    Link { to: Route::Recurring {}, class: "button button-quiet", "Review detected patterns" }
                }
            } else {
                p { class: "muted small", "{money(total)} expected over the next 30 days." }
                ul { class: "minilist",
                    for item in due.iter() {
                        li { class: "minirow", key: "{item.id}",
                            div { class: "minirow-main",
                                span { class: "minirow-merchant", "{item.name}" }
                                span { class: "minirow-meta",
                                    "{item.cadence.as_str()}"
                                    if let Some(date) = &item.next_date {
                                        " · due {date}"
                                    }
                                }
                            }
                            span { class: "amount amount-out", "{money(item.amount_cents)}" }
                        }
                    }
                }
            }
        }
    }
}

/// `YYYY-MM-DD`, `days` later.
///
/// Returns `None` if the date can't be read, in which case the caller treats
/// the horizon as unbounded — listing a few extra items is a better failure
/// than silently listing none.
fn add_days(date: &str, days: i64) -> Option<String> {
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(parsed.checked_add_signed(chrono::Duration::days(days))?.format("%Y-%m-%d").to_string())
}
