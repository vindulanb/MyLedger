//! Recurring bills and subscriptions.
//!
//! Two halves that meet in the middle. Detection reads the transaction history
//! and *proposes*; nothing it produces is stored. The `recurring_items` table
//! holds only what the user has confirmed. This module is where a proposal
//! becomes a record — or is waved away.
//!
//! A pattern is therefore in exactly one of three states, and the endpoints
//! here keep them exclusive:
//!
//! - **suggested** — detection found it, the user hasn't answered.
//! - **kept** — a `recurring_items` row exists whose name normalises to the
//!   pattern key, so it stops being suggested.
//! - **ignored** — a `dismissed_patterns` row hides it.
//!
//! Keeping a pattern clears any dismissal, ignoring one that is already kept is
//! refused, and deleting a confirmed item lets it be suggested again. There is
//! no way to end up in two states at once.

use std::collections::HashSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{Months, NaiveDate, Utc};
use myledger_core::{
    Cadence, NameLookup, RecurringItem, RecurringKind, RecurringSuggestion, detect_recurring,
    normalize_merchant, now_timestamp,
};
use myledger_db::lookups::{categories, tags};
use myledger_db::{Db, dismissed, recurring_items, transactions as db_transactions};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;
use crate::api::{Lookups, clean_date, clean_name, clean_recurring_fields, explicit_option};
use crate::error::ApiError;
use crate::extract::{ApiJson, optional_json};

/// How far back detection reads.
///
/// Three years. An annual charge needs three occurrences before it can be
/// called high-confidence, which is two years of history on its own; the extra
/// year absorbs a late renewal without the pattern falling out of the window.
pub const LOOKBACK_MONTHS: u32 = 36;

// ---- suggestions -----------------------------------------------------------

/// The suggestions worth showing, and why the list might be short.
#[derive(Debug, Serialize)]
pub struct SuggestionsResponse {
    pub suggestions: Vec<RecurringSuggestion>,
    /// Patterns detection found but is not offering, because the user already
    /// answered for them. Reported so a UI can say "3 hidden" rather than
    /// leaving the user wondering where Netflix went.
    pub hidden_as_kept: usize,
    pub hidden_as_ignored: usize,
    /// The date detection treated as today. Everything here — the cadence
    /// windows, the staleness cutoff — is relative to it.
    pub as_of: String,
}

/// Proposes repeating charges the user hasn't yet ruled on.
pub async fn get_suggestions(
    State(state): State<AppState>,
) -> Result<Json<SuggestionsResponse>, ApiError> {
    let today = today();
    let all = detect(&state.db, today).await?;

    let ignored = dismissed_keys(&state.db).await?;
    let kept = confirmed_keys(&state.db).await?;

    let mut hidden_as_kept = 0;
    let mut hidden_as_ignored = 0;
    let mut suggestions = Vec::with_capacity(all.len());

    for suggestion in all {
        if kept.contains(&suggestion.pattern_key) {
            hidden_as_kept += 1;
        } else if ignored.contains(&suggestion.pattern_key) {
            hidden_as_ignored += 1;
        } else {
            suggestions.push(suggestion);
        }
    }

    Ok(Json(SuggestionsResponse {
        suggestions,
        hidden_as_kept,
        hidden_as_ignored,
        as_of: today.format("%Y-%m-%d").to_string(),
    }))
}

/// Runs detection over the recent history. Unfiltered — the caller decides what
/// to hide.
async fn detect(db: &Db, today: NaiveDate) -> Result<Vec<RecurringSuggestion>, ApiError> {
    let since = today
        .checked_sub_months(Months::new(LOOKBACK_MONTHS))
        .unwrap_or(today)
        .format("%Y-%m-%d")
        .to_string();

    let transactions = db_transactions::list_expenses_since(db, &since).await?;
    let names =
        NameLookup::from_records(&categories::list_all_categories(db).await?, &tags::list_all_tags(db).await?);

    Ok(detect_recurring(&transactions, &names, today))
}

/// The date detection is anchored to.
fn today() -> NaiveDate {
    Utc::now().date_naive()
}

async fn dismissed_keys(db: &Db) -> Result<HashSet<String>, ApiError> {
    Ok(dismissed::list_dismissed_patterns(db).await?.into_iter().collect())
}

/// The pattern keys already covered by a confirmed item.
///
/// Matched by running each item's name through the same normalisation detection
/// uses, so a confirmed "Netflix.com" suppresses the "netflix com" suggestion
/// without needing a second column to link them.
async fn confirmed_keys(db: &Db) -> Result<HashSet<String>, ApiError> {
    Ok(recurring_items::list_recurring_items(db)
        .await?
        .iter()
        .map(|item| normalize_merchant(&item.name))
        .filter(|key| !key.is_empty())
        .collect())
}

