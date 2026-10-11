//! The sign-in screen.
//!
//! Nothing is stored here on success. The server replies with a `Set-Cookie`
//! the browser keeps for itself, which is why a refresh stays signed in without
//! this app touching `localStorage`.

use dioxus::prelude::*;

use crate::api;
use crate::store::Store;

#[component]
pub fn Login() -> Element {
    let store = Store::use_store();

    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let mut submit = move || {
        if busy() {
            return;
        }
        let (name, secret) = (username(), password());
        if name.trim().is_empty() || secret.is_empty() {
            error.set(Some("Enter your username and password.".to_string()));
            return;
        }

        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::login(&name, &secret).await {
                Ok(response) => {
                    // No `busy.set(false)` — this component is about to be
                    // replaced by the app shell.
                    store.signed_in(response.username);
                }
                Err(failure) => {
                    error.set(Some(failure.message));
                    password.set(String::new());
                    busy.set(false);
                }
            }
        });
    };

    rsx! {
        main { class: "login",
            form {
                class: "login-card",
                onsubmit: move |event| {
                    event.prevent_default();
                    submit();
                },

                h1 { class: "login-mark", "MyLedger" }
                p { class: "login-blurb", "Sign in to your ledger." }

                label { class: "field",
                    span { class: "field-label", "Username" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        value: "{username}",
                        autocomplete: "username",
                        autofocus: true,
                        disabled: busy(),
                        oninput: move |event| username.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Password" }
                    input {
                        class: "field-input",
                        r#type: "password",
                        value: "{password}",
                        autocomplete: "current-password",
                        disabled: busy(),
                        oninput: move |event| password.set(event.value()),
                    }
                }

                if let Some(message) = error() {
                    // `role=alert` so a screen reader announces a failed
                    // sign-in rather than leaving the user waiting.
                    p { class: "login-error", role: "alert", "{message}" }
                }

                button {
                    class: "button button-primary login-submit",
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { "Signing in…" } else { "Sign in" }
                }
            }
        }
    }
}
