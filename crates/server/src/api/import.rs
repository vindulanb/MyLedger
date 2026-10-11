//! `POST /api/v1/import/csv` — importing a bank or card statement.
//!
//! The parsing itself lives in `core::csv`, which the web app also uses to
//! show its column preview. That sharing is deliberate: the preview the user
//! approves and the import that runs are the same reader, so the screen
//! cannot promise one reading and the server perform another.
//!
//! What this module adds is everything that touches the database:
//!
//! - **Dedupe.** Every row goes through `transactions::insert_one`, the same
//!   function a manual entry and an SMS sync use. There is no second insert
//!   path that could disagree about what counts as the same purchase, which
//!   means re-importing a file is reliably a no-op.
//! - **Names to IDs.** A statement names its categories and accounts; this
//!   resolves them against the ones the user actually has, case-insensitively.
//! - **Refusing to guess.** If the file is ambiguous about its date order or
//!   its sign convention, the import is refused with the questions that need
//!   answering, rather than picking one and reporting a clean success.
//!
//! Unparseable lines are counted and described. No placeholder transaction is
//! ever created for one.

use axum::Json;
use axum::extract::{Multipart, State, multipart::MultipartError};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use myledger_core::csv::{
    AmountGuess, AmountSource, DateFormat, DateGuess, Mapping, Table, read_csv, read_rows,
};
// The request and response shapes live in `core::wire`: the web app builds
// one and reads the other, so one definition is what keeps the two sides from
// drifting. `Options` and `Columns` are the local names for them.
pub use myledger_core::wire::{
    ImportColumns as Columns, ImportCsvReport, ImportOptions as Options, SkippedDetail,
};
use myledger_db::rules as db_rules;
use serde_json::json;

use crate::AppState;
use crate::api::Lookups;
use crate::api::transactions::{Outcome, insert_one};
use crate::error::ApiError;

/// Largest statement accepted, in bytes. 10 MiB is a great many rows.
pub const MAX_CSV_BYTES: u64 = 10 * 1024 * 1024;

/// Ceiling on the request body, with slack for the multipart framing.
pub const MAX_REQUEST_BYTES: usize = (MAX_CSV_BYTES + 256 * 1024) as usize;

/// Most rows one file may carry.
const MAX_ROWS: usize = 20_000;

/// The category every row falls back to when nothing better is known.
const FALLBACK_CATEGORY: &str = "Needs review";

/// How many skipped rows are described individually.
///
/// A file where every line is unreadable should not come back as a response
/// larger than the file that caused it.
const MAX_SKIPPED_DETAIL: usize = 50;

