//! Document extraction: the rules, and running them.
//!
//! The rules are stored and validated here; the matching itself is
//! `core::textrules`, shared with SMS rule authoring so that one engine
//! decides what a pattern means everywhere in the app.
//!
//! What running an extraction will and will not do:
//!
//! - Text is read from the file by `crate::text`, which handles PDFs with a
//!   text layer and plain text, and plainly refuses the rest. There is no OCR.
//! - A transaction is created **only** when exactly one rule matches and the
//!   reading is complete. Anything else — no text, no match, or two rules
//!   disagreeing — leaves the document at `review` for the user to fill in.
//! - Only the extracted text is stored, never the fields read from it. Rules
//!   change; a stored reading would go stale the moment one did.
//!
//! An extraction can be re-run after writing a new rule, and doing so is the
//! normal way to use this: upload, see the text, write a pattern against it,
//! run it again.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use myledger_core::textrules::{Candidate, Context, MatchOutcome, RuleView, check_rule, match_text};
use myledger_core::{
    DocumentExtractionRule, DocumentStatus, SmsKindHint, now_timestamp,
};
use myledger_db::{documents as db_documents, extraction as db_extraction, rules as db_rules};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

// The responses live in `core::wire`: the web app deserialises them, so one
// definition is what keeps the two sides from drifting apart.
pub use myledger_core::wire::{ExtractionResponse, StoredExtractionResponse};

use crate::AppState;
use crate::api::{Lookups, clean_name};
use crate::api::transactions::{Outcome, insert_one};
use crate::error::ApiError;
use crate::extract::{ApiJson, optional_json};
use crate::text::{self, TextOutcome};

/// What the user sends to create an extraction rule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRuleRequest {
    pub label: String,
    #[serde(default)]
    pub filename_pattern: Option<String>,
    pub body_regex: String,
    #[serde(default)]
    pub amount_group: Option<i64>,
    #[serde(default)]
    pub merchant_group: Option<i64>,
    #[serde(default)]
    pub date_group: Option<i64>,
    #[serde(default)]
    pub kind_hint: Option<SmsKindHint>,
    #[serde(default)]
    pub account_hint: Option<String>,
    #[serde(default)]
    pub default_category_id: Option<Uuid>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// A change to an existing rule. Absent fields are left alone.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRuleRequest {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub filename_pattern: Option<Option<String>>,
    #[serde(default)]
    pub body_regex: Option<String>,
    #[serde(default)]
    pub amount_group: Option<Option<i64>>,
    #[serde(default)]
    pub merchant_group: Option<Option<i64>>,
    #[serde(default)]
    pub date_group: Option<Option<i64>>,
    #[serde(default)]
    pub kind_hint: Option<Option<SmsKindHint>>,
    #[serde(default)]
    pub account_hint: Option<Option<String>>,
    #[serde(default)]
    pub default_category_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn yes() -> bool {
    true
}

pub async fn list_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<DocumentExtractionRule>>, ApiError> {
    Ok(Json(db_extraction::list_rules(&state.db).await?))
}

