//! Erasing the financial data.
//!
//! Every delete runs inside one SQL transaction. A wipe that stopped half way —
//! transactions gone but budgets still referencing them, documents recorded but
//! their files removed — would be worse than one that plainly failed, and the
//! user has no way to undo either.
//!
//! Three things deliberately survive:
//!
//! - **`users` and `device_tokens`**, so clearing your data doesn't sign you out
//!   or unpair your phone.
//! - **`sessions`**, for the same reason: the browser's credential is no more
//!   financial data than the phone's is.
//! - **`categories` and `accounts`**, which are a list the user has curated
//!   rather than a record of their money. The seed refills them only if they
//!   were empty to begin with.

use myledger_core::Document;

use crate::Db;
use crate::error::DbError;

/// How many rows each table lost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WipeCounts {
    pub transactions: u64,
    pub transaction_tags: u64,
    pub tags: u64,
    pub documents: u64,
    pub rules: u64,
    pub sms_parse_rules: u64,
    pub budgets: u64,
    pub goals: u64,
    pub recurring_items: u64,
    pub dismissed_patterns: u64,
}

impl WipeCounts {
    /// Total rows removed.
    pub fn total(&self) -> u64 {
        self.transactions
            + self.transaction_tags
            + self.tags
            + self.documents
            + self.rules
            + self.sms_parse_rules
            + self.budgets
            + self.goals
            + self.recurring_items
            + self.dismissed_patterns
    }
}

/// The result of a wipe.
#[derive(Debug, Clone)]
pub struct Wiped {
    pub counts: WipeCounts,
    /// The documents whose rows were removed, so the caller can delete the
    /// files behind them. Returned rather than deleted here because this crate
    /// does not touch the filesystem.
    pub documents: Vec<Document>,
}

/// Removes every financial record, in one transaction.
///
/// Deletes are hard, not soft: a wipe that left rows behind with a `deleted_at`
/// would still be holding the data it promised to destroy — and would keep
/// every fingerprint taken, so re-importing a statement afterwards would report
/// duplicates against transactions the user believes are gone.
pub async fn wipe_financial_data(db: &Db) -> Result<Wiped, DbError> {
    // Read the document rows before anything is deleted; their paths are the
    // only record of which files to remove.
    let documents = crate::documents::list_all_documents(db).await?;

    let mut conn = db.pool().begin().await?;

    // The order is load-bearing, so each step is its own statement rather than
    // a field in a struct literal. Children go before parents: documents and
    // transaction_tags both reference transactions, and clearing them first
    // saves SQLite from rewriting every link on the way past.
    let documents_deleted = delete_all(&mut conn, "DELETE FROM documents").await?;
    let transaction_tags = delete_all(&mut conn, "DELETE FROM transaction_tags").await?;
    let transactions = delete_all(&mut conn, "DELETE FROM transactions").await?;
    let tags = delete_all(&mut conn, "DELETE FROM tags").await?;
    let rules = delete_all(&mut conn, "DELETE FROM rules").await?;
    let sms_parse_rules = delete_all(&mut conn, "DELETE FROM sms_parse_rules").await?;
    let budgets = delete_all(&mut conn, "DELETE FROM budgets").await?;
    let goals = delete_all(&mut conn, "DELETE FROM goals").await?;
    let recurring_items = delete_all(&mut conn, "DELETE FROM recurring_items").await?;
    let dismissed_patterns = delete_all(&mut conn, "DELETE FROM dismissed_patterns").await?;

    conn.commit().await?;

    Ok(Wiped {
        counts: WipeCounts {
            transactions,
            transaction_tags,
            tags,
            documents: documents_deleted,
            rules,
            sms_parse_rules,
            budgets,
            goals,
            recurring_items,
            dismissed_patterns,
        },
        documents,
    })
}

async fn delete_all(conn: &mut sqlx::SqliteConnection, sql: &str) -> Result<u64, DbError> {
    Ok(sqlx::query(sql).execute(&mut *conn).await?.rows_affected())
}