pub async fn import_csv(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    let (bytes, options) = collect(multipart).await?;

    // Statements are exported from spreadsheets, which are not reliably
    // UTF-8. Lossy rather than refusing: a stray byte in a merchant name
    // should not cost the user the whole import.
    let text = String::from_utf8_lossy(&bytes);

    let table = read_csv(&text).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if table.rows.is_empty() {
        return Err(ApiError::bad_request("the file has a header row but no data rows"));
    }
    if table.rows.len() > MAX_ROWS {
        return Err(ApiError::bad_request(format!(
            "{} rows in one file; the limit is {MAX_ROWS}",
            table.rows.len()
        )));
    }

    let (mapping, date_format, amount_source) = settle(&table, &options)?;

    let lookups = Lookups::load(&state.db).await?;
    let rules = db_rules::list_rules(&state.db).await?;

    // Resolved once for the file. A statement names the same handful of
    // categories on every row.
    let fallback = lookups
        .categories
        .iter()
        .find(|c| c.deleted_at.is_none() && c.name.eq_ignore_ascii_case(FALLBACK_CATEGORY))
        .map(|c| c.id);

    let read = read_rows(&table, mapping, date_format, amount_source);

    let mut report = ImportCsvReport {
        inserted: 0,
        duplicates: 0,
        skipped: read.skipped.len(),
        needs_review: 0,
        rows_in_file: table.rows.len(),
        skipped_detail: read
            .skipped
            .iter()
            .take(MAX_SKIPPED_DETAIL)
            .map(|row| SkippedDetail { line: row.line, reason: row.reason.clone() })
            .collect(),
        headers: table.headers.clone(),
    };

    for row in read.rows {
        // A category the statement names and the user has is kept; anything
        // else is left for the rules to decide, with `Needs review` behind
        // them.
        let category_id = row.category.as_deref().and_then(|name| {
            lookups
                .categories
                .iter()
                .find(|c| c.deleted_at.is_none() && c.name.eq_ignore_ascii_case(name.trim()))
                .map(|c| c.id)
        });

        // Likewise the account: the name the bank used, matched against the
        // accounts that exist. An unknown name is left unset rather than
        // creating an account the user never asked for.
        let account_id = row
            .account
            .as_deref()
            .and_then(|name| lookups.live_account_named(name))
            .map(|account| account.id);

        let payload = json!({
            "date": row.date,
            "merchant": row.merchant,
            "amount_cents": row.amount_cents,
            "kind": if row.is_income { "income" } else { "expense" },
            "category_id": category_id,
            "account_id": account_id,
            "source": "csv",
            "receipt": false,
        });

        match insert_one(&state, &lookups, &rules, payload, fallback).await {
            Ok(Outcome::Inserted(transaction)) => {
                report.inserted += 1;
                if transaction.category_id == fallback && fallback.is_some() {
                    report.needs_review += 1;
                }
            }
            Ok(Outcome::Duplicate { .. }) => report.duplicates += 1,
            // A row the server refuses is a skipped row, not a failed import:
            // the rest of the file is still worth having.
            Err(ApiError::BadRequest(message)) => {
                report.skipped += 1;
                if report.skipped_detail.len() < MAX_SKIPPED_DETAIL {
                    report.skipped_detail.push(SkippedDetail { line: row.line, reason: message });
                }
            }
            Err(other) => return Err(other),
        }
    }

    tracing::info!(
        inserted = report.inserted,
        duplicates = report.duplicates,
        skipped = report.skipped,
        "imported a CSV statement"
    );

    // 201 only when something new was written. A file that was entirely
    // duplicates succeeded, but it created nothing.
    let status = if report.inserted > 0 { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(report)).into_response())
}

/// Turns the detected plan plus the client's answers into a settled reading,
/// or refuses with the questions still outstanding.
fn settle(
    table: &Table,
    options: &Options,
) -> Result<(Mapping, DateFormat, AmountSource), ApiError> {
    let detected = myledger_core::csv::plan(table);
    let width = table.headers.len();

    let check = |index: Option<usize>, what: &str| -> Result<Option<usize>, ApiError> {
        match index {
            Some(i) if i >= width => Err(ApiError::bad_request(format!(
                "the {what} column is given as {i}, but the file has {width} columns"
            ))),
            other => Ok(other),
        }
    };

    let mapping = Mapping {
        date: check(options.columns.date, "date")?.or(detected.mapping.date),
        merchant: check(options.columns.merchant, "merchant")?.or(detected.mapping.merchant),
        category: check(options.columns.category, "category")?.or(detected.mapping.category),
        account: check(options.columns.account, "account")?.or(detected.mapping.account),
    };

    // ---- the date order ------------------------------------------------
    let date_format = match options.date_format.as_deref() {
        None => match detected.date {
            DateGuess::Certain(format) => format,
            DateGuess::Ambiguous => {
                return Err(ambiguous(
                    "dates in this file could be day-first or month-first",
                    &detected.questions(),
                ));
            }
            DateGuess::Unreadable => {
                return Err(ambiguous(
                    "the date column could not be read",
                    &detected.questions(),
                ));
            }
        },
        Some("iso") => DateFormat::Iso,
        Some("day-first") => DateFormat::DayFirst,
        Some("month-first") => DateFormat::MonthFirst,
        Some("named") => DateFormat::Named,
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "date_format {other:?} is not one of iso, day-first, month-first, named"
            )));
        }
    };

    if mapping.date.is_none() {
        return Err(ambiguous("no date column was found", &detected.questions()));
    }

    // ---- where the amount comes from -----------------------------------
    let columns = &options.columns;
    let amount_source = if let (Some(debit), Some(credit)) =
        (check(columns.debit, "debit")?, check(columns.credit, "credit")?)
    {
        AmountSource::DebitCredit { debit, credit }
    } else if let (Some(amount), Some(kind)) =
        (check(columns.amount, "amount")?, check(columns.kind, "kind")?)
    {
        AmountSource::Labelled { amount, kind }
    } else {
        match (&detected.amount, columns.amount, options.direction.as_deref()) {
            // An explicitly chosen amount column with a stated direction.
            (_, Some(amount), Some(direction)) => AmountSource::Fixed {
                amount,
                is_income: read_direction_word(direction)?,
            },
            (AmountGuess::NeedsDirection { amount }, _, Some(direction)) => {
                AmountSource::Fixed { amount: *amount, is_income: read_direction_word(direction)? }
            }
            (AmountGuess::Certain(source), None, _) => *source,
            // A chosen column with no direction: the sign in the file decides.
            (_, Some(amount), None) => AmountSource::Signed { amount },
            (AmountGuess::NeedsDirection { .. }, _, None) => {
                return Err(ambiguous(
                    "every amount in this file is positive and nothing says which way the money \
                     went",
                    &detected.questions(),
                ));
            }
            (AmountGuess::Unreadable, _, _) => {
                return Err(ambiguous("no amount column was found", &detected.questions()));
            }
        }
    };

    Ok((mapping, date_format, amount_source))
}

