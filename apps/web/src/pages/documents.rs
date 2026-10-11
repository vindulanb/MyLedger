//! The Documents screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn Documents() -> Element {
    rsx! {
        Pending {
            title: "Documents",
            blurb: "Receipts, invoices and statements, stored on your own server.",
        }
    }
}
