//! Confirmed recurring bills and subscriptions.
//!
//! Only charges the user has accepted land here. Suggestions are recomputed
//! from the transaction history on every request and never stored.

use myledger_core::{Cadence, RecurringItem, RecurringKind, now_timestamp};
use uuid::Uuid;

use crate::convert::{enum_column, flag, flag_value, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "recurring_items";

const SELECT: &str = "SELECT id, name, category_id, amount_cents, cadence, next_date, account_id, \
                      kind, active, created_at, updated_at, deleted_at FROM recurring_items";

#[derive(sqlx::FromRow)]
struct RecurringItemRow {
    id: String,
    name: String,
    category_id: Option<String>,
    amount_cents: i64,
    cadence: String,
    next_date: Option<String>,
    account_id: Option<String>,
    kind: String,
    active: i64,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

impl RecurringItemRow {
    fn into_item(self) -> Result<RecurringItem, DbError> {
        Ok(RecurringItem {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            name: self.name,
            category_id: opt_uuid_column(TABLE, &self.id, self.category_id.as_deref())?,
            amount_cents: self.amount_cents,
            cadence: enum_column::<Cadence>(TABLE, &self.id, &self.cadence)?,
            next_date: self.next_date,
            account_id: opt_uuid_column(TABLE, &self.id, self.account_id.as_deref())?,
            kind: enum_column::<RecurringKind>(TABLE, &self.id, &self.kind)?,
            active: flag(self.active),
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

/// Every live confirmed item, soonest expected charge first.
pub async fn list_recurring_items(db: &Db) -> Result<Vec<RecurringItem>, DbError> {
    let rows: Vec<RecurringItemRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE deleted_at IS NULL ORDER BY next_date ASC, name COLLATE NOCASE"
    ))
    .fetch_all(db.pool())
    .await?;
    rows.into_iter().map(RecurringItemRow::into_item).collect()
}

/// One item by ID, soft-deleted or not.
pub async fn get_recurring_item(db: &Db, id: Uuid) -> Result<Option<RecurringItem>, DbError> {
    let row: Option<RecurringItemRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(RecurringItemRow::into_item).transpose()
}

/// Finds a live item by name, ignoring case.
///
/// Accepting a suggestion checks this first, so accepting the same pattern
/// twice doesn't produce two identical bills.
pub async fn find_recurring_item_by_name(
    db: &Db,
    name: &str,
) -> Result<Option<RecurringItem>, DbError> {
    let row: Option<RecurringItemRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL"
    ))
    .bind(name.trim())
    .fetch_optional(db.pool())
    .await?;
    row.map(RecurringItemRow::into_item).transpose()
}

/// Writes a new item.
pub async fn insert_recurring_item(db: &Db, item: &RecurringItem) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO recurring_items (id, name, category_id, amount_cents, cadence, next_date, \
         account_id, kind, active, created_at, updated_at, deleted_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(item.id.to_string())
    .bind(&item.name)
    .bind(opt_uuid_text(item.category_id))
    .bind(item.amount_cents)
    .bind(item.cadence.as_str())
    .bind(&item.next_date)
    .bind(opt_uuid_text(item.account_id))
    .bind(item.kind.as_str())
    .bind(flag_value(item.active))
    .bind(&item.created_at)
    .bind(&item.updated_at)
    .bind(&item.deleted_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Overwrites an item, returning whether it existed.
pub async fn update_recurring_item(db: &Db, item: &RecurringItem) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE recurring_items SET name = ?, category_id = ?, amount_cents = ?, cadence = ?, \
         next_date = ?, account_id = ?, kind = ?, active = ?, updated_at = ?, deleted_at = ? \
         WHERE id = ?",
    )
    .bind(&item.name)
    .bind(opt_uuid_text(item.category_id))
    .bind(item.amount_cents)
    .bind(item.cadence.as_str())
    .bind(&item.next_date)
    .bind(opt_uuid_text(item.account_id))
    .bind(item.kind.as_str())
    .bind(flag_value(item.active))
    .bind(now_timestamp())
    .bind(&item.deleted_at)
    .bind(item.id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Soft-deletes an item, returning whether anything changed.
pub async fn soft_delete_recurring_item(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let now = now_timestamp();
    let result = sqlx::query(
        "UPDATE recurring_items SET deleted_at = ?, updated_at = ? \
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(&now)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every item. Used by the data wipe.
pub async fn delete_all_recurring_items(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM recurring_items").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