fn read_direction_word(value: &str) -> Result<bool, ApiError> {
    match value.trim().to_lowercase().as_str() {
        "income" | "credit" | "in" => Ok(true),
        "expense" | "debit" | "out" => Ok(false),
        other => Err(ApiError::bad_request(format!(
            "direction {other:?} is not one of expense, income"
        ))),
    }
}

/// A refusal that carries what needs answering.
///
/// A 400 rather than a success with guessed values: an import that quietly
/// picked a date order would report a clean result and leave the user with
/// months of transactions on the wrong days.
fn ambiguous(summary: &str, questions: &[String]) -> ApiError {
    let listed = questions.join(" ");
    ApiError::bad_request(format!("{summary}. {listed}"))
}

/// Reads the file and the options out of the multipart body.
async fn collect(mut multipart: Multipart) -> Result<(Vec<u8>, Options), ApiError> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut options = Options::default();
    let mut options_seen = false;

    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.file_name().is_some() {
            if bytes.is_some() {
                return Err(ApiError::bad_request("send one CSV file, not several"));
            }
            // Counted as it streams, so an oversized file is refused without
            // ever being held in full.
            let mut collected: Vec<u8> = Vec::new();
            loop {
                match field.chunk().await {
                    Ok(Some(chunk)) => {
                        if collected.len() as u64 + chunk.len() as u64 > MAX_CSV_BYTES {
                            return Err(ApiError::too_large(format!(
                                "the file is larger than the {} MiB limit",
                                MAX_CSV_BYTES / (1024 * 1024)
                            )));
                        }
                        collected.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(multipart_error(e)),
                }
            }
            bytes = Some(collected);
            continue;
        }

        let name = field.name().unwrap_or_default().to_string();
        let value = field.text().await.map_err(multipart_error)?;
        if name == "options" && !value.trim().is_empty() {
            options = serde_json::from_str(&value).map_err(|e| {
                ApiError::bad_request(format!("could not read the import options: {e}"))
            })?;
            options_seen = true;
        }
    }

    let Some(bytes) = bytes else {
        return Err(ApiError::bad_request("no CSV file in the upload"));
    };
    if bytes.is_empty() {
        return Err(ApiError::bad_request("the uploaded file is empty"));
    }
    let _ = options_seen;

    Ok((bytes, options))
}

