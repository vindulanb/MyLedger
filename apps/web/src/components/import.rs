//! The CSV import dialog.
//!
//! Four steps: pick a file, look at what was read from it, answer anything
//! the parser could not settle, then import and see the counts.
//!
//! The file is parsed here in the browser, with `core::csv` — the same reader
//! the server uses. That matters for honesty as much as for speed: the
//! preview the user approves and the import that runs cannot disagree,
//! because they are one implementation. The server parses again from the
//! bytes and never trusts this parse; all it does here is decide what to ask.
//!
//! Nothing is guessed. If the date order is ambiguous, or every amount is
//! positive with nothing saying which way the money went, the import button
//! stays disabled until the question is answered. Picking a default would
//! produce a clean-looking success and a ledger full of wrong dates.

use dioxus::prelude::*;
use myledger_core::csv::{AmountGuess, DateGuess, Plan, Table, plan, read_csv};
use myledger_core::{ImportColumns, ImportCsvReport, ImportOptions};

use crate::api;
use crate::store::Store;

/// How many data rows the preview shows.
const PREVIEW_ROWS: usize = 5;

/// Where the dialog is up to.
enum Stage {
    /// Nothing picked yet.
    Waiting,
    /// Reading and parsing the chosen file.
    Reading,
    /// Parsed. Showing the preview, and any questions.
    Ready(Box<Read>),
    /// The file could not be parsed at all.
    Unreadable(String),
    /// Uploading.
    Importing,
    /// Done.
    Done(Box<ImportCsvReport>),
}

/// A parsed file and what was made of it.
struct Read {
    filename: String,
    bytes: Vec<u8>,
    table: Table,
    plan: Plan,
}

