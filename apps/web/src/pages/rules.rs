//! The Rules & Tags screen.

use dioxus::prelude::*;

use super::Pending;

#[component]
pub fn RulesAndTags() -> Element {
    rsx! {
        Pending {
            title: "Rules & Tags",
            blurb: "Categorisation rules, tag management, and SMS parsing rules to sync to your phone.",
        }
    }
}
