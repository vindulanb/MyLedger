//! Writing to the ledger.
//!
//! One endpoint takes either a single transaction or a whole CSV import, and
//! reports on each row independently — an import of four hundred rows where two
//! are already present should not fail, it should say so.
//!
//! The order of operations inside [`insert_one`] is the part that matters:
//! validate, fingerprint, check for a duplicate, *then* apply categorisation
//! rules. Rules run last so that they can never influence what counts as a
//! duplicate.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use myledger_core::{
    Rule, Transaction, TransactionSource, now_timestamp, transaction_fingerprint,
    validate_new_transaction,
};
use myledger_db::transactions as db_transactions;
use myledger_db::{DbError, rules as db_rules};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::AppState;
use crate::api::{Lookups, explicit_option};
use crate::error::ApiError;
use crate::extract::ApiJson;

/// Most rows one request may carry. A guard against a runaway import, not a
/// limit anyone should meet in practice.
const MAX_BATCH: usize = 10_000;

/// One inbound transaction.
#[derive(Debug, Deserialize)]
pub struct NewTransactionRequest {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub merchant: String,
    /// Always positive; `kind` carries the direction.
    pub amount_cents: i64,
    /// `expense` or `income`.
    pub kind: String,

    #[serde(default)]
    pub category_id: Option<Uuid>,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    /// The account by name, for imports that know what the bank called it
    /// rather than which row it is. Ignored when `account_id` is given.
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub tags: Vec<Uuid>,
    #[serde(default)]
    pub receipt: bool,
    #[serde(default = "default_source")]
    pub source: TransactionSource,
    #[serde(default)]
    pub sms_hash: Option<String>,
}

fn default_source() -> TransactionSource {
    TransactionSource::Manual
}

/// What happened to one row of the request.
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ItemResult {
    /// Written to the ledger.
    Inserted { index: usize, id: Uuid },
    /// The ledger already holds this purchase. Not an error.
    Duplicate {
        index: usize,
        /// The transaction already on file.
        existing_id: Uuid,
        fingerprint: String,
    },
    /// Failed validation and was skipped.
    Rejected { index: usize, message: String },
}

/// The report a client reads to find out what an import did.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub inserted: usize,
    pub duplicates: usize,
    pub rejected: usize,
    /// One entry per row of the request, in order.
    pub results: Vec<ItemResult>,
    /// The rows that were written, as stored.
    pub transactions: Vec<Transaction>,
}

/// Adds one transaction or a batch of them.
///
/// The body is either a single object or an array of them. The response shape
/// is the same either way; the status code reflects the outcome:
///
/// - `201` — everything was written.
/// - `200` — a mix, or nothing new because it was all already on file.
/// - `400` — nothing was written and at least one row was invalid.
pub async fn create_transactions(
    State(state): State<AppState>,
    ApiJson(payload): ApiJson<Value>,
) -> Result<Response, ApiError> {
    let items = match payload {
        Value::Array(items) => items,
        object @ Value::Object(_) => vec![object],
        _ => {
            return Err(ApiError::bad_request(
                "send a transaction object, or an array of them",
            ));
        }
    };

    if items.is_empty() {
        return Err(ApiError::bad_request("no transactions in the request"));
    }
    if items.len() > MAX_BATCH {
        return Err(ApiError::bad_request(format!(
            "{} transactions in one request; the limit is {MAX_BATCH}",
            items.len()
        )));
    }

    // Loaded once for the whole batch rather than per row: an import of a
    // thousand rows should not read the categories a thousand times.
    let lookups = Lookups::load(&state.db).await?;
    let rules = db_rules::list_rules(&state.db).await?;

    let mut report =
        ImportReport { inserted: 0, duplicates: 0, rejected: 0, results: Vec::new(), transactions: Vec::new() };

    for (index, item) in items.into_iter().enumerate() {
        match insert_one(&state, &lookups, &rules, item).await {
            Ok(Outcome::Inserted(transaction)) => {
                report.inserted += 1;
                report.results.push(ItemResult::Inserted { index, id: transaction.id });
                report.transactions.push(*transaction);
            }
            Ok(Outcome::Duplicate { existing_id, fingerprint }) => {
                report.duplicates += 1;
                report.results.push(ItemResult::Duplicate { index, existing_id, fingerprint });
            }
            // A row the user can fix is reported and the import carries on.
            Err(ApiError::BadRequest(message)) => {
                report.rejected += 1;
                report.results.push(ItemResult::Rejected { index, message });
            }
            // Anything else is our problem, not theirs, and stops the batch.
            Err(other) => return Err(other),
        }
    }

    let status = if report.rejected == 0 && report.duplicates == 0 {
        StatusCode::CREATED
    } else if report.inserted == 0 && report.rejected > 0 {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };

    Ok((status, Json(report)).into_response())
}

enum Outcome {
    /// Boxed: a `Transaction` dwarfs the other variant, and this is returned
    /// once per row of an import that may be thousands long.
    Inserted(Box<Transaction>),
    Duplicate { existing_id: Uuid, fingerprint: String },
}

