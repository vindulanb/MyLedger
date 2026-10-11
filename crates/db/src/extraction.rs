//! Storage for document extraction rules, and for the text read out of a
//! document.
//!
//! Mirrors [`crate::sms_rules`] because the two rule kinds have the same
//! shape; see the migration for why they are separate tables rather than one
//! with a flag.
//!
//! Only the extracted *text* is stored, never the fields a rule read from it.
//! Rules change, and a stored reading would go stale the moment one did.

use myledger_core::{DocumentExtractionRule, SmsKindHint, now_timestamp};
use uuid::Uuid;

use crate::convert::{flag, flag_value, opt_enum_column, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "document_extraction_rules";

const SELECT: &str = "SELECT id, label, filename_pattern, body_regex, amount_group, \
                      merchant_group, date_group, kind_hint, account_hint, default_category_id, \
                      enabled, created_at, updated_at FROM document_extraction_rules";

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: String,
    label: String,
    filename_pattern: Option<String>,
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

impl RuleRow {
    fn into_rule(self) -> Result<DocumentExtractionRule, DbError> {
        Ok(DocumentExtractionRule {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            label: self.label,
            filename_pattern: self.filename_pattern,
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

/// Every extraction rule, oldest first.
pub async fn list_rules(db: &Db) -> Result<Vec<DocumentExtractionRule>, DbError> {
    let rows: Vec<RuleRow> = sqlx::query_as(&format!("{SELECT} ORDER BY created_at ASC"))
        .fetch_all(db.pool())
        .await?;
    rows.into_iter().map(RuleRow::into_rule).collect()
}

/// One rule by ID.
pub async fn get_rule(db: &Db, id: Uuid) -> Result<Option<DocumentExtractionRule>, DbError> {
    let row: Option<RuleRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(RuleRow::into_rule).transpose()
}

/// Writes a new rule.
pub async fn insert_rule(db: &Db, rule: &DocumentExtractionRule) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_extraction_rules (id, label, filename_pattern, body_regex, \
         amount_group, merchant_group, date_group, kind_hint, account_hint, \
         default_category_id, enabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(rule.id.to_string())
    .bind(&rule.label)
    .bind(&rule.filename_pattern)
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
pub async fn update_rule(db: &Db, rule: &DocumentExtractionRule) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE document_extraction_rules SET label = ?, filename_pattern = ?, body_regex = ?, \
         amount_group = ?, merchant_group = ?, date_group = ?, kind_hint = ?, account_hint = ?, \
         default_category_id = ?, enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&rule.label)
    .bind(&rule.filename_pattern)
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
pub async fn delete_rule(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM document_extraction_rules WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every rule. Used by the data wipe.
pub async fn delete_all_rules(db: &Db) -> Result<u64, DbError> {
    let result =
        sqlx::query("DELETE FROM document_extraction_rules").execute(db.pool()).await?;
    Ok(result.rows_affected())
}

// ---- what reading a document found ----------------------------------------

/// The text pulled out of one document, and why there is none when there
/// isn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredExtraction {
    pub document_id: Uuid,
    pub extracted_text: Option<String>,
    /// Why no text could be read. Shown to the user, not just logged.
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(sqlx::FromRow)]
struct ExtractionRow {
    document_id: String,
    extracted_text: Option<String>,
    note: Option<String>,
    created_at: String,
}

/// What a previous read of this document found, if it has been read.
pub async fn get_extraction(db: &Db, document_id: Uuid) -> Result<Option<StoredExtraction>, DbError> {
    let row: Option<ExtractionRow> = sqlx::query_as(
        "SELECT document_id, extracted_text, note, created_at FROM document_extractions \
         WHERE document_id = ?",
    )
    .bind(document_id.to_string())
    .fetch_optional(db.pool())
    .await?;

    row.map(|row| {
        Ok(StoredExtraction {
            document_id: uuid_column("document_extractions", &row.document_id, &row.document_id)?,
            extracted_text: row.extracted_text,
            note: row.note,
            created_at: row.created_at,
        })
    })
    .transpose()
}

/// Records what reading a document found, replacing any earlier read.
///
/// An upsert, because re-reading a document after writing a new rule is a
/// normal thing to do and should not leave two rows disagreeing.
pub async fn save_extraction(
    db: &Db,
    document_id: Uuid,
    text: Option<&str>,
    note: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO document_extractions (document_id, extracted_text, note, created_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (document_id) DO UPDATE SET \
         extracted_text = excluded.extracted_text, note = excluded.note, \
         created_at = excluded.created_at",
    )
    .bind(document_id.to_string())
    .bind(text)
    .bind(note)
    .bind(now_timestamp())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// Removes every stored extraction. Used by the data wipe.
///
/// `documents` has `ON DELETE CASCADE` onto this table, so wiping the
/// documents would take these with it; this exists so the wipe can say how
/// many it removed rather than relying on a side effect.
pub async fn delete_all_extractions(db: &Db) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM document_extractions").execute(db.pool()).await?;
    Ok(result.rows_affected())
}
