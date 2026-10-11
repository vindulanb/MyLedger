//! The Documents screen.
//!
//! An upload card, the vault list, and the extraction rules — plus, per
//! document, the review flow for one that extraction could not read on its
//! own.
//!
//! Two things this screen is careful about:
//!
//! - **It never shows a storage path.** A document is fetched by ID through
//!   a download link; where the bytes sit on disk is nobody's business, and
//!   the server does not even send it.
//! - **It is honest about extraction.** There is no OCR in this build, so a
//!   photograph of a receipt yields nothing. Rather than leaving the user
//!   wondering, the screen prints the reason the server gave and offers the
//!   manual route.

use dioxus::prelude::*;
use myledger_core::textrules::Candidate;
use myledger_core::{
    Document, DocumentExtractionRule, DocumentStatus, SmsKindHint, StateResponse, Transaction,
    money, signed_money,
};
use uuid::Uuid;

use crate::api;
use crate::components::EmptyState;
use crate::store::{Load, Store};

#[component]
pub fn Documents() -> Element {
    let store = Store::use_store();
    let ledger = store.ledger.read();

    let Some(state) = ledger.as_ref() else {
        return rsx! {
            section { class: "pending",
                h2 { class: "pending-title", "Documents" }
                p { class: "pending-blurb",
                    if matches!(*store.load.read(), Load::Loading) {
                        "Loading your ledger…"
                    } else {
                        "No ledger loaded yet."
                    }
                }
            }
        };
    };

    rsx! {
        section { class: "stack",
            UploadCard {}
            VaultList { state: state.clone() }
            ExtractionRules { state: state.clone() }
        }
    }
}

/// Picking files and sending them.
#[component]
fn UploadCard() -> Element {
    let store = Store::use_store();
    let mut busy = use_signal(|| false);
    let mut problem = use_signal(|| None::<String>);

    rsx! {
        div { class: "card",
            h2 { class: "card-title", "Upload documents" }
            p { class: "muted small",
                "Receipts, invoices and statements — PDFs, images, text or CSV, up to 20 MB each and six at a time."
            }

            label { class: "field",
                span { class: "field-label", "Files" }
                input {
                    class: "field-input",
                    r#type: "file",
                    multiple: true,
                    accept: "application/pdf,image/*,text/plain,text/csv,.txt,.csv,.pdf",
                    disabled: busy(),
                    onchange: move |event| {
                        let files = event.files();
                        if files.is_empty() {
                            return;
                        }
                        busy.set(true);
                        problem.set(None);

                        spawn(async move {
                            let mut uploads = Vec::new();
                            let mut failed = None;
                            for file in files {
                                match file.read_bytes().await {
                                    Ok(bytes) => uploads.push(api::Upload {
                                        filename: file.name(),
                                        mime_type: file.content_type(),
                                        bytes: bytes.to_vec(),
                                    }),
                                    Err(trouble) => {
                                        failed = Some(format!(
                                            "Could not read {}: {trouble}",
                                            file.name()
                                        ));
                                    }
                                }
                            }

                            if let Some(message) = failed {
                                problem.set(Some(message));
                                busy.set(false);
                                return;
                            }

                            let count = uploads.len();
                            let message = if count == 1 {
                                "1 document stored.".to_string()
                            } else {
                                format!("{count} documents stored.")
                            };
                            store
                                .act(api::upload_documents(&uploads, "web-upload", None), &message)
                                .await;
                            busy.set(false);
                        });
                    },
                }
                if busy() {
                    span { class: "field-hint", "Uploading…" }
                }
            }

            if let Some(message) = problem() {
                p { class: "formerror", role: "alert", "{message}" }
            }
        }
    }
}

