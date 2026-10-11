//! Savings goals.

use myledger_core::{Goal, now_timestamp};
use uuid::Uuid;

use crate::convert::uuid_column;
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "goals";

const SELECT: &str = "SELECT id, name, target_cents, current_cents, due_date, note, created_at, \
                      updated_at, deleted_at FROM goals";

#[derive(sqlx::FromRow)]
struct GoalRow {
    id: String,
    name: String,
    target_cents: i64,
    current_cents: i64,
    due_date: Option<String>,
    note: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

impl GoalRow {
    fn into_goal(self) -> Result<Goal, DbError> {
        Ok(Goal {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            name: self.name,
            target_cents: self.target_cents,
            current_cents: self.current_cents,
            due_date: self.due_date,
            note: self.note,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

/// Every live goal, oldest first.
pub async fn list_goals(db: &Db) -> Result<Vec<Goal>, DbError> {
    let rows: Vec<GoalRow> =
        sqlx::query_as(&format!("{SELECT} WHERE deleted_at IS NULL ORDER BY created_at ASC"))
            .fetch_all(db.pool())
            .await?;
    rows.into_iter().map(GoalRow::into_goal).collect()
}

/// One goal by ID, soft-deleted or not.
pub async fn get_goal(db: &Db, id: Uuid) -> Result<Option<Goal>, DbError> {
    let row: Option<GoalRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(GoalRow::into_goal).transpose()
}

/// Writes a new goal.
pub async fn insert_goal(db: &Db, goal: &Goal) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO goals (id, name, target_cents, current_cents, due_date, note, created_at, \
         updated_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(goal.id.to_string())
    .bind(&goal.name)
    .bind(goal.target_cents)
    .bind(goal.current_cents)
    .bind(&goal.due_date)
    .bind(&goal.note)
    .bind(&goal.created_at)
    .bind(&goal.updated_at)
    .bind(&goal.deleted_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Overwrites a goal, returning whether it existed.
pub async fn update_goal(db: &Db, goal: &Goal) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE goals SET name = ?, target_cents = ?, current_cents = ?, due_date = ?, note = ?, \
         updated_at = ?, deleted_at = ? WHERE id = ?",
    )
    .bind(&goal.name)
    .bind(goal.target_cents)
    .bind(goal.current_cents)
    .bind(&goal.due_date)
    .bind(&goal.note)
    .bind(now_timestamp())
    .bind(&goal.deleted_at)
    .bind(goal.id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Soft-deletes a goal, returning whether anything changed.
pub async fn soft_delete_goal(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let now = now_timestamp();
    let result = sqlx::query(
        "UPDATE goals SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(&now)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every goal. Used by the data wipe.
pub async fn delete_all_goals(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM goals").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
