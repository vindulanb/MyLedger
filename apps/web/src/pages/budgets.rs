//! The Budgets screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Budgets() -> Element {
    rsx! {
        Pending {
            title: "Budgets",
            blurb: "Monthly spending limits per category, measured against real spending.",
        }
    }
}