pub async fn get_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DocumentExtractionRule>, ApiError> {
    db_extraction::get_rule(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no extraction rule with id {id}")))
}

pub async fn create_rule(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<NewRuleRequest>,
) -> Result<Response, ApiError> {
    let now = now_timestamp();
    let rule = DocumentExtractionRule {
        id: Uuid::new_v4(),
        label: clean_name(&request.label, "label")?,
        filename_pattern: blank_to_none(request.filename_pattern),
        body_regex: request.body_regex.trim().to_string(),
        amount_group: request.amount_group,
        merchant_group: request.merchant_group,
        date_group: request.date_group,
        kind_hint: request.kind_hint,
        account_hint: blank_to_none(request.account_hint),
        default_category_id: request.default_category_id,
        enabled: request.enabled,
        created_at: now.clone(),
        updated_at: now,
    };

    validate(&state, &rule).await?;
    db_extraction::insert_rule(&state.db, &rule).await?;
    Ok((StatusCode::CREATED, Json(rule)).into_response())
}

pub async fn patch_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<DocumentExtractionRule>, ApiError> {
    let request: PatchRuleRequest = optional_json(&body)?;

    let Some(mut rule) = db_extraction::get_rule(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no extraction rule with id {id}")));
    };

    if let Some(label) = request.label {
        rule.label = clean_name(&label, "label")?;
    }
    if let Some(pattern) = request.filename_pattern {
        rule.filename_pattern = blank_to_none(pattern);
    }
    if let Some(regex) = request.body_regex {
        rule.body_regex = regex.trim().to_string();
    }
    if let Some(group) = request.amount_group {
        rule.amount_group = group;
    }
    if let Some(group) = request.merchant_group {
        rule.merchant_group = group;
    }
    if let Some(group) = request.date_group {
        rule.date_group = group;
    }
    if let Some(hint) = request.kind_hint {
        rule.kind_hint = hint;
    }
    if let Some(hint) = request.account_hint {
        rule.account_hint = blank_to_none(hint);
    }
    if let Some(category) = request.default_category_id {
        rule.default_category_id = category;
    }
    if let Some(enabled) = request.enabled {
        rule.enabled = enabled;
    }
    rule.updated_at = now_timestamp();

    validate(&state, &rule).await?;
    if !db_extraction::update_rule(&state.db, &rule).await? {
        return Err(ApiError::not_found(format!("no extraction rule with id {id}")));
    }
    Ok(Json(rule))
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
    pub id: Uuid,
}

pub async fn delete_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    if !db_extraction::delete_rule(&state.db, id).await? {
        return Err(ApiError::not_found(format!("no extraction rule with id {id}")));
    }
    Ok(Json(DeleteResponse { deleted: true, id }))
}

/// Refuses a rule that could never work.
///
/// Checked when the rule is written rather than when it fails to match
/// anything, because "my rule does nothing" is a miserable thing to debug.
async fn validate(state: &AppState, rule: &DocumentExtractionRule) -> Result<(), ApiError> {
    check_rule(&rule.as_view()).map_err(|problem| ApiError::bad_request(problem.to_string()))?;

    if let Some(category_id) = rule.default_category_id {
        Lookups::load(&state.db).await?.check_category(category_id)?;
    }
    Ok(())
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ---- running an extraction -------------------------------------------------

/// `POST /api/v1/documents/{id}/extract`.
pub async fn extract_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ExtractionResponse>, ApiError> {
    let Some(mut document) = db_documents::get_document(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no document with id {id}")));
    };

    let path = state.config.data_dir.join(&document.storage_path);
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        ApiError::internal(format!("could not read the stored file for {id}: {e}"))
    })?;

    // Reading a PDF is CPU-bound and can take a moment on a large one, so it
    // goes to a blocking thread rather than stalling the async runtime.
    let filename = document.filename.clone();
    let mime = document.mime_type.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        text::read_text(&filename, mime.as_deref(), &bytes)
    })
    .await
    .map_err(|e| ApiError::internal(format!("reading {id} failed: {e}")))?;

    db_extraction::save_extraction(&state.db, id, outcome.text(), outcome.note()).await?;

    let TextOutcome::Text(body) = &outcome else {
        // No text at all. The document waits for the user.
        document = set_status(&state, document, DocumentStatus::Review).await?;
        return Ok(Json(ExtractionResponse {
            document,
            outcome: "no-text".to_string(),
            note: outcome.note().map(str::to_string),
            text: None,
            candidates: Vec::new(),
            created_transaction: None,
            rules_tried: 0,
        }));
    };

    let rules = db_extraction::list_rules(&state.db).await?;
    let views: Vec<RuleView<'_>> = rules.iter().map(DocumentExtractionRule::as_view).collect();
    let rules_tried = views.iter().filter(|rule| rule.enabled).count();

    // The upload date stands in when a rule captures no date. It is the only
    // date actually known about the file, and the response says whether the
    // date was captured or assumed.
    let fallback_date = document.created_at.get(..10).unwrap_or("").to_string();
    let context = Context { source: &document.filename, fallback_date: &fallback_date };
    let matched = match_text(&views, body, &context);

    let (label, candidates) = match &matched {
        MatchOutcome::NoMatch => ("no-match", Vec::new()),
        MatchOutcome::Matched(candidate) => ("matched", vec![candidate.clone()]),
        MatchOutcome::Ambiguous(all) => ("ambiguous", all.clone()),
    };

    // Only an unambiguous reading becomes a transaction. Everything else is
    // the user's to decide.
    let mut created = None;
    if let MatchOutcome::Matched(candidate) = &matched {
        if document.linked_transaction_id.is_some() {
            // Already linked by an earlier run, or by hand. Re-running must
            // not create a second transaction for the same receipt.
            document = set_status(&state, document, DocumentStatus::Stored).await?;
        } else {
            match create_from(&state, candidate).await? {
                Created::Transaction(transaction) => {
                    document.linked_transaction_id = Some(transaction.id);
                    document.status = DocumentStatus::Stored;
                    db_documents::update_document(&state.db, &document).await?;
                    created = Some(*transaction);
                }
                Created::AlreadyPresent(existing_id) => {
                    // The transaction was already in the ledger — entered by
                    // hand, or imported. Linking the receipt to it is the
                    // right outcome; creating a duplicate is not.
                    document.linked_transaction_id = Some(existing_id);
                    document.status = DocumentStatus::Stored;
                    db_documents::update_document(&state.db, &document).await?;
                }
            }
        }
    } else {
        document = set_status(&state, document, DocumentStatus::Review).await?;
    }

    Ok(Json(ExtractionResponse {
        document,
        outcome: label.to_string(),
        note: None,
        text: Some(body.clone()),
        candidates,
        created_transaction: created,
        rules_tried,
    }))
}