#[component]
pub fn ImportForm(on_done: EventHandler<()>) -> Element {
    let store = Store::use_store();
    let mut stage = use_signal(|| Stage::Waiting);

    // The user's answers. Each starts empty and is only sent if filled in.
    let mut date_format = use_signal(String::new);
    let mut direction = use_signal(String::new);
    let mut columns = use_signal(ImportColumns::default);
    let mut showing_mapping = use_signal(|| false);

    rsx! {
        div { class: "importer",
            match &*stage.read() {
                Stage::Waiting => rsx! {
                    p { class: "muted",
                        "Choose a CSV exported from your bank or card. Nothing is imported until you've seen what it contains."
                    }
                    label { class: "field",
                        span { class: "field-label", "Statement file" }
                        input {
                            class: "field-input",
                            r#type: "file",
                            accept: ".csv,text/csv,text/plain",
                            onchange: move |event| {
                                let Some(file) = event.files().into_iter().next() else {
                                    return;
                                };
                                stage.set(Stage::Reading);
                                date_format.set(String::new());
                                direction.set(String::new());
                                columns.set(ImportColumns::default());
                                showing_mapping.set(false);

                                spawn(async move {
                                    let name = file.name();
                                    let bytes = match file.read_bytes().await {
                                        Ok(bytes) => bytes.to_vec(),
                                        Err(problem) => {
                                            stage.set(Stage::Unreadable(format!(
                                                "Could not read {name}: {problem}"
                                            )));
                                            return;
                                        }
                                    };
                                    // Lossy, like the server: a stray byte in
                                    // one merchant name should not cost the
                                    // whole import.
                                    let text = String::from_utf8_lossy(&bytes).into_owned();
                                    match read_csv(&text) {
                                        Ok(table) => {
                                            let found = plan(&table);
                                            stage.set(Stage::Ready(Box::new(Read {
                                                filename: name,
                                                bytes,
                                                table,
                                                plan: found,
                                            })));
                                        }
                                        Err(problem) => stage.set(Stage::Unreadable(format!(
                                            "{name} could not be read: {problem}."
                                        ))),
                                    }
                                });
                            },
                        }
                    }
                },

                Stage::Reading => rsx! { p { class: "muted", "Reading the file…" } },

                Stage::Unreadable(message) => rsx! {
                    p { class: "formerror", role: "alert", "{message}" }
                    button {
                        class: "button button-quiet",
                        r#type: "button",
                        onclick: move |_| stage.set(Stage::Waiting),
                        "Choose another file"
                    }
                },

                Stage::Ready(read) => {
                    let questions = outstanding(&read.plan, &date_format(), &direction(), &columns());
                    rsx! {
                        div { class: "importhead",
                            div {
                                strong { "{read.filename}" }
                                p { class: "muted small",
                                    "{read.table.rows.len()} rows, {read.table.headers.len()} columns, separated by {delimiter_name(read.table.delimiter)}."
                                }
                            }
                            button {
                                class: "button button-quiet button-small",
                                r#type: "button",
                                onclick: move |_| stage.set(Stage::Waiting),
                                "Change file"
                            }
                        }

                        ColumnPreview { table: read.table.clone(), plan: read.plan.clone(), columns: columns() }

                        if questions.is_empty() {
                            p { class: "importok",
                                "Everything needed was found in the file. Nothing will be guessed."
                            }
                        } else {
                            div { class: "importasks", role: "group", "aria-label": "Questions about this file",
                                p { class: "importasks-title",
                                    "This file is ambiguous. Rather than guess, MyLedger needs you to decide:"
                                }
                                ul { class: "importasks-list",
                                    for question in questions.iter() {
                                        li { key: "{question}", "{question}" }
                                    }
                                }
                            }
                        }

                        // Shown when needed, and available otherwise.
                        if needs_date_answer(&read.plan) {
                            label { class: "field",
                                span { class: "field-label", "Date order" }
                                select {
                                    class: "field-input",
                                    onchange: move |event| date_format.set(event.value()),
                                    option { value: "", "Choose…" }
                                    option { value: "day-first", "Day first — 03/04/2026 is 3 April" }
                                    option { value: "month-first", "Month first — 03/04/2026 is 4 March" }
                                    option { value: "iso", "Year first — 2026-04-03" }
                                }
                            }
                        }

                        if needs_direction_answer(&read.plan) {
                            label { class: "field",
                                span { class: "field-label", "Which way does the money go?" }
                                select {
                                    class: "field-input",
                                    onchange: move |event| direction.set(event.value()),
                                    option { value: "", "Choose…" }
                                    option { value: "expense", "Every row is money out" }
                                    option { value: "income", "Every row is money in" }
                                }
                                span { class: "field-hint",
                                    "A row marked CR, or with a minus sign, is still read as written."
                                }
                            }
                        }

                        button {
                            class: "button button-quiet button-small",
                            r#type: "button",
                            onclick: move |_| {
                                let open = showing_mapping();
                                showing_mapping.set(!open);
                            },
                            if showing_mapping() { "Hide column mapping" } else { "Change column mapping" }
                        }

                        if showing_mapping() {
                            MappingControls {
                                headers: read.table.headers.clone(),
                                columns,
                            }
                        }

                        div { class: "formactions",
                            button {
                                class: "button button-quiet",
                                r#type: "button",
                                onclick: move |_| on_done.call(()),
                                "Cancel"
                            }
                            button {
                                class: "button button-primary",
                                r#type: "button",
                                disabled: !questions.is_empty(),
                                onclick: {
                                    let bytes = read.bytes.clone();
                                    let filename = read.filename.clone();
                                    move |_| {
                                        let options = ImportOptions {
                                            date_format: blank_to_none(date_format()),
                                            direction: blank_to_none(direction()),
                                            columns: columns(),
                                        };
                                        let upload = api::Upload {
                                            filename: filename.clone(),
                                            mime_type: Some("text/csv".to_string()),
                                            bytes: bytes.clone(),
                                        };
                                        stage.set(Stage::Importing);
                                        spawn(async move {
                                            match api::import_csv(&upload, &options).await {
                                                Ok(report) => {
                                                    // The ledger has changed,
                                                    // so every screen behind
                                                    // this dialog is stale.
                                                    store.reload().await;
                                                    stage.set(Stage::Done(Box::new(report)));
                                                }
                                                Err(failure) => {
                                                    stage.set(Stage::Unreadable(failure.message));
                                                }
                                            }
                                        });
                                    }
                                },
                                "Import {read.table.rows.len()} rows"
                            }
                        }
                    }
                }

                Stage::Importing => rsx! { p { class: "muted", "Importing…" } },

                Stage::Done(report) => rsx! {
                    Outcome { report: (**report).clone() }
                    div { class: "formactions",
                        button {
                            class: "button button-quiet",
                            r#type: "button",
                            onclick: move |_| stage.set(Stage::Waiting),
                            "Import another"
                        }
                        button {
                            class: "button button-primary",
                            r#type: "button",
                            onclick: move |_| on_done.call(()),
                            "Done"
                        }
                    }
                },
            }
        }
    }
}

