//! App settings, stored as key/value text.
//!
//! A key/value table rather than a one-row settings table, so that writing one
//! preference physically cannot disturb another — a property the preferences
//! API depends on.
//!
//! [`load_settings`] decodes the rows into a typed [`Settings`]; anything
//! missing or unreadable falls back to its default rather than failing, because
//! a single bad row should not stop the app from opening. Values are validated
//! on the way in, so that fallback should never fire in practice.

use std::collections::BTreeMap;

use myledger_core::{Period, Settings, now_timestamp};

use crate::Db;
use crate::error::DbError;

/// The setting keys the app knows about.
pub mod keys {
    /// Which span of time the UI is showing. A [`Period`](myledger_core::Period) token.
    pub const SELECTED_PERIOD: &str = "selected_period";
    /// Whether the user has entered their assets and liabilities.
    pub const NET_WORTH_CONFIGURED: &str = "net_worth_configured";
    /// Total assets, in cents.
    pub const ASSETS_CENTS: &str = "assets_cents";
    /// Total liabilities, in cents.
    pub const LIABILITIES_CENTS: &str = "liabilities_cents";
    /// When the financial data was last wiped.
    pub const WIPE_RESET_AT: &str = "wipe_reset_at";
}

/// One setting, or `None` if it has never been written.
pub async fn get_setting(db: &Db, key: &str) -> Result<Option<String>, DbError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.map(|(value,)| value))
}

/// Writes one setting, leaving every other key alone.
pub async fn set_setting(db: &Db, key: &str, value: &str) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now_timestamp())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Writes a setting only if the key has never been set.
///
/// This is what makes seeding defaults idempotent: re-seeding will not undo a
/// choice the user has since made.
pub async fn set_setting_if_absent(db: &Db, key: &str, value: &str) -> Result<bool, DbError> {
    let result =
        sqlx::query("INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES (?, ?, ?)")
            .bind(key)
            .bind(value)
            .bind(now_timestamp())
            .execute(db.pool())
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes a setting, returning whether it was there.
pub async fn delete_setting(db: &Db, key: &str) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Every setting as raw text, keyed by name.
pub async fn all_settings(db: &Db) -> Result<BTreeMap<String, String>, DbError> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(db.pool())
        .await?;
    Ok(rows.into_iter().collect())
}

/// The settings, decoded.
pub async fn load_settings(db: &Db) -> Result<Settings, DbError> {
    let raw = all_settings(db).await?;
    let defaults = Settings::default();

    Ok(Settings {
        selected_period: raw
            .get(keys::SELECTED_PERIOD)
            .and_then(|value| value.parse::<Period>().ok())
            .unwrap_or(defaults.selected_period),
        net_worth_configured: raw
            .get(keys::NET_WORTH_CONFIGURED)
            .map(|value| value == "true" || value == "1")
            .unwrap_or(defaults.net_worth_configured),
        assets_cents: raw
            .get(keys::ASSETS_CENTS)
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.assets_cents),
        liabilities_cents: raw
            .get(keys::LIABILITIES_CENTS)
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.liabilities_cents),
        wipe_reset_at: raw.get(keys::WIPE_RESET_AT).cloned(),
    })
}

/// Writes every field of [`Settings`] back.
///
/// Callers wanting to change one thing should use [`set_setting`]; this is for
/// the wipe, which genuinely means to reset the lot.
pub async fn save_settings(db: &Db, settings: &Settings) -> Result<(), DbError> {
    set_setting(db, keys::SELECTED_PERIOD, settings.selected_period.as_str()).await?;
    set_setting(
        db,
        keys::NET_WORTH_CONFIGURED,
        if settings.net_worth_configured { "true" } else { "false" },
    )
    .await?;
    set_setting(db, keys::ASSETS_CENTS, &settings.assets_cents.to_string()).await?;
    set_setting(db, keys::LIABILITIES_CENTS, &settings.liabilities_cents.to_string()).await?;
    match &settings.wipe_reset_at {
        Some(stamp) => set_setting(db, keys::WIPE_RESET_AT, stamp).await?,
        None => {
            delete_setting(db, keys::WIPE_RESET_AT).await?;
        }
    }
    Ok(())
}