fn multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::too_large(format!(
            "the upload is larger than the {} MiB limit",
            MAX_CSV_BYTES / (1024 * 1024)
        ));
    }
    ApiError::bad_request(format!("could not read the upload: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myledger_core::csv::read_csv;

    fn table(csv: &str) -> Table {
        read_csv(csv).unwrap()
    }

    #[test]
    fn a_tidy_file_settles_without_any_options() {
        let table = table("Date,Description,Amount\n2026-10-01,Shop,-12.50\n");
        let (mapping, format, amount) = settle(&table, &Options::default()).unwrap();
        assert_eq!(mapping.date, Some(0));
        assert_eq!(mapping.merchant, Some(1));
        assert_eq!(format, DateFormat::Iso);
        assert_eq!(amount, AmountSource::Signed { amount: 2 });
    }

    #[test]
    fn an_ambiguous_date_order_is_refused_rather_than_guessed() {
        let table = table("Date,Description,Amount\n03/04/2026,Shop,-12.50\n");
        let error = settle(&table, &Options::default()).unwrap_err();
        let message = format!("{error:?}");
        assert!(message.contains("day-first"), "{message}");
    }

    #[test]
    fn being_told_the_date_order_settles_it() {
        let table = table("Date,Description,Amount\n03/04/2026,Shop,-12.50\n");
        let options =
            Options { date_format: Some("month-first".into()), ..Default::default() };
        let (_, format, _) = settle(&table, &options).unwrap();
        assert_eq!(format, DateFormat::MonthFirst);
    }

    #[test]
    fn an_all_positive_file_is_refused_until_the_direction_is_given() {
        let table = table("Date,Description,Amount\n2026-10-01,Shop,12.50\n");
        let error = settle(&table, &Options::default()).unwrap_err();
        assert!(format!("{error:?}").contains("which way"), "{error:?}");

        let options = Options { direction: Some("expense".into()), ..Default::default() };
        let (_, _, amount) = settle(&table, &options).unwrap();
        assert_eq!(amount, AmountSource::Fixed { amount: 2, is_income: false });
    }

    #[test]
    fn a_nonsense_direction_is_refused() {
        let table = table("Date,Description,Amount\n2026-10-01,Shop,12.50\n");
        let options = Options { direction: Some("sideways".into()), ..Default::default() };
        assert!(settle(&table, &options).is_err());
    }

    #[test]
    fn a_nonsense_date_format_is_refused() {
        let table = table("Date,Description,Amount\n2026-10-01,Shop,-12.50\n");
        let options = Options { date_format: Some("yyyy".into()), ..Default::default() };
        assert!(settle(&table, &options).is_err());
    }

    #[test]
    fn explicit_columns_override_what_was_detected() {
        // Two plausible description columns; the user picks the other one.
        let table = table("Date,Memo,Payee,Amount\n2026-10-01,note,Shop,-1.00\n");
        let options = Options {
            columns: Columns { merchant: Some(2), ..Default::default() },
            ..Default::default()
        };
        let (mapping, _, _) = settle(&table, &options).unwrap();
        assert_eq!(mapping.merchant, Some(2));
    }

    #[test]
    fn a_column_index_past_the_end_is_refused() {
        let table = table("Date,Description,Amount\n2026-10-01,Shop,-1.00\n");
        let options = Options {
            columns: Columns { date: Some(9), ..Default::default() },
            ..Default::default()
        };
        let error = settle(&table, &options).unwrap_err();
        assert!(format!("{error:?}").contains("3 columns"), "{error:?}");
    }

    #[test]
    fn explicit_debit_and_credit_columns_are_honoured() {
        let table = table("Date,Note,Out,In\n2026-10-01,Shop,1.00,\n");
        let options = Options {
            columns: Columns { debit: Some(2), credit: Some(3), ..Default::default() },
            ..Default::default()
        };
        let (_, _, amount) = settle(&table, &options).unwrap();
        assert_eq!(amount, AmountSource::DebitCredit { debit: 2, credit: 3 });
    }

    #[test]
    fn an_explicit_amount_and_kind_column_are_honoured() {
        let table = table("Date,Note,Value,Flag\n2026-10-01,Shop,1.00,D\n");
        let options = Options {
            columns: Columns { amount: Some(2), kind: Some(3), ..Default::default() },
            ..Default::default()
        };
        let (_, _, amount) = settle(&table, &options).unwrap();
        assert_eq!(amount, AmountSource::Labelled { amount: 2, kind: 3 });
    }

    #[test]
    fn a_file_with_no_recognisable_columns_is_refused_with_questions() {
        let table = table("Col1,Col2,Col3\na,b,c\n");
        let error = settle(&table, &Options::default()).unwrap_err();
        let message = format!("{error:?}");
        assert!(message.contains("column"), "{message}");
    }

    #[test]
    fn unknown_option_fields_are_refused_rather_than_ignored() {
        // A typo in an option should fail loudly, not silently change nothing.
        assert!(serde_json::from_str::<Options>(r#"{"dateformat":"iso"}"#).is_err());
        assert!(serde_json::from_str::<Options>(r#"{"columns":{"dates":0}}"#).is_err());
        assert!(serde_json::from_str::<Options>(r#"{"date_format":"iso"}"#).is_ok());
    }
}
