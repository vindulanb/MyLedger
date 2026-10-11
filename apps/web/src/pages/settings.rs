//! The Settings screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Settings() -> Element {
    rsx! {
        Pending {
            title: "Settings",
            blurb: "Net worth, managed lists, device pairing and the danger zone.",
        }
    }
}