/// Validates, de-duplicates and writes one row.
async fn insert_one(
    state: &AppState,
    lookups: &Lookups,
    rules: &[Rule],
    raw: Value,
) -> Result<Outcome, ApiError> {
    let request: NewTransactionRequest = serde_json::from_value(raw)
        .map_err(|e| ApiError::bad_request(format!("could not read the transaction: {e}")))?;

    let validated = validate_new_transaction(
        &request.date,
        &request.merchant,
        request.amount_cents,
        &request.kind,
    )
    .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // The account contributes to the fingerprint by name, so it has to be
    // resolved before duplicate detection rather than after.
    let (account_id, account_name) = resolve_account(lookups, &request)?;

    if let Some(category_id) = request.category_id {
        lookups.check_category(category_id)?;
    }
    lookups.check_tags(&request.tags)?;

    let date = validated.date_string();
    let fingerprint = transaction_fingerprint(
        &date,
        &validated.merchant,
        validated.amount_cents,
        &account_name,
    );

    // Pre-check, so an import can report a duplicate with the ID of the row it
    // clashed with. The database's UNIQUE constraint below is what actually
    // guarantees it; this only makes the answer more useful.
    if let Some(existing) =
        db_transactions::find_transaction_by_fingerprint(&state.db, &fingerprint).await?
    {
        return Ok(Outcome::Duplicate { existing_id: existing.id, fingerprint });
    }

    let now = now_timestamp();
    let mut transaction = Transaction {
        id: Uuid::new_v4(),
        date,
        merchant: validated.merchant,
        category_id: request.category_id,
        amount_cents: validated.amount_cents,
        kind: validated.kind,
        account_id,
        tags: request.tags,
        receipt: request.receipt,
        source: request.source,
        fingerprint: fingerprint.clone(),
        sms_hash: request.sms_hash,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
    };

    // Last, and only now. A rule changes the category and tags, neither of
    // which feeds the fingerprint — but running rules before the duplicate
    // check would mean a rule could decide what "the same purchase" is.
    apply_rules(rules, &mut transaction);

    match db_transactions::insert_transaction(&state.db, &transaction).await {
        Ok(()) => Ok(Outcome::Inserted(Box::new(transaction))),
        // Lost a race, or the same purchase appeared twice inside one batch.
        Err(DbError::DuplicateFingerprint) => {
            let existing_id =
                db_transactions::find_transaction_by_fingerprint(&state.db, &fingerprint)
                    .await?
                    .map(|t| t.id)
                    .unwrap_or_else(Uuid::nil);
            Ok(Outcome::Duplicate { existing_id, fingerprint })
        }
        Err(other) => Err(other.into()),
    }
}

/// Works out which account a transaction belongs to, and what it is called.
///
/// The name matters as much as the ID: it is what the fingerprint is built
/// from, so that the same purchase imported from two sources that identify the
/// account differently still collides.
fn resolve_account(
    lookups: &Lookups,
    request: &NewTransactionRequest,
) -> Result<(Option<Uuid>, String), ApiError> {
    if let Some(id) = request.account_id {
        lookups.check_account(id)?;
        let name = lookups.account_name(id).unwrap_or_default().to_string();
        return Ok((Some(id), name));
    }

    match request.account.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => match lookups.live_account_named(name) {
            Some(account) => Ok((Some(account.id), account.name.clone())),
            None => Err(ApiError::bad_request(format!(
                "no account called {name:?}; create it first under preferences"
            ))),
        },
        // No account at all is allowed. The fingerprint then has an empty
        // account component, which is consistent for every such transaction.
        None => Ok((None, String::new())),
    }
}

/// Applies categorisation rules to a transaction that is about to be written.
///
/// A rule fills a gap rather than overriding a decision: the category is only
/// set if the client didn't supply one, and the first matching rule wins.
/// Tags accumulate — every matching rule may contribute one.
fn apply_rules(rules: &[Rule], transaction: &mut Transaction) {
    let merchant = transaction.merchant.to_lowercase();

    for rule in rules.iter().filter(|rule| rule.enabled) {
        let needle = rule.when_text.trim().to_lowercase();
        // An empty rule would match everything, which is never what was meant.
        if needle.is_empty() || !merchant.contains(&needle) {
            continue;
        }

        if transaction.category_id.is_none()
            && let Some(category_id) = rule.then_category_id
        {
            transaction.category_id = Some(category_id);
        }
        if let Some(tag_id) = rule.then_tag_id
            && !transaction.tags.contains(&tag_id)
        {
            transaction.tags.push(tag_id);
        }
    }
}

/// A change to a transaction's filing.
///
/// Only the category and tags: the date, merchant and amount are what the
/// purchase *was*, and editing them would silently invalidate the fingerprint
/// that keeps the ledger free of duplicates. Correcting those means deleting
/// the row and adding the right one.
#[derive(Debug, Deserialize)]
pub struct PatchTransactionRequest {
    /// Absent leaves the category alone; `null` clears it.
    #[serde(default, deserialize_with = "explicit_option")]
    pub category_id: Option<Option<Uuid>>,
    /// Absent leaves the tags alone; a list replaces them wholesale.
    #[serde(default)]
    pub tags: Option<Vec<Uuid>>,
}

