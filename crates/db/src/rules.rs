//! Categorisation rules.
//!
//! Small enough that a delete is a real delete — there is no `deleted_at` to
//! soft-delete into, and nothing else references a rule.

use myledger_core::{Rule, now_timestamp};
use uuid::Uuid;

use crate::convert::{flag, flag_value, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "rules";

const SELECT: &str = "SELECT id, when_text, then_category_id, then_tag_id, enabled, created_at, \
                      updated_at FROM rules";

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: String,
    when_text: String,
    then_category_id: Option<String>,
    then_tag_id: Option<String>,
    enabled: i64,
    created_at: String,
    updated_at: String,
}

impl RuleRow {
    fn into_rule(self) -> Result<Rule, DbError> {
        Ok(Rule {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            when_text: self.when_text,
            then_category_id: opt_uuid_column(TABLE, &self.id, self.then_category_id.as_deref())?,
            then_tag_id: opt_uuid_column(TABLE, &self.id, self.then_tag_id.as_deref())?,
            enabled: flag(self.enabled),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// Every rule, oldest first — the order they are applied in.
pub async fn list_rules(db: &Db) -> Result<Vec<Rule>, DbError> {
    let rows: Vec<RuleRow> = sqlx::query_as(&format!("{SELECT} ORDER BY created_at ASC"))
        .fetch_all(db.pool())
        .await?;
    rows.into_iter().map(RuleRow::into_rule).collect()
}

/// One rule by ID.
pub async fn get_rule(db: &Db, id: Uuid) -> Result<Option<Rule>, DbError> {
    let row: Option<RuleRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(RuleRow::into_rule).transpose()
}

/// Writes a new rule.
pub async fn insert_rule(db: &Db, rule: &Rule) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO rules (id, when_text, then_category_id, then_tag_id, enabled, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(rule.id.to_string())
    .bind(&rule.when_text)
    .bind(opt_uuid_text(rule.then_category_id))
    .bind(opt_uuid_text(rule.then_tag_id))
    .bind(flag_value(rule.enabled))
    .bind(&rule.created_at)
    .bind(&rule.updated_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Overwrites a rule, returning whether it existed.
pub async fn update_rule(db: &Db, rule: &Rule) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE rules SET when_text = ?, then_category_id = ?, then_tag_id = ?, enabled = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&rule.when_text)
    .bind(opt_uuid_text(rule.then_category_id))
    .bind(opt_uuid_text(rule.then_tag_id))
    .bind(flag_value(rule.enabled))
    .bind(now_timestamp())
    .bind(rule.id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a rule, returning whether it existed.
pub async fn delete_rule(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM rules WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every rule. Used by the data wipe.
pub async fn delete_all_rules(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM rules").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
