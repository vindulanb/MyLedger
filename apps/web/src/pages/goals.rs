//! The Goals screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Goals() -> Element {
    rsx! {
        Pending {
            title: "Goals",
            blurb: "Savings targets and how close you are to them.",
        }
    }
}
