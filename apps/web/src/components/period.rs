//! The period selector, shared by every screen that shows totals.
//!
//! There is one of these, reading and writing the store's period, which is
//! itself backed by the server's `selected_period`. That is what makes the
//! Dashboard and the Transactions list agree: neither holds its own copy to
//! fall out of step with the other.

use dioxus::prelude::*;
use myledger_core::Period;

use crate::store::Store;

#[component]
pub fn PeriodSelector() -> Element {
    let store = Store::use_store();
    let current = store.period();
    // Disabled while a change is in flight, so a second click can't race the
    // first and leave the selector showing a period that was never saved.
    let waiting = store.pending_period.read().is_some();

    rsx! {
        div { class: "periodbar", role: "group", "aria-label": "Period",
            for period in Period::all() {
                button {
                    key: "{period.as_str()}",
                    class: if period == current { "chip is-active" } else { "chip" },
                    r#type: "button",
                    disabled: waiting,
                    // `aria-pressed` rather than `aria-selected`: these are
                    // toggle buttons, not tabs, and a screen reader should
                    // announce which one is on.
                    "aria-pressed": if period == current { "true" } else { "false" },
                    onclick: move |_| store.set_period(period),
                    "{period.label()}"
                }
            }
        }
    }
}

/// A sentence naming the period, for use under a heading.
pub fn period_blurb(period: Period) -> String {
    match period {
        Period::AllTime => "Everything recorded so far.".to_string(),
        Period::ThisMonth => "This calendar month.".to_string(),
        Period::LastMonth => "The whole of last month.".to_string(),
        Period::LastThreeMonths => "This month and the two before it.".to_string(),
        Period::LastSixMonths => "This month and the five before it.".to_string(),
        Period::ThisYear => "January to now.".to_string(),
    }
}
