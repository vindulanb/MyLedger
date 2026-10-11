//! The Subscriptions screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Subscriptions() -> Element {
    rsx! {
        Pending {
            title: "Subscriptions",
            blurb: "Detected and confirmed subscriptions will be listed here.",
        }
    }
}
