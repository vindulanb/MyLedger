//! The ledger itself.
//!
//! Tags live in their own join table, so reading a transaction means reading
//! two tables and writing one means writing two. Both are handled here: a
//! [`Transaction`] handed to this module goes in whole and comes back whole.

use std::collections::HashMap;

use myledger_core::{Transaction, TransactionKind, TransactionSource, now_timestamp};
use sqlx::{QueryBuilder, Sqlite};
use uuid::Uuid;

use crate::convert::{enum_column, flag, flag_value, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "transactions";

const SELECT: &str = "SELECT id, date, merchant, category_id, amount_cents, kind, account_id, \
                      receipt, source, fingerprint, sms_hash, created_at, updated_at, deleted_at \
                      FROM transactions";

#[derive(sqlx::FromRow)]
struct TransactionRow {
    id: String,
    date: String,
    merchant: String,
    category_id: Option<String>,
    amount_cents: i64,
    kind: String,
    account_id: Option<String>,
    receipt: i64,
    source: String,
    fingerprint: String,
    sms_hash: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

impl TransactionRow {
    /// Builds the domain record. Tags are attached separately, since they come
    /// from a different table.
    fn into_transaction(self) -> Result<Transaction, DbError> {
        let id = uuid_column(TABLE, &self.id, &self.id)?;
        Ok(Transaction {
            id,
            date: self.date,
            merchant: self.merchant,
            category_id: opt_uuid_column(TABLE, &self.id, self.category_id.as_deref())?,
            amount_cents: self.amount_cents,
            kind: enum_column::<TransactionKind>(TABLE, &self.id, &self.kind)?,
            account_id: opt_uuid_column(TABLE, &self.id, self.account_id.as_deref())?,
            tags: Vec::new(),
            receipt: flag(self.receipt),
            source: enum_column::<TransactionSource>(TABLE, &self.id, &self.source)?,
            fingerprint: self.fingerprint,
            sms_hash: self.sms_hash,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

/// Writes a transaction and its tags.
///
/// Returns [`DbError::DuplicateFingerprint`] if the ledger already holds this
/// purchase. That check is the database's `UNIQUE` constraint rather than a
/// prior `SELECT`, so two imports racing each other still can't both win.
pub async fn insert_transaction(db: &Db, transaction: &Transaction) -> Result<(), DbError> {
    let mut conn = db.pool().begin().await?;

    sqlx::query(
        "INSERT INTO transactions (id, date, merchant, category_id, amount_cents, kind, \
         account_id, receipt, source, fingerprint, sms_hash, created_at, updated_at, deleted_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(transaction.id.to_string())
    .bind(&transaction.date)
    .bind(&transaction.merchant)
    .bind(opt_uuid_text(transaction.category_id))
    .bind(transaction.amount_cents)
    .bind(transaction.kind.as_str())
    .bind(opt_uuid_text(transaction.account_id))
    .bind(flag_value(transaction.receipt))
    .bind(transaction.source.as_str())
    .bind(&transaction.fingerprint)
    .bind(&transaction.sms_hash)
    .bind(&transaction.created_at)
    .bind(&transaction.updated_at)
    .bind(&transaction.deleted_at)
    .execute(&mut *conn)
    .await
    .map_err(error::classify_insert_error)?;

    write_tags(&mut conn, transaction.id, &transaction.tags).await?;

    conn.commit().await?;
    Ok(())
}

/// One transaction by ID, soft-deleted or not.
pub async fn get_transaction(db: &Db, id: Uuid) -> Result<Option<Transaction>, DbError> {
    let row: Option<TransactionRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;

    let Some(row) = row else { return Ok(None) };
    let mut transaction = row.into_transaction()?;
    transaction.tags = read_tags(db, &[transaction.id]).await?.remove(&id).unwrap_or_default();
    Ok(Some(transaction))
}

/// Looks a transaction up by its duplicate-detection fingerprint.
///
/// Soft-deleted rows count: the fingerprint stays unique across the whole
/// table, so a deleted transaction still blocks a re-import of the same
/// purchase. Callers that want to offer an undelete need to see it.
pub async fn find_transaction_by_fingerprint(
    db: &Db,
    fingerprint: &str,
) -> Result<Option<Transaction>, DbError> {
    let row: Option<TransactionRow> = sqlx::query_as(&format!("{SELECT} WHERE fingerprint = ?"))
        .bind(fingerprint)
        .fetch_optional(db.pool())
        .await?;

    let Some(row) = row else { return Ok(None) };
    let mut transaction = row.into_transaction()?;
    transaction.tags = read_tags(db, &[transaction.id])
        .await?
        .remove(&transaction.id)
        .unwrap_or_default();
    Ok(Some(transaction))
}

/// The most recent live transactions, newest first, capped at `limit`.
pub async fn list_transactions(db: &Db, limit: i64) -> Result<Vec<Transaction>, DbError> {
    let rows: Vec<TransactionRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE deleted_at IS NULL ORDER BY date DESC, created_at DESC LIMIT ?"
    ))
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    attach_tags(db, rows).await
}

/// Live expenses dated on or after `since`, oldest first.
///
/// This is what recurring detection reads: it only cares about money going
/// out, and it wants the dates in order.
pub async fn list_expenses_since(db: &Db, since: &str) -> Result<Vec<Transaction>, DbError> {
    let rows: Vec<TransactionRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE deleted_at IS NULL AND kind = 'expense' AND date >= ? ORDER BY date ASC"
    ))
    .bind(since)
    .fetch_all(db.pool())
    .await?;

    attach_tags(db, rows).await
}

/// Overwrites a transaction, tags included, and stamps `updated_at`.
///
/// A full-row write rather than a set of partial updaters: callers read the
/// transaction, change what they mean to change, and write it back, which
/// keeps "update only the category" honest without a tower of `Option`s.
/// Returns whether a row was found.
pub async fn update_transaction(db: &Db, transaction: &Transaction) -> Result<bool, DbError> {
    let mut conn = db.pool().begin().await?;

    let result = sqlx::query(
        "UPDATE transactions SET date = ?, merchant = ?, category_id = ?, amount_cents = ?, \
         kind = ?, account_id = ?, receipt = ?, source = ?, fingerprint = ?, sms_hash = ?, \
         updated_at = ?, deleted_at = ? WHERE id = ?",
    )
    .bind(&transaction.date)
    .bind(&transaction.merchant)
    .bind(opt_uuid_text(transaction.category_id))
    .bind(transaction.amount_cents)
    .bind(transaction.kind.as_str())
    .bind(opt_uuid_text(transaction.account_id))
    .bind(flag_value(transaction.receipt))
    .bind(transaction.source.as_str())
    .bind(&transaction.fingerprint)
    .bind(&transaction.sms_hash)
    .bind(now_timestamp())
    .bind(&transaction.deleted_at)
    .bind(transaction.id.to_string())
    .execute(&mut *conn)
    .await
    .map_err(error::classify_insert_error)?;

    if result.rows_affected() == 0 {
        conn.rollback().await?;
        return Ok(false);
    }

    sqlx::query("DELETE FROM transaction_tags WHERE transaction_id = ?")
        .bind(transaction.id.to_string())
        .execute(&mut *conn)
        .await?;
    write_tags(&mut conn, transaction.id, &transaction.tags).await?;

    conn.commit().await?;
    Ok(true)
}

/// Soft-deletes a transaction, returning whether anything changed.
pub async fn soft_delete_transaction(db: &Db, id: Uuid) -> Result<bool, DbError> {
    let now = now_timestamp();
    let result = sqlx::query(
        "UPDATE transactions SET deleted_at = ?, updated_at = ? \
         WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(&now)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// How many live transactions there are.
pub async fn count_transactions(db: &Db) -> Result<i64, DbError> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM transactions WHERE deleted_at IS NULL")
            .fetch_one(db.pool())
            .await?;
    Ok(count)
}

/// Removes every transaction and its tag links, for real. Used by the data wipe.
pub async fn delete_all_transactions(db: &Db) -> Result<u64, DbError> {
    let mut conn = db.pool().begin().await?;
    sqlx::query("DELETE FROM transaction_tags").execute(&mut *conn).await?;
    let result = sqlx::query("DELETE FROM transactions").execute(&mut *conn).await?;
    conn.commit().await?;
    Ok(result.rows_affected())
}

/// Fills in the tags for a batch of rows in one extra query.
async fn attach_tags(db: &Db, rows: Vec<TransactionRow>) -> Result<Vec<Transaction>, DbError> {
    let mut transactions: Vec<Transaction> = rows
        .into_iter()
        .map(TransactionRow::into_transaction)
        .collect::<Result<_, _>>()?;

    let ids: Vec<Uuid> = transactions.iter().map(|t| t.id).collect();
    let mut tags = read_tags(db, &ids).await?;
    for transaction in &mut transactions {
        transaction.tags = tags.remove(&transaction.id).unwrap_or_default();
    }
    Ok(transactions)
}

/// The tag IDs attached to each of `ids`.
async fn read_tags(db: &Db, ids: &[Uuid]) -> Result<HashMap<Uuid, Vec<Uuid>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder: QueryBuilder<Sqlite> =
        QueryBuilder::new("SELECT transaction_id, tag_id FROM transaction_tags WHERE transaction_id IN (");
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(id.to_string());
    }
    builder.push(")");

    let pairs: Vec<(String, String)> = builder.build_query_as().fetch_all(db.pool()).await?;

    let mut grouped: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (transaction_id, tag_id) in pairs {
        let transaction_id = uuid_column("transaction_tags", &transaction_id, &transaction_id)?;
        let tag = uuid_column("transaction_tags", &tag_id, &tag_id)?;
        grouped.entry(transaction_id).or_default().push(tag);
    }
    Ok(grouped)
}

/// Links a transaction to its tags. Duplicates in `tags` are ignored rather
/// than being an error — the join table's primary key settles it.
async fn write_tags(
    conn: &mut sqlx::SqliteConnection,
    transaction_id: Uuid,
    tags: &[Uuid],
) -> Result<(), DbError> {
    for tag_id in tags {
        sqlx::query(
            "INSERT OR IGNORE INTO transaction_tags (transaction_id, tag_id) VALUES (?, ?)",
        )
        .bind(transaction_id.to_string())
        .bind(tag_id.to_string())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
