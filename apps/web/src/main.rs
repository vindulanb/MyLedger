//! MyLedger — the web client.
//!
//! A Dioxus single-page app, compiled to WASM and served as static files by the
//! same Axum backend it talks to. Being same-origin is what lets the session
//! live in an `HttpOnly` cookie that this code cannot read.
//!
//! The root decides one thing: which of three states the app is in. Until the
//! server has answered "who am I?", neither the login screen nor the app is
//! correct to show — so there is an explicit third state rather than a guess
//! that flashes the wrong thing.

use dioxus::prelude::*;

mod api;
mod login;
mod pages;
mod shell;
mod store;

use login::Login;
use shell::Shell;
use store::{Session, Store};

const MAIN_CSS: Asset = asset!("/assets/main.css");

/// Every screen in the app.
///
/// Paths are real URLs, not fragments, so a page can be bookmarked and
/// reloaded — which works because the server falls back to `index.html` for
/// anything that isn't `/api/v1/...`.
#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Dashboard {},
        #[route("/transactions")]
        Transactions {},
        #[route("/recurring")]
        Recurring {},
        #[route("/subscriptions")]
        Subscriptions {},
        #[route("/budgets")]
        Budgets {},
        #[route("/goals")]
        Goals {},
        #[route("/documents")]
        Documents {},
        #[route("/rules")]
        RulesAndTags {},
        #[route("/settings")]
        Settings {},
        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}

pub use pages::{
    Budgets, Dashboard, Documents, Goals, NotFound, Recurring, RulesAndTags, Settings,
    Subscriptions, Transactions,
};

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let store = Store::provide();

    // Once, on mount: ask the server whether the cookie we may or may not have
    // is still a session.
    use_hook(move || store.restore_session());

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        document::Title { "MyLedger" }
        document::Meta { name: "color-scheme", content: "light dark" }

        {
            match store.session.read().clone() {
                Session::Unknown => rsx! { Splash {} },
                Session::SignedOut => rsx! { Login {} },
                Session::SignedIn { .. } => rsx! { Router::<Route> {} },
            }
        }
    }
}

/// Shown for the moment it takes to check the session.
///
/// Deliberately almost empty: a spinner that appears for 40ms is worse than
/// nothing, so this is just the wordmark holding the space.
#[component]
fn Splash() -> Element {
    rsx! {
        div { class: "splash",
            div { class: "splash-mark", "MyLedger" }
        }
    }
}