/// The stored documents.
#[component]
fn VaultList(state: StateResponse) -> Element {
    let documents = &state.documents;

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Document vault" }
                if !documents.is_empty() {
                    span { class: "muted small",
                        "{documents.len()} "
                        if documents.len() == 1 { "document" } else { "documents" }
                    }
                }
            }

            if documents.is_empty() {
                EmptyState {
                    // The spec's wording, verbatim.
                    title: "No documents yet.",
                    blurb: "Upload a file to get started.",
                }
            } else {
                div { class: "tablewrap",
                    table { class: "ledger",
                        thead {
                            tr {
                                th { scope: "col", "File" }
                                th { scope: "col", "Type" }
                                th { scope: "col", class: "numeric", "Size" }
                                th { scope: "col", "Source" }
                                th { scope: "col", "Status" }
                                th { scope: "col", "Uploaded" }
                                th { scope: "col", class: "right",
                                    span { class: "visually-hidden", "Actions" }
                                }
                            }
                        }
                        tbody {
                            for document in documents.iter() {
                                DocumentRow {
                                    key: "{document.id}",
                                    state: state.clone(),
                                    document: document.clone(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DocumentRow(state: StateResponse, document: Document) -> Element {
    let store = Store::use_store();
    let id = document.id;
    let busy = *store.saving.read();

    // The review panel for this document, when it is open.
    let mut reviewing = use_signal(|| false);

    let linked = document
        .linked_transaction_id
        .and_then(|transaction_id| {
            state.transactions.iter().find(|t| t.id == transaction_id).cloned()
        });

    rsx! {
        tr {
            td { class: "cell-merchant", "data-label": "File",
                // A download link, not a path. The server streams the bytes
                // and names the file; the on-disk layout is never shown.
                a {
                    class: "filelink",
                    href: "{api::document_download_url(id)}",
                    download: "{document.filename}",
                    "{document.filename}"
                }
                if let Some(transaction) = &linked {
                    div { class: "rowmarks",
                        span { class: "badge", title: "Attached to a transaction", "Attached" }
                        span { class: "muted small",
                            "{transaction.merchant} · {signed_money(transaction.amount_cents, transaction.kind == myledger_core::TransactionKind::Income)}"
                        }
                    }
                }
            }

            td { "data-label": "Type",
                span { class: "muted small", "{friendly_type(&document)}" }
            }

            td { class: "numeric", "data-label": "Size", "{friendly_size(document.size)}" }

            td { "data-label": "Source",
                span { class: "muted small",
                    "{document.source.clone().unwrap_or_else(|| \"—\".to_string())}"
                }
            }

            td { "data-label": "Status", StatusBadge { status: document.status } }

            td { "data-label": "Uploaded",
                span { class: "muted small", "{document.created_at.get(..10).unwrap_or(\"\")}" }
            }

            td { class: "right",
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| {
                        let open = reviewing();
                        reviewing.set(!open);
                    },
                    if reviewing() { "Close" } else { "Read" }
                }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| {
                        spawn(async move {
                            store.act(api::delete_document(id), "Document deleted.").await;
                        });
                    },
                    "Delete"
                }
            }
        }

        if reviewing() {
            tr { class: "tagrow",
                td { colspan: "7",
                    ReviewPanel { state: state.clone(), document: document.clone() }
                }
            }
        }
    }
}

/// Reading a document, and filing it.
///
/// Opens showing whatever a previous read found, so reopening it does not
/// re-read the file. Running the rules again is an explicit action, which is
/// the normal loop: read, look at the text, write a pattern, read again.
#[component]
fn ReviewPanel(state: StateResponse, document: Document) -> Element {
    let store = Store::use_store();
    let id = document.id;
    let busy = *store.saving.read();

    let mut outcome = use_signal(|| None::<String>);
    let mut note = use_signal(|| None::<String>);
    let mut text = use_signal(|| None::<String>);
    let mut candidates = use_signal(Vec::<Candidate>::new);
    let mut rules_tried = use_signal(|| None::<usize>);
    let mut loading = use_signal(|| true);

    // Once, on open: what did the last read find?
    use_hook(move || {
        spawn(async move {
            match api::stored_extraction(id).await {
                Ok(Some(stored)) => {
                    outcome.set(Some(stored.outcome));
                    note.set(stored.note);
                    text.set(stored.text);
                    candidates.set(stored.candidates);
                }
                Ok(None) => outcome.set(None),
                Err(failure) => store.fail(failure),
            }
            loading.set(false);
        });
    });

    let run = move |_| {
        spawn(async move {
            if let Some(result) = store.act(api::extract_document(id), "").await {
                outcome.set(Some(result.outcome.clone()));
                note.set(result.note.clone());
                text.set(result.text.clone());
                candidates.set(result.candidates.clone());
                rules_tried.set(Some(result.rules_tried));

                match result.created_transaction {
                    Some(transaction) => store.say(format!(
                        "Recorded {} for {}.",
                        money(transaction.amount_cents),
                        transaction.merchant
                    )),
                    None => match result.outcome.as_str() {
                        "matched" => store.say("Already recorded; the receipt is attached to it."),
                        "ambiguous" => store.say("More than one rule matched. Pick the right reading below."),
                        "no-text" => store.say("No text could be read from this file."),
                        _ => store.say("No rule matched this document."),
                    },
                }
            }
        });
    };

    rsx! {
        div { class: "review",
            div { class: "review-actions",
                button {
                    class: "button button-primary button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: run,
                    if outcome().is_some() { "Read it again" } else { "Read this document" }
                }
                if let Some(tried) = rules_tried() {
                    span { class: "muted small",
                        if tried == 0 {
                            "No extraction rules are enabled yet."
                        } else if tried == 1 {
                            "1 rule was tried."
                        } else {
                            "{tried} rules were tried."
                        }
                    }
                }
            }

            if loading() {
                p { class: "muted small", "Checking what was found last time…" }
            }

            match outcome().as_deref() {
                None => rsx! {
                    p { class: "muted small",
                        "This document hasn't been read yet."
                    }
                },
                Some("no-text") => rsx! {
                    div { class: "importasks",
                        p { class: "importasks-title", "No text could be read" }
                        p { class: "muted",
                            "{note().unwrap_or_else(|| \"The file had no readable text.\".to_string())}"
                        }
                    }
                },
                Some("no-match") => rsx! {
                    div { class: "importasks",
                        p { class: "importasks-title", "No rule matched" }
                        p { class: "muted",
                            "The text below was read from the file. Write an extraction rule against it, then read the document again."
                        }
                    }
                },
                Some("ambiguous") => rsx! {
                    div { class: "importasks",
                        p { class: "importasks-title", "More than one rule matched" }
                        p { class: "muted",
                            "They disagree about what this document says, so nothing has been recorded. Choose the right reading, or narrow the rules."
                        }
                    }
                },
                Some(_) => rsx! {
                    p { class: "importok", "A rule matched and this document is filed." }
                },
            }

            if !candidates().is_empty() {
                div { class: "candidates",
                    h3 { class: "review-title",
                        if candidates().len() == 1 { "What was read" } else { "The readings" }
                    }
                    for (index, candidate) in candidates().iter().enumerate() {
                        CandidateRow {
                            key: "{candidate.rule_id}-{index}",
                            state: state.clone(),
                            document_id: id,
                            candidate: candidate.clone(),
                            choosable: candidates().len() > 1
                                && document.linked_transaction_id.is_none(),
                        }
                    }
                }
            }

            // Filing it by hand, for everything extraction could not do.
            if document.linked_transaction_id.is_none() {
                ManualLink { state: state.clone(), document_id: id }
            } else {
                div { class: "review-actions",
                    span { class: "muted small", "Attached to a transaction." }
                    button {
                        class: "button button-quiet button-small",
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| {
                            spawn(async move {
                                store
                                    .act(
                                        api::patch_document(
                                            id,
                                            &api::DocumentPatch {
                                                linked_transaction_id: Some(None),
                                                status: Some("review".to_string()),
                                            },
                                        ),
                                        "Receipt detached.",
                                    )
                                    .await;
                            });
                        },
                        "Detach"
                    }
                }
            }

            if let Some(body) = text() {
                details { class: "skipped",
                    summary { "The text read from this file" }
                    pre { class: "doctext", "{body}" }
                    p { class: "muted small",
                        "An extraction rule is a regular expression matched against this text."
                    }
                }
            }
        }
    }
}

/// One reading a rule produced.
#[component]
fn CandidateRow(
    state: StateResponse,
    document_id: Uuid,
    candidate: Candidate,
    choosable: bool,
) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();

    let category = candidate.category_id.and_then(|id| state.category_name(id));
    let is_income = candidate.kind == myledger_core::TransactionKind::Income;

    rsx! {
        div { class: "candidate",
            div { class: "candidate-main",
                span { class: "minirow-merchant", "{candidate.merchant}" }
                span { class: "minirow-meta",
                    "{candidate.date}"
                    if !candidate.date_was_captured {
                        " (upload date — the rule captured none)"
                    }
                    if let Some(name) = category {
                        " · {name}"
                    }
                    " · by rule “{candidate.rule_label}”"
                }
            }
            span {
                class: if is_income { "amount amount-in" } else { "amount amount-out" },
                "{signed_money(candidate.amount_cents, is_income)}"
            }
            if choosable {
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    disabled: busy,
                    onclick: {
                        let candidate = candidate.clone();
                        move |_| {
                            let entry = api::NewTransaction {
                                date: candidate.date.clone(),
                                merchant: candidate.merchant.clone(),
                                amount_cents: candidate.amount_cents,
                                kind: candidate.kind.as_str().to_string(),
                                category_id: candidate.category_id,
                                account_id: None,
                                tags: Vec::new(),
                                source: "document".to_string(),
                            };
                            spawn(async move {
                                // Record it, then attach this document to it.
                                let Some(report) =
                                    store.act(api::create_transaction(&entry), "").await
                                else {
                                    return;
                                };
                                let Some(transaction) = report.transactions.first() else {
                                    store.say("That reading is already in your ledger.");
                                    return;
                                };
                                store
                                    .act(
                                        api::patch_document(
                                            document_id,
                                            &api::DocumentPatch {
                                                linked_transaction_id: Some(Some(transaction.id)),
                                                status: Some("stored".to_string()),
                                            },
                                        ),
                                        "Recorded, with the receipt attached.",
                                    )
                                    .await;
                            });
                        }
                    },
                    "Use this one"
                }
            }
        }
    }
}

/// Attaching a document to a transaction the user picks.
#[component]
fn ManualLink(state: StateResponse, document_id: Uuid) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();
    let mut chosen = use_signal(|| None::<Uuid>);

    // Newest first, and only the ones without a receipt already — those are
    // the ones a loose document plausibly belongs to.
    let candidates: Vec<&Transaction> =
        state.transactions.iter().filter(|t| t.deleted_at.is_none()).take(50).collect();

    rsx! {
        div { class: "manuallink",
            h3 { class: "review-title", "File it by hand" }
            if candidates.is_empty() {
                p { class: "muted small",
                    "There are no transactions to attach this to yet. Record one from the top bar, then come back."
                }
            } else {
                div { class: "manuallink-row",
                    label { class: "field field-grow",
                        span { class: "field-label", "Attach to a transaction" }
                        select {
                            class: "field-input",
                            disabled: busy,
                            onchange: move |event| {
                                chosen.set(Uuid::parse_str(&event.value()).ok());
                            },
                            option { value: "", "Choose a transaction…" }
                            for transaction in candidates.iter() {
                                option {
                                    key: "{transaction.id}",
                                    value: "{transaction.id}",
                                    "{transaction.date} · {transaction.merchant} · {money(transaction.amount_cents)}"
                                    if transaction.receipt { " (has a receipt)" }
                                }
                            }
                        }
                    }
                    button {
                        class: "button button-primary button-small",
                        r#type: "button",
                        disabled: busy || chosen().is_none(),
                        onclick: move |_| {
                            let Some(transaction_id) = chosen() else { return };
                            spawn(async move {
                                store
                                    .act(
                                        api::patch_document(
                                            document_id,
                                            &api::DocumentPatch {
                                                linked_transaction_id: Some(Some(transaction_id)),
                                                status: Some("stored".to_string()),
                                            },
                                        ),
                                        "Receipt attached.",
                                    )
                                    .await;
                            });
                        },
                        "Attach"
                    }
                }
            }
        }
    }
}

/// Where a document has got to.
#[component]
fn StatusBadge(status: DocumentStatus) -> Element {
    let (label, title) = match status {
        DocumentStatus::Queued => ("Queued", "Stored, not read yet"),
        DocumentStatus::Stored => ("Stored", "Read and filed"),
        DocumentStatus::Review => ("Review", "Needs you to look at it"),
    };

    rsx! {
        span { class: "badge badge-doc-{status.as_str()}", title: title, "{label}" }
    }
}

// ---- extraction rules ------------------------------------------------------

/// The small settings section for document extraction rules.
#[component]
fn ExtractionRules(state: StateResponse) -> Element {
    let store = Store::use_store();
    let mut adding = use_signal(|| false);
    let rules = &state.document_extraction_rules;
    let busy = *store.saving.read();

    rsx! {
        div { class: "card",
            header { class: "card-head",
                h2 { class: "card-title", "Document extraction rules" }
                button {
                    class: "button button-quiet button-small",
                    r#type: "button",
                    onclick: move |_| {
                        let open = adding();
                        adding.set(!open);
                    },
                    if adding() { "Cancel" } else { "Add a rule" }
                }
            }
            p { class: "muted small",
                "A rule is a pattern matched against the text read from a document, saying which part is the amount, the merchant and the date. A document is only recorded automatically when exactly one rule matches it."
            }

            if adding() {
                RuleForm {
                    state: state.clone(),
                    on_done: move |_| adding.set(false),
                }
            }

            if rules.is_empty() {
                EmptyState {
                    title: "No extraction rules",
                    blurb: "None are provided — a rule has to match your own receipts. Read a document first, then write a pattern against the text it shows.",
                }
            } else {
                ul { class: "minilist",
                    for rule in rules.iter() {
                        li { class: "minirow", key: "{rule.id}",
                            div { class: "minirow-main",
                                span { class: "minirow-merchant", "{rule.label}" }
                                span { class: "minirow-meta", "{rule_summary(rule)}" }
                                code { class: "rulepattern", "{rule.body_regex}" }
                            }
                            div { class: "rowactions",
                                label { class: "checkline small",
                                    input {
                                        r#type: "checkbox",
                                        checked: rule.enabled,
                                        disabled: busy,
                                        onchange: {
                                            let id = rule.id;
                                            move |event: FormEvent| {
                                                let on = event.checked();
                                                spawn(async move {
                                                    store
                                                        .act(
                                                            api::toggle_extraction_rule(id, on),
                                                            if on { "Rule enabled." } else { "Rule disabled." },
                                                        )
                                                        .await;
                                                });
                                            }
                                        },
                                    }
                                    span { "Enabled" }
                                }
                                button {
                                    class: "button button-quiet button-small",
                                    r#type: "button",
                                    disabled: busy,
                                    onclick: {
                                        let id = rule.id;
                                        move |_| {
                                            spawn(async move {
                                                store
                                                    .act(
                                                        api::delete_extraction_rule(id),
                                                        "Rule deleted.",
                                                    )
                                                    .await;
                                            });
                                        }
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Writing a new extraction rule.
#[component]
fn RuleForm(state: StateResponse, on_done: EventHandler<()>) -> Element {
    let store = Store::use_store();
    let busy = *store.saving.read();

    let mut label = use_signal(String::new);
    let mut pattern = use_signal(String::new);
    let mut filename = use_signal(String::new);
    let mut amount_group = use_signal(|| "1".to_string());
    let mut merchant_group = use_signal(String::new);
    let mut date_group = use_signal(String::new);
    let mut kind = use_signal(|| "debit".to_string());
    let mut account = use_signal(String::new);
    let mut category = use_signal(|| None::<Uuid>);
    let mut error = use_signal(|| None::<String>);

    let submit = move |_| {
        if label().trim().is_empty() {
            error.set(Some("Give the rule a name.".to_string()));
            return;
        }
        if pattern().trim().is_empty() {
            error.set(Some("A rule needs a pattern to match.".to_string()));
            return;
        }
        let Some(amount) = amount_group().trim().parse::<i64>().ok() else {
            error.set(Some("Say which capture group holds the amount.".to_string()));
            return;
        };
        error.set(None);

        let input = api::ExtractionRuleInput {
            label: label().trim().to_string(),
            filename_pattern: blank(filename()),
            body_regex: pattern().trim().to_string(),
            amount_group: Some(amount),
            merchant_group: merchant_group().trim().parse().ok(),
            date_group: date_group().trim().parse().ok(),
            kind_hint: Some(kind()),
            account_hint: blank(account()),
            default_category_id: category(),
            enabled: true,
        };

        spawn(async move {
            if store.act(api::create_extraction_rule(&input), "Rule saved.").await.is_some() {
                on_done.call(());
            }
        });
    };

    rsx! {
        div { class: "ruleform",
            div { class: "formgrid",
                label { class: "field field-wide",
                    span { class: "field-label", "Name" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        placeholder: "Corner Grocer receipts",
                        value: "{label}",
                        disabled: busy,
                        oninput: move |event| label.set(event.value()),
                    }
                }

                label { class: "field field-wide",
                    span { class: "field-label", "Pattern" }
                    input {
                        class: "field-input mono",
                        r#type: "text",
                        placeholder: r"(?i)total:\s*([0-9.]+)",
                        value: "{pattern}",
                        disabled: busy,
                        oninput: move |event| pattern.set(event.value()),
                    }
                    span { class: "field-hint",
                        "A regular expression. Each bracketed group can be used below."
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Amount is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        value: "{amount_group}",
                        disabled: busy,
                        oninput: move |event| amount_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Merchant is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        placeholder: "optional",
                        value: "{merchant_group}",
                        disabled: busy,
                        oninput: move |event| merchant_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Date is group" }
                    input {
                        class: "field-input",
                        r#type: "number",
                        min: "1",
                        placeholder: "optional",
                        value: "{date_group}",
                        disabled: busy,
                        oninput: move |event| date_group.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Direction" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| kind.set(event.value()),
                        option { value: "debit", selected: kind() == "debit", "Money out" }
                        option { value: "credit", selected: kind() == "credit", "Money in" }
                        option { value: "auto", selected: kind() == "auto", "Read it from the text" }
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Only files named like" }
                    input {
                        class: "field-input mono",
                        r#type: "text",
                        placeholder: "optional, e.g. ^grocer",
                        value: "{filename}",
                        disabled: busy,
                        oninput: move |event| filename.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Account name" }
                    input {
                        class: "field-input",
                        r#type: "text",
                        placeholder: "optional",
                        value: "{account}",
                        disabled: busy,
                        oninput: move |event| account.set(event.value()),
                    }
                }

                label { class: "field",
                    span { class: "field-label", "Category" }
                    select {
                        class: "field-input",
                        disabled: busy,
                        onchange: move |event| category.set(Uuid::parse_str(&event.value()).ok()),
                        option { value: "", "No category" }
                        for item in state.live_categories() {
                            option { key: "{item.id}", value: "{item.id}", "{item.name}" }
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
                    r#type: "button",
                    disabled: busy,
                    onclick: submit,
                    "Save rule"
                }
            }
        }
    }
}

/// A one-line description of what a rule does.
fn rule_summary(rule: &DocumentExtractionRule) -> String {
    let mut parts = Vec::new();
    parts.push(match rule.kind_hint {
        Some(SmsKindHint::Credit) => "money in".to_string(),
        Some(SmsKindHint::Auto) => "direction from the text".to_string(),
        _ => "money out".to_string(),
    });
    if let Some(group) = rule.amount_group {
        parts.push(format!("amount from group {group}"));
    }
    if let Some(group) = rule.merchant_group {
        parts.push(format!("merchant from group {group}"));
    }
    if let Some(group) = rule.date_group {
        parts.push(format!("date from group {group}"));
    }
    if let Some(pattern) = &rule.filename_pattern {
        parts.push(format!("files matching {pattern}"));
    }
    if !rule.enabled {
        parts.push("disabled".to_string());
    }
    parts.join(" · ")
}

/// A readable file type, never a MIME string on its own.
fn friendly_type(document: &Document) -> String {
    let mime = document.mime_type.as_deref().unwrap_or("");
    match mime {
        "application/pdf" => return "PDF".to_string(),
        "text/csv" | "application/csv" => return "CSV".to_string(),
        "text/plain" => return "Text".to_string(),
        _ => {}
    }
    if mime.starts_with("image/") {
        return format!("Image ({})", mime.trim_start_matches("image/").to_uppercase());
    }
    if let Some((_, extension)) = document.filename.to_lowercase().rsplit_once('.') {
        return extension.to_uppercase();
    }
    "Unknown".to_string()
}

/// Bytes as something a person reads.
fn friendly_size(size: Option<i64>) -> String {
    let Some(bytes) = size.filter(|b| *b >= 0) else {
        return "—".to_string();
    };
    const KIB: i64 = 1024;
    const MIB: i64 = 1024 * 1024;
    if bytes >= MIB {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{} KB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn blank(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
