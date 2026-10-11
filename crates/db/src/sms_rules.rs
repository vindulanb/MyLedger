//! Storage for SMS parsing rules.
//!
//! Rules only. The regex engine that runs them against real messages lives in
//! the mobile app; the backend's job is to keep them and hand them out. None
//! are seeded — the user writes their own to match their bank.

use myledger_core::{SmsKindHint, SmsParseRule, now_timestamp};
use uuid::Uuid;

use crate::convert::{flag, flag_value, opt_enum_column, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "sms_parse_rules";

const SELECT: &str = "SELECT id, label, sender_pattern, body_regex, amount_group, merchant_group, \
                      date_group, kind_hint, account_hint, default_category_id, enabled, \
                      created_at, updated_at FROM sms_parse_rules";

#[derive(sqlx::FromRow)]
struct SmsRuleRow {
    id: String,
    label: String,
    sender_pattern: Option<String>,
    body_regex: String,
    amount_group: Option<i64>,
    merchant_group: Option<i64>,
    date_group: Option<i64>,
    kind_hint: Option<String>,
    account_hint: Option<String>,
    default_category_id: Option<String>,
    enabled: i64,
    created_at: String,
    updated_at: String,
}

impl SmsRuleRow {
    fn into_rule(self) -> Result<SmsParseRule, DbError> {
        Ok(SmsParseRule {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            label: self.label,
            sender_pattern: self.sender_pattern,
            body_regex: self.body_regex,
            amount_group: self.amount_group,
            merchant_group: self.merchant_group,
            date_group: self.date_group,
            kind_hint: opt_enum_column::<SmsKindHint>(TABLE, &self.id, self.kind_hint.as_deref())?,
            account_hint: self.account_hint,
            default_category_id: opt_uuid_column(
                TABLE,
                &self.id,
                self.default_category_id.as_deref(),
            )?,
            enabled: flag(self.enabled),
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// Every SMS parsing rule, oldest first.
pub async fn list_sms_rules(db: &Db) -> Result<Vec<SmsParseRule>, DbError> {
    let rows: Vec<SmsRuleRow> = sqlx::query_as(&format!("{SELECT} ORDER BY created_at ASC"))
        .fetch_all(db.pool())
        .await?;
    rows.into_iter().map(SmsRuleRow::into_rule).collect()
}

/// One rule by ID.
pub async fn get_sms_rule(db: &Db, id: Uuid) -> Result<Option<SmsParseRule>, DbError> {
    let row: Option<SmsRuleRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(SmsRuleRow::into_rule).transpose()
}

/// Writes a new rule.
pub async fn insert_sms_rule(db: &Db, rule: &SmsParseRule) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sms_parse_rules (id, label, sender_pattern, body_regex, amount_group, \
         merchant_group, date_group, kind_hint, account_hint, default_category_id, enabled, \
         created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(rule.id.to_string())
    .bind(&rule.label)
    .bind(&rule.sender_pattern)
    .bind(&rule.body_regex)
    .bind(rule.amount_group)
    .bind(rule.merchant_group)
    .bind(rule.date_group)
    .bind(rule.kind_hint.map(|hint| hint.as_str()))
    .bind(&rule.account_hint)
    .bind(opt_uuid_text(rule.default_category_id))
    .bind(flag_value(rule.enabled))
    .bind(&rule.created_at)
    .bind(&rule.updated_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Overwrites a rule, returning whether it existed.
pub async fn update_sms_rule(db: &Db, rule: &SmsParseRule) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE sms_parse_rules SET label = ?, sender_pattern = ?, body_regex = ?, \
         amount_group = ?, merchant_group = ?, date_group = ?, kind_hint = ?, account_hint = ?, \
         default_category_id = ?, enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&rule.label)
    .bind(&rule.sender_pattern)
    .bind(&rule.body_regex)
    .bind(rule.amount_group)
    .bind(rule.merchant_group)
    .bind(rule.date_group)
    .bind(rule.kind_hint.map(|hint| hint.as_str()))
    .bind(&rule.account_hint)
    .bind(opt_uuid_text(rule.default_category_id))
    .bind(flag_value(rule.enabled))
    .bind(now_timestamp())
    .bind(rule.id.to_string())
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a rule, returning whether it existed.
pub async fn delete_sms_rule(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM sms_parse_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every rule. Used by the data wipe.
pub async fn delete_all_sms_rules(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM sms_parse_rules").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
