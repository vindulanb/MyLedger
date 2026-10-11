//! The Transactions screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Transactions() -> Element {
    rsx! {
        Pending {
            title: "Transactions",
            blurb: "The ledger, with search, filters and inline editing, arrives next.",
        }
    }
}