/// Optional edits a user may make while accepting a suggestion.
///
/// Anything left out is taken from what detection worked out. `kind` is
/// included because a person may know better than the hint lists — a "gym"
/// that is really a loan repayment.
#[derive(Debug, Default, Deserialize)]
pub struct KeepRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category_id: Option<Uuid>,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub amount_cents: Option<i64>,
    #[serde(default)]
    pub cadence: Option<Cadence>,
    #[serde(default)]
    pub next_date: Option<String>,
    #[serde(default)]
    pub kind: Option<RecurringKind>,
}

/// Accepts a suggestion, turning it into a confirmed item.
///
/// The suggestion is recomputed rather than taken on trust from the request:
/// which hint list matched decides whether this is a bill or a subscription,
/// and that is a judgement the server makes, not one a client asserts.
pub async fn keep_suggestion(
    State(state): State<AppState>,
    Path(pattern_key): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let pattern_key = normalize_merchant(&pattern_key);
    if pattern_key.is_empty() {
        return Err(ApiError::bad_request("that is not a pattern key"));
    }
    let request: KeepRequest = optional_json(&body)?;

    if confirmed_keys(&state.db).await?.contains(&pattern_key) {
        return Err(ApiError::conflict(format!(
            "{pattern_key:?} is already a confirmed recurring item"
        )));
    }

    // Detection runs unfiltered here, so a pattern the user previously ignored
    // can still be accepted — changing your mind should not need an undo first.
    let suggestion = detect(&state.db, today())
        .await?
        .into_iter()
        .find(|candidate| candidate.pattern_key == pattern_key)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "no current suggestion for {pattern_key:?}; add it by hand with POST /recurring-items"
            ))
        })?;

    let lookups = Lookups::load(&state.db).await?;

    let category_id = request.category_id.or(suggestion.category_id);
    let account_id = request.account_id.or(suggestion.account_id);
    // A suggestion may name a category or account that has since been deleted,
    // in which case the item is created without one rather than refused.
    let category_id = category_id.filter(|id| lookups.check_category(*id).is_ok());
    let account_id = account_id.filter(|id| lookups.check_account(*id).is_ok());

    let (name, next_date) = clean_recurring_fields(
        request.name.as_deref().unwrap_or(&suggestion.merchant),
        request.amount_cents.unwrap_or(suggestion.average_amount_cents),
        Some(request.next_date.as_deref().unwrap_or(&suggestion.next_expected_date)),
    )?;

    let now = now_timestamp();
    let item = RecurringItem {
        id: Uuid::new_v4(),
        name,
        category_id,
        amount_cents: request.amount_cents.unwrap_or(suggestion.average_amount_cents),
        cadence: request.cadence.unwrap_or(suggestion.cadence),
        next_date,
        account_id,
        kind: request.kind.unwrap_or(suggestion.kind),
        active: true,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
    };
    recurring_items::insert_recurring_item(&state.db, &item).await?;

    // Keeping is the stronger statement: it overrides an earlier dismissal so
    // the pattern cannot be both kept and ignored.
    dismissed::restore_pattern(&state.db, &pattern_key).await?;

    tracing::info!(pattern_key, kind = %item.kind, "confirmed a recurring item");
    Ok((StatusCode::CREATED, Json(item)))
}

#[derive(Debug, Serialize)]
pub struct IgnoreResponse {
    pub pattern_key: String,
    pub ignored: bool,
}

/// Hides a suggestion for good.
pub async fn ignore_suggestion(
    State(state): State<AppState>,
    Path(pattern_key): Path<String>,
) -> Result<Json<IgnoreResponse>, ApiError> {
    let pattern_key = normalize_merchant(&pattern_key);
    if pattern_key.is_empty() {
        return Err(ApiError::bad_request("that is not a pattern key"));
    }

    if confirmed_keys(&state.db).await?.contains(&pattern_key) {
        return Err(ApiError::conflict(format!(
            "{pattern_key:?} is a confirmed recurring item; delete it with \
             DELETE /recurring-items/{{id}} rather than ignoring it"
        )));
    }

    dismissed::dismiss_pattern(&state.db, &pattern_key).await?;
    Ok(Json(IgnoreResponse { pattern_key, ignored: true }))
}

// ---- confirmed items -------------------------------------------------------

/// A recurring item added by hand rather than accepted from a suggestion.
#[derive(Debug, Deserialize)]
pub struct NewRecurringItemRequest {
    pub name: String,
    #[serde(default)]
    pub category_id: Option<Uuid>,
    pub amount_cents: i64,
    pub cadence: Cadence,
    #[serde(default)]
    pub next_date: Option<String>,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    pub kind: RecurringKind,
    #[serde(default = "yes")]
    pub active: bool,
}

