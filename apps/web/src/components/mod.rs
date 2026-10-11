//! Pieces used by more than one screen.
//!
//! A component earns a place here the moment a second page needs it. Keeping
//! the period selector, the toast and the charts in one place is what stops
//! two screens drifting into two slightly different versions of the same
//! control.

mod add_entry;
mod charts;
mod import;
mod period;

pub use add_entry::AddEntryForm;
pub use charts::{CashFlowChart, SpendingDonut};
pub use import::ImportForm;
pub use period::{PeriodSelector, period_blurb};

use dioxus::prelude::*;

use crate::store::Store;

/// The outcome of the last action, shown over everything else.
///
/// Errors wait to be dismissed; a success says so and can be waved away. Both
/// are announced to assistive tech, since the thing they report on — a save
/// that worked, a save that didn't — often happens with the focus somewhere
/// else entirely.
#[component]
pub fn Toast() -> Element {
    let store = Store::use_store();
    let Some(notice) = store.notice.read().clone() else {
        return rsx! {};
    };

    rsx! {
        div {
            class: if notice.is_error { "toast toast-error" } else { "toast toast-ok" },
            role: if notice.is_error { "alert" } else { "status" },
            "aria-live": if notice.is_error { "assertive" } else { "polite" },

            span { class: "toast-text", "{notice.text}" }
            button {
                class: "toast-close",
                "aria-label": "Dismiss",
                onclick: move |_| store.clear_notice(),
                "×"
            }
        }
    }
}

/// A headline figure.
///
/// `detail` is for the one line under the number that says what it is counted
/// from. It is never a prediction — see the note on the dashboard about why
/// nothing here invents a trend.
#[component]
pub fn SummaryCard(
    label: String,
    value: String,
    detail: Option<String>,
    #[props(default = false)] muted: bool,
) -> Element {
    rsx! {
        div { class: if muted { "summary summary-muted" } else { "summary" },
            span { class: "summary-label", "{label}" }
            strong { class: "summary-value", "{value}" }
            if let Some(detail) = detail {
                span { class: "summary-detail", "{detail}" }
            }
        }
    }
}

/// What a section shows when it has nothing to show.
///
/// Always says what to do next, because an empty panel with no explanation
/// reads as a bug rather than as an empty ledger.
#[component]
pub fn EmptyState(title: String, blurb: String, children: Element) -> Element {
    rsx! {
        div { class: "emptystate",
            p { class: "emptystate-title", "{title}" }
            p { class: "muted", "{blurb}" }
            {children}
        }
    }
}