/// Re-files a transaction.
pub async fn patch_transaction(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    ApiJson(request): ApiJson<PatchTransactionRequest>,
) -> Result<Json<Transaction>, ApiError> {
    if request.category_id.is_none() && request.tags.is_none() {
        return Err(ApiError::bad_request(
            "nothing to update: send a category_id, tags, or both",
        ));
    }

    let Some(mut transaction) = db_transactions::get_transaction(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no transaction with id {id}")));
    };
    if transaction.deleted_at.is_some() {
        return Err(ApiError::not_found(format!("transaction {id} has been deleted")));
    }

    let lookups = Lookups::load(&state.db).await?;

    if let Some(category_id) = request.category_id {
        if let Some(category_id) = category_id {
            lookups.check_category(category_id)?;
        }
        transaction.category_id = category_id;
    }
    if let Some(tags) = request.tags {
        lookups.check_tags(&tags)?;
        transaction.tags = tags;
    }

    db_transactions::update_transaction(&state.db, &transaction).await?;

    // Read back rather than returning what we wrote, so the response carries
    // the stored `updated_at` and the tags exactly as the join table holds them.
    let saved = db_transactions::get_transaction(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::internal("the transaction vanished mid-update"))?;
    Ok(Json(saved))
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
    pub id: Uuid,
}

/// Removes a transaction.
///
/// A soft delete: the row stays, so its fingerprint stays taken and
/// re-importing the same statement will not quietly bring it back.
pub async fn delete_transaction(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    if !db_transactions::soft_delete_transaction(&state.db, id).await? {
        return Err(ApiError::not_found(format!("no live transaction with id {id}")));
    }
    Ok(Json(DeleteResponse { deleted: true, id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myledger_core::{TransactionKind, now_timestamp};

    fn rule(when_text: &str, category: Option<Uuid>, tag: Option<Uuid>, enabled: bool) -> Rule {
        let now = now_timestamp();
        Rule {
            id: Uuid::new_v4(),
            when_text: when_text.to_string(),
            then_category_id: category,
            then_tag_id: tag,
            enabled,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    fn transaction(merchant: &str) -> Transaction {
        let now = now_timestamp();
        Transaction {
            id: Uuid::new_v4(),
            date: "2026-03-04".to_string(),
            merchant: merchant.to_string(),
            category_id: None,
            amount_cents: 545,
            kind: TransactionKind::Expense,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Csv,
            fingerprint: "fp".to_string(),
            sms_hash: None,
            created_at: now.clone(),
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn a_matching_rule_fills_in_the_category_and_tag() {
        let category = Uuid::new_v4();
        let tag = Uuid::new_v4();
        let mut subject = transaction("STARBUCKS #1234");

        apply_rules(&[rule("starbucks", Some(category), Some(tag), true)], &mut subject);

        assert_eq!(subject.category_id, Some(category));
        assert_eq!(subject.tags, vec![tag]);
    }

    #[test]
    fn rules_never_override_a_category_the_client_chose() {
        let chosen = Uuid::new_v4();
        let mut subject = transaction("Starbucks");
        subject.category_id = Some(chosen);

        apply_rules(&[rule("starbucks", Some(Uuid::new_v4()), None, true)], &mut subject);

        assert_eq!(subject.category_id, Some(chosen), "an explicit choice wins");
    }

    #[test]
    fn the_first_matching_rule_wins_the_category_but_tags_accumulate() {
        let first_category = Uuid::new_v4();
        let (first_tag, second_tag) = (Uuid::new_v4(), Uuid::new_v4());
        let mut subject = transaction("Starbucks Reserve");

        apply_rules(
            &[
                rule("starbucks", Some(first_category), Some(first_tag), true),
                rule("reserve", Some(Uuid::new_v4()), Some(second_tag), true),
            ],
            &mut subject,
        );

        assert_eq!(subject.category_id, Some(first_category));
        assert_eq!(subject.tags, vec![first_tag, second_tag]);
    }

    #[test]
    fn disabled_and_empty_rules_do_nothing() {
        let mut subject = transaction("Starbucks");

        apply_rules(
            &[
                rule("starbucks", Some(Uuid::new_v4()), None, false),
                // An empty `when_text` would otherwise match every merchant.
                rule("   ", Some(Uuid::new_v4()), Some(Uuid::new_v4()), true),
            ],
            &mut subject,
        );

        assert_eq!(subject.category_id, None);
        assert!(subject.tags.is_empty());
    }

    #[test]
    fn rule_matching_ignores_case() {
        let category = Uuid::new_v4();
        let mut subject = transaction("whole foods market");

        apply_rules(&[rule("WHOLE FOODS", Some(category), None, true)], &mut subject);

        assert_eq!(subject.category_id, Some(category));
    }

    #[test]
    fn a_tag_is_not_added_twice() {
        let tag = Uuid::new_v4();
        let mut subject = transaction("Starbucks");
        subject.tags = vec![tag];

        apply_rules(&[rule("starbucks", None, Some(tag), true)], &mut subject);

        assert_eq!(subject.tags, vec![tag]);
    }
}