/// `GET /api/v1/documents/{id}/extraction` — what the last read found.
pub async fn get_extraction(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<StoredExtractionResponse>, ApiError> {
    let Some(document) = db_documents::get_document(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no document with id {id}")));
    };
    let Some(stored) = db_extraction::get_extraction(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("document {id} has not been read yet")));
    };

    let (outcome, candidates) = match &stored.extracted_text {
        None => ("no-text", Vec::new()),
        Some(body) => {
            let rules = db_extraction::list_rules(&state.db).await?;
            let views: Vec<RuleView<'_>> =
                rules.iter().map(DocumentExtractionRule::as_view).collect();
            let fallback_date = document.created_at.get(..10).unwrap_or("").to_string();
            let context = Context { source: &document.filename, fallback_date: &fallback_date };
            match match_text(&views, body, &context) {
                MatchOutcome::NoMatch => ("no-match", Vec::new()),
                MatchOutcome::Matched(candidate) => ("matched", vec![candidate]),
                MatchOutcome::Ambiguous(all) => ("ambiguous", all),
            }
        }
    };

    Ok(Json(StoredExtractionResponse {
        document_id: stored.document_id,
        text: stored.extracted_text,
        note: stored.note,
        read_at: stored.created_at,
        outcome: outcome.to_string(),
        candidates,
    }))
}

enum Created {
    Transaction(Box<myledger_core::Transaction>),
    /// The same purchase was already on file.
    AlreadyPresent(Uuid),
}

/// Writes the transaction a candidate describes, through the shared path.
async fn create_from(state: &AppState, candidate: &Candidate) -> Result<Created, ApiError> {
    let lookups = Lookups::load(&state.db).await?;
    let rules = db_rules::list_rules(&state.db).await?;

    let account_id = candidate
        .account_hint
        .as_deref()
        .and_then(|name| lookups.live_account_named(name))
        .map(|account| account.id);

    let payload = json!({
        "date": candidate.date,
        "merchant": candidate.merchant,
        "amount_cents": candidate.amount_cents,
        "kind": candidate.kind.as_str(),
        "category_id": candidate.category_id,
        "account_id": account_id,
        "source": "document",
        // The document being extracted *is* the receipt.
        "receipt": true,
    });

    // Through `insert_one`, like every other insert path, so a document
    // extraction cannot create a duplicate the import would have caught.
    match insert_one(state, &lookups, &rules, payload, None).await? {
        Outcome::Inserted(transaction) => Ok(Created::Transaction(transaction)),
        Outcome::Duplicate { existing_id, .. } => Ok(Created::AlreadyPresent(existing_id)),
    }
}

/// Saves a document's status, if it isn't already that.
async fn set_status(
    state: &AppState,
    mut document: myledger_core::Document,
    status: DocumentStatus,
) -> Result<myledger_core::Document, ApiError> {
    if document.status == status {
        return Ok(document);
    }
    document.status = status;
    db_documents::update_document(&state.db, &document).await?;
    Ok(document)
}