/// The first few rows, with each column's detected role above it.
#[component]
fn ColumnPreview(table: Table, plan: Plan, columns: ImportColumns) -> Element {
    let roles: Vec<&'static str> =
        (0..table.headers.len()).map(|index| role_of(index, &plan, &columns)).collect();

    rsx! {
        div { class: "tablewrap",
            table { class: "preview",
                thead {
                    tr {
                        for (index, header) in table.headers.iter().enumerate() {
                            th { key: "h{index}", scope: "col",
                                span { class: "preview-name", "{header}" }
                                span {
                                    class: if roles[index].is_empty() { "preview-role is-unused" } else { "preview-role" },
                                    if roles[index].is_empty() { "not used" } else { "{roles[index]}" }
                                }
                            }
                        }
                    }
                }
                tbody {
                    for (row_index, row) in table.rows.iter().take(PREVIEW_ROWS).enumerate() {
                        tr { key: "r{row_index}",
                            for (index, value) in row.iter().enumerate() {
                                td { key: "c{index}",
                                    if value.is_empty() {
                                        span { class: "muted", "—" }
                                    } else {
                                        "{value}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if table.rows.len() > PREVIEW_ROWS {
            p { class: "muted small", "Showing the first {PREVIEW_ROWS} of {table.rows.len()} rows." }
        }
    }
}

/// Explicit column choices, for a file nothing recognised.
#[component]
fn MappingControls(headers: Vec<String>, columns: Signal<ImportColumns>) -> Element {
    // Each row is (label, read the current value, write a new one).
    rsx! {
        div { class: "mapgrid",
            for (label, field) in [
                ("Date", Field::Date),
                ("Merchant", Field::Merchant),
                ("Amount", Field::Amount),
                ("Money out", Field::Debit),
                ("Money in", Field::Credit),
                ("Debit or credit", Field::Kind),
                ("Category", Field::Category),
                ("Account", Field::Account),
            ] {
                label { key: "{label}", class: "field",
                    span { class: "field-label", "{label}" }
                    select {
                        class: "field-input",
                        onchange: {
                            let mut columns = columns;
                            move |event: FormEvent| {
                                let chosen = event.value().parse::<usize>().ok();
                                let mut current = columns();
                                field.set(&mut current, chosen);
                                columns.set(current);
                            }
                        },
                        option { value: "", "Not in this file" }
                        for (index, header) in headers.iter().enumerate() {
                            option {
                                key: "{index}",
                                value: "{index}",
                                selected: field.get(&columns()) == Some(index),
                                "{header}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One mapping field, so the eight selects share one control.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Date,
    Merchant,
    Amount,
    Debit,
    Credit,
    Kind,
    Category,
    Account,
}

impl Field {
    fn get(self, columns: &ImportColumns) -> Option<usize> {
        match self {
            Field::Date => columns.date,
            Field::Merchant => columns.merchant,
            Field::Amount => columns.amount,
            Field::Debit => columns.debit,
            Field::Credit => columns.credit,
            Field::Kind => columns.kind,
            Field::Category => columns.category,
            Field::Account => columns.account,
        }
    }

    fn set(self, columns: &mut ImportColumns, value: Option<usize>) {
        match self {
            Field::Date => columns.date = value,
            Field::Merchant => columns.merchant = value,
            Field::Amount => columns.amount = value,
            Field::Debit => columns.debit = value,
            Field::Credit => columns.credit = value,
            Field::Kind => columns.kind = value,
            Field::Category => columns.category = value,
            Field::Account => columns.account = value,
        }
    }
}

/// The counts, after an import.
#[component]
fn Outcome(report: ImportCsvReport) -> Element {
    rsx! {
        div { class: "importdone", role: "status",
            p { class: "importdone-title",
                if report.inserted == 0 {
                    "Nothing new to add."
                } else if report.inserted == 1 {
                    "1 transaction imported."
                } else {
                    "{report.inserted} transactions imported."
                }
            }

            dl { class: "factgrid",
                div { class: "fact",
                    dt { class: "fact-label", "Imported" }
                    dd { class: "fact-value", "{report.inserted}" }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Already on file" }
                    dd { class: "fact-value", "{report.duplicates}" }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Skipped" }
                    dd { class: "fact-value", "{report.skipped}" }
                }
                div { class: "fact",
                    dt { class: "fact-label", "Needs review" }
                    dd { class: "fact-value", "{report.needs_review}" }
                }
            }

            if report.duplicates > 0 {
                p { class: "muted small",
                    "Rows already on file were matched by date, merchant, amount and account, and left alone."
                }
            }

            if !report.skipped_detail.is_empty() {
                details { class: "skipped",
                    summary { "Why {report.skipped} rows were skipped" }
                    ul { class: "skipped-list",
                        for row in report.skipped_detail.iter() {
                            li { key: "{row.line}", "Line {row.line}: {row.reason}" }
                        }
                    }
                    p { class: "muted small",
                        "Skipped rows were not imported, and nothing was invented in their place."
                    }
                }
            }
        }
    }
}

/// What still has to be answered, taking the user's replies into account.
fn outstanding(
    plan: &Plan,
    date_format: &str,
    direction: &str,
    columns: &ImportColumns,
) -> Vec<String> {
    let mut asking = Vec::new();

    if plan.mapping.date.is_none() && columns.date.is_none() {
        asking.push("Which column holds the date?".to_string());
    }
    if plan.mapping.merchant.is_none() && columns.merchant.is_none() {
        asking.push("Which column holds the description or merchant?".to_string());
    }

    if needs_date_answer(plan) && date_format.is_empty() {
        asking.push(match plan.date {
            DateGuess::Ambiguous => {
                "Are the dates day-first or month-first? Every row in this file reads \
                 either way."
                    .to_string()
            }
            _ => "The date column could not be read. Which column is it, and in what order?"
                .to_string(),
        });
    }

    let amount_given = columns.amount.is_some()
        || (columns.debit.is_some() && columns.credit.is_some());
    match &plan.amount {
        AmountGuess::Certain(_) => {}
        AmountGuess::NeedsDirection { .. } => {
            if direction.is_empty() {
                asking.push(
                    "Every amount in this file is positive and nothing says which way the \
                     money went."
                        .to_string(),
                );
            }
        }
        AmountGuess::Unreadable => {
            if !amount_given {
                asking.push("Which column holds the amount?".to_string());
            }
        }
    }

    asking
}

fn needs_date_answer(plan: &Plan) -> bool {
    !matches!(plan.date, DateGuess::Certain(_))
}

fn needs_direction_answer(plan: &Plan) -> bool {
    matches!(plan.amount, AmountGuess::NeedsDirection { .. })
}

/// What a column was detected or chosen as, for the preview header.
fn role_of(index: usize, plan: &Plan, columns: &ImportColumns) -> &'static str {
    let is = |chosen: Option<usize>, detected: Option<usize>| chosen.or(detected) == Some(index);

    if is(columns.date, plan.mapping.date) {
        return "date";
    }
    if is(columns.merchant, plan.mapping.merchant) {
        return "merchant";
    }
    if is(columns.category, plan.mapping.category) {
        return "category";
    }
    if is(columns.account, plan.mapping.account) {
        return "account";
    }
    if columns.debit == Some(index) {
        return "money out";
    }
    if columns.credit == Some(index) {
        return "money in";
    }
    if columns.kind == Some(index) {
        return "debit or credit";
    }
    if columns.amount == Some(index) {
        return "amount";
    }

    match &plan.amount {
        AmountGuess::Certain(source) => match source {
            myledger_core::AmountSource::DebitCredit { debit, credit } => {
                if *debit == index {
                    "money out"
                } else if *credit == index {
                    "money in"
                } else {
                    ""
                }
            }
            myledger_core::AmountSource::Signed { amount }
            | myledger_core::AmountSource::Fixed { amount, .. } => {
                if *amount == index { "amount" } else { "" }
            }
            myledger_core::AmountSource::Labelled { amount, kind } => {
                if *amount == index {
                    "amount"
                } else if *kind == index {
                    "debit or credit"
                } else {
                    ""
                }
            }
        },
        AmountGuess::NeedsDirection { amount } => {
            if *amount == index { "amount" } else { "" }
        }
        AmountGuess::Unreadable => "",
    }
}

fn delimiter_name(delimiter: char) -> &'static str {
    match delimiter {
        ',' => "commas",
        ';' => "semicolons",
        '\t' => "tabs",
        '|' => "pipes",
        _ => "an unusual character",
    }
}

fn blank_to_none(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}
