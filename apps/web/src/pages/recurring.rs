//! The Recurring screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Recurring() -> Element {
    rsx! {
        Pending {
            title: "Recurring",
            blurb: "Detected and confirmed recurring bills will be listed here.",
        }
    }
}
