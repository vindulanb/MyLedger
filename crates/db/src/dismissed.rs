//! Suggestions the user has waved away.
//!
//! A row here is keyed by normalised merchant, so dismissing a pattern hides
//! that merchant for good — even if its price or cadence later shifts and
//! detection would otherwise offer it again as something new.

use myledger_core::now_timestamp;

use crate::Db;
use crate::error::DbError;

/// Every dismissed pattern key.
pub async fn list_dismissed_patterns(db: &Db) -> Result<Vec<String>, DbError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT pattern_key FROM dismissed_patterns ORDER BY pattern_key")
            .fetch_all(db.pool())
            .await?;
    Ok(rows.into_iter().map(|(key,)| key).collect())
}

/// Hides a pattern. Dismissing an already-dismissed pattern is a no-op rather
/// than an error, so a double-tap in the UI does no harm.
pub async fn dismiss_pattern(db: &Db, pattern_key: &str) -> Result<(), DbError> {
    sqlx::query("INSERT OR IGNORE INTO dismissed_patterns (pattern_key, created_at) VALUES (?, ?)")
        .bind(pattern_key)
        .bind(now_timestamp())
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Whether this pattern is currently hidden.
pub async fn is_pattern_dismissed(db: &Db, pattern_key: &str) -> Result<bool, DbError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT pattern_key FROM dismissed_patterns WHERE pattern_key = ?")
            .bind(pattern_key)
            .fetch_optional(db.pool())
            .await?;
    Ok(row.is_some())
}

/// Un-hides a pattern, returning whether it had been hidden.
pub async fn restore_pattern(db: &Db, pattern_key: &str) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM dismissed_patterns WHERE pattern_key = ?")
        .bind(pattern_key)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every dismissal. Used by the data wipe.
pub async fn delete_all_dismissed_patterns(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM dismissed_patterns").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
