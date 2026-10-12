//! `GET /api/v1/state` — everything a client needs, in one response.
//!
//! Clients are offline-tolerant: the phone and the web app each keep a local
//! copy of the ledger and reconcile against this. One large read beats a dozen
//! small ones, and it means a client can never hold a half-refreshed view where
//! transactions have been updated but the categories they name have not.
//!
//! What is deliberately absent: file bytes, storage paths, password hashes and
//! token secrets. The first two are excluded here; the last two cannot appear,
//! because the types holding them do not implement `Serialize` at all.

use axum::Json;
use axum::extract::State;
use myledger_core::{StateResponse, TagUsage, now_timestamp};
use myledger_db::lookups::{accounts, categories, tags};
use myledger_db::{
    budgets, dismissed, documents as db_documents, goals, recurring_items, rules, seed, settings,
    extraction as db_extraction, sms_rules, transactions, wipe as db_wipe,
};

use crate::AppState;
use crate::api::documents;
use crate::error::ApiError;
use crate::extract::ApiJson;

/// Most transactions a single `/state` will return.
///
/// A ledger larger than this is real but rare, and a client that needs the tail
/// of it is doing history rather than day-to-day work. Capping keeps the
/// response bounded.
pub const TRANSACTION_LIMIT: i64 = 5_000;

/// Most recent documents returned. Metadata only.
pub const DOCUMENT_LIMIT: i64 = 100;

// `StateResponse` itself lives in `myledger_core::wire`: the web and mobile
// apps deserialise it, so a field renamed here has to stop compiling there
// rather than quietly arriving as `None`.

/// Reads the whole ledger.
pub async fn get_state(State(state): State<AppState>) -> Result<Json<StateResponse>, ApiError> {
    let db = &state.db;

    let transactions = transactions::list_transactions(db, TRANSACTION_LIMIT).await?;
    let transactions_truncated = transactions.len() as i64 == TRANSACTION_LIMIT;

    Ok(Json(StateResponse {
        transactions,
        transactions_truncated,
        categories: categories::list_all_categories(db).await?,
        accounts: accounts::list_all_accounts(db).await?,
        tags: tags::list_all_tags(db).await?,
        tag_usage: tags::tag_usage(db)
            .await?
            .into_iter()
            .map(|(tag_id, transactions)| TagUsage { tag_id, transactions })
            .collect(),
        rules: rules::list_rules(db).await?,
        sms_parse_rules: sms_rules::list_sms_rules(db).await?,
        document_extraction_rules: db_extraction::list_rules(db).await?,
        budgets: budgets::list_budgets(db).await?,
        goals: goals::list_goals(db).await?,
        recurring_items: recurring_items::list_recurring_items(db).await?,
        dismissed_patterns: dismissed::list_dismissed_patterns(db).await?,
        documents: db_documents::list_recent_documents(db, DOCUMENT_LIMIT).await?,
        settings: settings::load_settings(db).await?,
    }))
}

// The wipe's confirmation phrase and its request and response shapes live in
// `myledger_core::wire`: the web app has to send exactly the phrase this
// endpoint checks for, and a copy of that string on each side is precisely
// the sort of thing that drifts. Re-exported here so the route and its tests
// still name them where they are used.
pub use myledger_core::wire::{
    DeletedCounts, KeptSummary, WIPE_CONFIRMATION, WipeRequest, WipeResponse,
};

/// Erases every financial record.
///
/// Irreversible, and there is no undo anywhere in the app — so the guard is a
/// literal phrase rather than a flag, and a request that fails the check
/// touches nothing at all.
pub async fn wipe_state(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<WipeRequest>,
) -> Result<Json<WipeResponse>, ApiError> {
    if request.confirm != WIPE_CONFIRMATION {
        return Err(ApiError::bad_request(format!(
            "nothing has been changed. To erase every transaction, budget, goal, \
             rule and document, send exactly {{\"confirm\": \"{WIPE_CONFIRMATION}\"}}"
        )));
    }

    let db = &state.db;

    // One transaction: either all of it goes or none of it does.
    let wiped = db_wipe::wipe_financial_data(db).await?;

    // Files only once the rows are committed. A file removed before the commit
    // could be orphaned by a rollback, with a row still pointing at nothing.
    let mut document_files_removed = 0;
    for document in &wiped.documents {
        if documents::remove_file(&state.config.data_dir, &document.storage_path).await {
            document_files_removed += 1;
        }
    }

    // Refills categories and accounts only if the user had emptied them, and
    // puts back any settings row that had been deleted outright.
    let restored = seed::seed_starter_data(db).await?;
    // Then force the four the wipe is meant to reset, which the seed leaves
    // alone precisely because it must not undo a user's choices.
    seed::reset_settings_to_defaults(db).await?;

    let wipe_reset_at = now_timestamp();
    settings::set_setting(db, settings::keys::WIPE_RESET_AT, &wipe_reset_at).await?;

    let counts = wiped.counts;
    tracing::warn!(rows = counts.total(), files = document_files_removed, "wiped all financial data");

    Ok(Json(WipeResponse {
        wiped: true,
        deleted: DeletedCounts {
            transactions: counts.transactions,
            transaction_tags: counts.transaction_tags,
            tags: counts.tags,
            documents: counts.documents,
            document_extractions: counts.document_extractions,
            rules: counts.rules,
            sms_parse_rules: counts.sms_parse_rules,
            document_extraction_rules: counts.document_extraction_rules,
            budgets: counts.budgets,
            goals: counts.goals,
            recurring_items: counts.recurring_items,
            dismissed_patterns: counts.dismissed_patterns,
            total: counts.total(),
        },
        document_files_removed,
        starter_categories_restored: restored.categories_inserted,
        starter_accounts_restored: restored.accounts_inserted,
        kept: KeptSummary {
            categories: categories::count_categories(db).await?,
            accounts: accounts::count_accounts(db).await?,
            signed_in: true,
        },
        wipe_reset_at,
    }))
}
