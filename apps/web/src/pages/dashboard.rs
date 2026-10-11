//! The Dashboard screen.
//!
//! The real one — period selector, summary cards, charts — arrives with the
//! dashboard step. What's here now is deliberately only *facts already in the
//! ledger*: counts of what the backend returned. It earns its place by proving
//! the data layer is live, and it states nothing it hasn't been told.

use dioxus::prelude::*;

use crate::Route;
use crate::store::{Load, Store};

#[component]
pub fn Dashboard() -> Element {
    let store = Store::use_store();
    let ledger = store.ledger.read();

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

    // Counts, not analysis. Net worth, income, spending and the charts need
    // the period selector that the next step brings; inventing them from
    // "all time" now would mean rewriting them immediately.
    let facts = [
        ("Transactions", state.transactions.len()),
        ("Categories", state.live_categories().count()),
        ("Accounts", state.live_accounts().count()),
        ("Tags", state.live_tags().count()),
        ("Budgets", state.budgets.len()),
        ("Goals", state.goals.len()),
        ("Recurring & subscriptions", state.recurring_items.len()),
        ("Documents", state.documents.len()),
    ];

    rsx! {
        section { class: "stack",
            if state.transactions.is_empty() {
                div { class: "card empty",
                    h2 { class: "card-title", "Your ledger is empty" }
                    p { class: "muted",
                        "Nothing has been recorded yet. Add an entry from the top bar, or import a statement."
                    }
                }
            }

            div { class: "card",
                h2 { class: "card-title", "What's in the ledger" }
                dl { class: "factgrid",
                    for (label, count) in facts {
                        div { class: "fact", key: "{label}",
                            dt { class: "fact-label", "{label}" }
                            dd { class: "fact-value", "{count}" }
                        }
                    }
                }
                if state.transactions_truncated {
                    p { class: "muted small",
                        "Showing the most recent 5,000 transactions; the ledger holds more."
                    }
                }
            }

            div { class: "card",
                h2 { class: "card-title", "Coming up" }
                p { class: "muted",
                    "Summary cards, the cash-flow chart and spending by category arrive with the dashboard build."
                }
                Link { to: Route::Settings {}, class: "button button-quiet", "Open settings" }
            }
        }
    }
}
