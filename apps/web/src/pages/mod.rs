//! The screens.
//!
//! Each page gets its own module from the start, so that building one out later
//! is an edit to that file rather than a split of a shared one. For now most of
//! them render [`Pending`], which says plainly that the screen isn't built
//! instead of showing an empty frame that looks broken.

mod budgets;
mod dashboard;
mod documents;
mod goals;
mod repeating;
mod rules;
mod settings;
mod transactions;

pub use budgets::Budgets;
pub use dashboard::Dashboard;
pub use documents::Documents;
pub use goals::Goals;
pub use repeating::{Recurring, Subscriptions};
pub use rules::RulesAndTags;
pub use settings::Settings;
pub use transactions::Transactions;

use dioxus::prelude::*;

use crate::Route;

/// Stands in for a screen that hasn't been built yet.
#[component]
pub fn Pending(title: String, blurb: String) -> Element {
    rsx! {
        section { class: "pending",
            h2 { class: "pending-title", "{title}" }
            p { class: "pending-blurb", "{blurb}" }
        }
    }
}

/// A URL that matches nothing.
#[component]
pub fn NotFound(segments: Vec<String>) -> Element {
    let path = segments.join("/");

    rsx! {
        section { class: "pending",
            h2 { class: "pending-title", "Nothing here" }
            p { class: "pending-blurb",
                if path.is_empty() {
                    "That page doesn't exist."
                } else {
                    "There's no page at /{path}."
                }
            }
            Link { to: Route::Dashboard {}, class: "button button-primary", "Back to the dashboard" }
        }
    }
}
