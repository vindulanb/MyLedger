//! Monthly spending ceilings, one per category.

use myledger_core::{Budget, now_timestamp};
use uuid::Uuid;

use crate::convert::{flag, flag_value, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "budgets";

const SELECT: &str = "SELECT id, category_id, monthly_limit_cents, active, created_at, \
                      updated_at, deleted_at FROM budgets";

#[derive(sqlx::FromRow)]
struct BudgetRow {
    id: String,
    category_id: Option<String>,
    monthly_limit_cents: i64,
    active: i64,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

impl BudgetRow {
    fn into_budget(self) -> Result<Budget, DbError> {
        Ok(Budget {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            category_id: opt_uuid_column(TABLE, &self.id, self.category_id.as_deref())?,
            monthly_limit_cents: self.monthly_limit_cents,
            active: flag(self.active),
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

/// Every live budget, oldest first.
pub async fn list_budgets(db: &Db) -> Result<Vec<Budget>, DbError> {
    let rows: Vec<BudgetRow> =
        sqlx::query_as(&format!("{SELECT} WHERE deleted_at IS NULL ORDER BY created_at ASC"))
            .fetch_all(db.pool())
            .await?;
    rows.into_iter().map(BudgetRow::into_budget).collect()
}

/// One budget by ID, soft-deleted or not.
pub async fn get_budget(db: &Db, id: Uuid) -> Result<Option<Budget>, DbError> {
    let row: Option<BudgetRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(BudgetRow::into_budget).transpose()
}

/// Writes a new budget.
pub async fn insert_budget(db: &Db, budget: &Budget) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO budgets (id, category_id, monthly_limit_cents, active, created_at, \
         updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(budget.id.to_string())
    .bind(opt_uuid_text(budget.category_id))
    .bind(budget.monthly_limit_cents)
    .bind(flag_value(budget.active))
    .bind(&budget.created_at)
    .bind(&budget.updated_at)
    .bind(&budget.deleted_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Overwrites a budget, returning whether it existed.
pub async fn update_budget(db: &Db, budget: &Budget) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE budgets SET category_id = ?, monthly_limit_cents = ?, active = ?, updated_at = ?, \
         deleted_at = ? WHERE id = ?",
    )
    .bind(opt_uuid_text(budget.category_id))
    .bind(budget.monthly_limit_cents)
    .bind(flag_value(budget.active))
    .bind(now_timestamp())
    .bind(&budget.deleted_at)
    .bind(budget.id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Soft-deletes a budget, returning whether anything changed.
pub async fn soft_delete_budget(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let now = now_timestamp();
    let result = sqlx::query(
        "UPDATE budgets SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(&now)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every budget. Used by the data wipe.
pub async fn delete_all_budgets(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM budgets").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