fn yes() -> bool {
    true
}

/// Every confirmed item, soonest charge first.
pub async fn list_items(State(state): State<AppState>) -> Result<Json<Vec<RecurringItem>>, ApiError> {
    Ok(Json(recurring_items::list_recurring_items(&state.db).await?))
}

/// One confirmed item.
pub async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RecurringItem>, ApiError> {
    match recurring_items::get_recurring_item(&state.db, id).await? {
        Some(item) if item.deleted_at.is_none() => Ok(Json(item)),
        _ => Err(ApiError::not_found(format!("no recurring item with id {id}"))),
    }
}

/// Adds an item the user knows about but detection hasn't seen — a bill that
/// has only been paid once, or one that is about to start.
pub async fn create_item(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<NewRecurringItemRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let lookups = Lookups::load(&state.db).await?;
    if let Some(id) = request.category_id {
        lookups.check_category(id)?;
    }
    if let Some(id) = request.account_id {
        lookups.check_account(id)?;
    }

    let (name, next_date) = clean_recurring_fields(
        &request.name,
        request.amount_cents,
        request.next_date.as_deref(),
    )?;

    // Two items for one merchant would double-count in every monthly total.
    let key = normalize_merchant(&name);
    if !key.is_empty() && confirmed_keys(&state.db).await?.contains(&key) {
        return Err(ApiError::conflict(format!("there is already a recurring item for {name:?}")));
    }

    let now = now_timestamp();
    let item = RecurringItem {
        id: Uuid::new_v4(),
        name,
        category_id: request.category_id,
        amount_cents: request.amount_cents,
        cadence: request.cadence,
        next_date,
        account_id: request.account_id,
        kind: request.kind,
        active: request.active,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
    };
    recurring_items::insert_recurring_item(&state.db, &item).await?;

    Ok((StatusCode::CREATED, Json(item)))
}

/// A change to a confirmed item. Every field is optional; the ones that can be
/// cleared take an explicit `null` to do it.
#[derive(Debug, Deserialize)]
pub struct PatchRecurringItemRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub category_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub amount_cents: Option<i64>,
    #[serde(default)]
    pub cadence: Option<Cadence>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub next_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub account_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub kind: Option<RecurringKind>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// Edits a confirmed item.
pub async fn patch_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    ApiJson(request): ApiJson<PatchRecurringItemRequest>,
) -> Result<Json<RecurringItem>, ApiError> {
    let Some(mut item) = recurring_items::get_recurring_item(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no recurring item with id {id}")));
    };
    if item.deleted_at.is_some() {
        return Err(ApiError::not_found(format!("recurring item {id} has been deleted")));
    }

    let lookups = Lookups::load(&state.db).await?;

    if let Some(name) = request.name {
        item.name = clean_name(&name, "recurring item")?;
    }
    if let Some(category_id) = request.category_id {
        if let Some(category_id) = category_id {
            lookups.check_category(category_id)?;
        }
        item.category_id = category_id;
    }
    if let Some(account_id) = request.account_id {
        if let Some(account_id) = account_id {
            lookups.check_account(account_id)?;
        }
        item.account_id = account_id;
    }
    if let Some(amount_cents) = request.amount_cents {
        if amount_cents <= 0 {
            return Err(ApiError::bad_request("a recurring charge must be more than zero"));
        }
        item.amount_cents = amount_cents;
    }
    if let Some(cadence) = request.cadence {
        item.cadence = cadence;
    }
    if let Some(next_date) = request.next_date {
        item.next_date = clean_date(next_date.as_deref())?;
    }
    if let Some(kind) = request.kind {
        item.kind = kind;
    }
    if let Some(active) = request.active {
        item.active = active;
    }

    recurring_items::update_recurring_item(&state.db, &item).await?;

    recurring_items::get_recurring_item(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::internal("the recurring item vanished mid-update"))
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
    pub id: Uuid,
    /// The pattern this item covered. Now that the item is gone, detection may
    /// offer it again — unless it is also on the ignored list.
    pub pattern_key: String,
}

/// Removes a confirmed item.
pub async fn delete_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let Some(item) = recurring_items::get_recurring_item(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no recurring item with id {id}")));
    };
    if !recurring_items::soft_delete_recurring_item(&state.db, id).await? {
        return Err(ApiError::not_found(format!("recurring item {id} has already been deleted")));
    }

    Ok(Json(DeleteResponse { deleted: true, id, pattern_key: normalize_merchant(&item.name) }))
}
