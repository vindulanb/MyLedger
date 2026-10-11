//! The global Add-entry form.
//!
//! Lives in the shell's modal host rather than on the Transactions screen,
//! because the top bar can open it from anywhere — recording a coffee
//! shouldn't require navigating to the ledger first.
//!
//! The amount is kept as the text the user typed and parsed only on submit.
//! Parsing on every keystroke means "1." becomes "1" under the cursor, and
//! deleting the last digit of "5" leaves a stubborn 0 behind.

use dioxus::prelude::*;
use myledger_core::{
    TransactionKind, money, parse_money, validate_amount_cents, validate_date, validate_merchant,
};
use uuid::Uuid;

use crate::api;
use crate::store::{Store, today};

#[component]
pub fn AddEntryForm(on_done: EventHandler<()>) -> Element {
    let store = Store::use_store();

    let mut kind = use_signal(|| TransactionKind::Expense);
    // Both start blank, as the spec asks — not at zero, which looks like a
    // real amount already entered.
    let mut amount_text = use_signal(String::new);
    let mut merchant = use_signal(String::new);
    let mut date = use_signal(today);
    let mut category = use_signal(|| None::<Uuid>);
    let mut account = use_signal(|| None::<Uuid>);
    let mut tags = use_signal(Vec::<Uuid>::new);
    let mut wants_receipt = use_signal(|| false);
    let mut picked = use_signal(Vec::<api::Upload>::new);
    let mut error = use_signal(|| None::<String>);

    let busy = *store.saving.read();
    let ledger = store.ledger.read();
    let Some(state) = ledger.as_ref() else {
        return rsx! { p { class: "muted", "The ledger is still loading." } };
    };

    // Rendered live under the amount field, so a typo is visible before
    // submitting rather than after.
    let parsed_amount = parse_money(&amount_text());

    let mut submit = {
        let state = state.clone();
        move |_| {
            let merchant_text = merchant();
            let date_text = date();

            let Some(cents) = parse_money(&amount_text()) else {
                error.set(Some("Enter an amount, for example 12.50.".to_string()));
                return;
            };
            // The same validators the server uses, so the message the user
            // sees is the rule that will actually be applied.
            if let Err(problem) = validate_amount_cents(cents) {
                error.set(Some(problem.to_string()));
                return;
            }
            if let Err(problem) = validate_merchant(&merchant_text) {
                error.set(Some(problem.to_string()));
                return;
            }
            if let Err(problem) = validate_date(&date_text) {
                error.set(Some(problem.to_string()));
                return;
            }
            if wants_receipt() && picked().is_empty() {
                error.set(Some("Choose a receipt file, or untick the box.".to_string()));
                return;
            }

            error.set(None);

            let entry = api::NewTransaction {
                date: date_text,
                merchant: merchant_text.trim().to_string(),
                amount_cents: cents,
                kind: kind().as_str().to_string(),
                category_id: category(),
                account_id: account().or_else(|| state.live_accounts().next().map(|a| a.id)),
                tags: tags(),
                source: "manual".to_string(),
            };
            // Only sent if the box is ticked, so clearing the tick discards a
            // file picked by mistake rather than uploading it anyway.
            let receipts = if wants_receipt() { picked() } else { Vec::new() };

            spawn(async move {
                if store.add_transaction(entry, receipts).await {
                    on_done.call(());
                }
            });
        }
    };

    rsx! {
        form {
            class: "entryform",
            onsubmit: move |event| {
                event.prevent_default();
                submit(());
            },

            // Expense or income. Two radios styled as a segmented control, so
            // a screen reader gets a real group with a real selected state.
            fieldset { class: "segmented",
                legend { class: "field-label", "Kind" }
                for (option, label) in [
                    (TransactionKind::Expense, "Expense"),
                    (TransactionKind::Income, "Income"),
                ] {
                    label {
                        key: "{label}",
                        class: if kind() == option { "segment is-active" } else { "segment" },
                        input {
                            r#type: "radio",
                            name: "entry-kind",
                            value: "{option.as_str()}",
                            checked: kind() == option,
                            disabled: busy,
                            onchange: move |_| kind.set(option),
                        }
                        span { "{label}" }
                    }
                }
            }

            div { class: "formgrid",
                label { class: "field",
                    span { class: "field-label", "Amount" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        // Named, so this field is addressable as itself
                        // rather than as "the first text input".
                        name: "amount",
                        inputmode: "decimal",
                        placeholder: "0.00",
                        value: "{amount_text}",
                        disabled: busy,
                        autofocus: true,
                        oninput: move |event| amount_text.set(event.value()),
                    }
                    if let Some(cents) = parsed_amount {
                        span { class: "field-hint", "{money(cents)}" }
                    } else if !amount_text().trim().is_empty() {
                        span { class: "field-hint field-hint-bad", "Not a number" }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Date" }
                    input {
                        class: "field-input",
                        r#type: "date",
                        name: "date",
                        value: "{date}",
                        disabled: busy,
                        oninput: move |event| date.set(event.value()),
                    }
                }

                label { class: "field field-wide",
                    span { class: "field-label", "Merchant" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        name: "merchant",
                        placeholder: "Where the money went",
                        value: "{merchant}",
                        disabled: busy,
                        oninput: move |event| merchant.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Category" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| {
                            category.set(Uuid::parse_str(&event.value()).ok());
                        },
                        option { value: "", "No category" }
                        for item in state.live_categories() {
                            option { key: "{item.id}", value: "{item.id}", "{item.name}" }
                        }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Account" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| {
                            account.set(Uuid::parse_str(&event.value()).ok());
                        },
                        option { value: "", "No account" }
                        for item in state.live_accounts() {
                            option { key: "{item.id}", value: "{item.id}", "{item.name}" }
                        }
                    }
                }
            }

            if state.live_tags().next().is_some() {
                div { class: "field",
                    span { class: "field-label", "Tags" }
                    div { class: "tagcell",
                        for item in state.live_tags() {
                            button {
                                key: "{item.id}",
                                class: if tags().contains(&item.id) { "pill pill-choice is-on" } else { "pill pill-choice" },
                                r#type: "button",
                                "aria-pressed": if tags().contains(&item.id) { "true" } else { "false" },
                                disabled: busy,
                                onclick: {
                                    let id = item.id;
                                    move |_| {
                                        let mut current = tags();
                                        if let Some(at) = current.iter().position(|t| *t == id) {
                                            current.remove(at);
                                        } else {
                                            current.push(id);
                                        }
                                        tags.set(current);
                                    }
                                },
                                "{item.name}"
                            }
                        }
                    }
                }
            }

            label { class: "checkline",
                input {
                    r#type: "checkbox",
                    checked: wants_receipt(),
                    disabled: busy,
                    onchange: move |event| {
                        let on = event.checked();
                        wants_receipt.set(on);
                        if !on {
                            picked.set(Vec::new());
                        }
                    },
                }
                span { "I have a receipt to attach" }
            }

            if wants_receipt() {
                div { class: "field",
                    span { class: "field-label", "Receipt" }
                    input {
                        class: "field-input",
                        r#type: "file",
                        multiple: true,
                        accept: "image/*,application/pdf",
                        disabled: busy,
                        onchange: move |event| {
                            let files = event.files();
                            spawn(async move {
                                let mut loaded = Vec::new();
                                for file in files {
                                    match file.read_bytes().await {
                                        Ok(bytes) => loaded.push(api::Upload {
                                            filename: file.name(),
                                            mime_type: file.content_type(),
                                            bytes: bytes.to_vec(),
                                        }),
                                        Err(problem) => {
                                            error.set(Some(format!(
                                                "Could not read {}: {problem}",
                                                file.name()
                                            )));
                                        }
                                    }
                                }
                                picked.set(loaded);
                            });
                        },
                    }
                    if !picked().is_empty() {
                        span { class: "field-hint",
                            "{picked().len()} file(s) ready: "
                            {picked().iter().map(|f| f.filename.clone()).collect::<Vec<_>>().join(", ")}
                        }
                    }
                }
            }

            if let Some(message) = error() {
                p { class: "formerror", role: "alert", "{message}" }
            }

            div { class: "formactions",
                button {
                    class: "button button-quiet",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_done.call(()),
                    "Cancel"
                }
                button {
                    class: "button button-primary",
                    r#type: "submit",
                    disabled: busy,
                    if busy { "Saving…" } else { "Save entry" }
                }
            }
        }
    }
}
