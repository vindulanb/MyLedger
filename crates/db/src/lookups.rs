//! Accounts, categories and tags.
//!
//! Three tables with identical shape — an ID, a name, the usual bookkeeping —
//! so one macro writes the same six operations for each. The generated function
//! names are spelled out at the call site rather than derived, so that
//! `insert_category` is still something you can grep for.

use myledger_core::{Account, Category, Tag, now_timestamp};
use uuid::Uuid;

use crate::convert::uuid_column;
use crate::error::DbError;
use crate::{Db, error};

/// One row of any of the three lookup tables.
#[derive(sqlx::FromRow)]
struct LookupRow {
    id: String,
    name: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

macro_rules! lookup_table {
    (
        table = $table:literal,
        // An `ident`, not a `path`: only a bare identifier can open a struct
        // literal inside a macro.
        record = $record:ident,
        list = $list:ident,
        list_all = $list_all:ident,
        get = $get:ident,
        find_by_name = $find:ident,
        insert = $insert:ident,
        rename = $rename:ident,
        soft_delete = $soft_delete:ident,
        count = $count:ident $(,)?
    ) => {
        /// Every live row, ordered by name.
        pub async fn $list(db: &Db) -> Result<Vec<$record>, DbError> {
            let rows: Vec<LookupRow> = sqlx::query_as(concat!(
                "SELECT id, name, created_at, updated_at, deleted_at FROM ",
                $table,
                " WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE"
            ))
            .fetch_all(db.pool())
            .await?;
            rows.into_iter().map(convert).collect()
        }

        /// Every row, soft-deleted ones included.
        ///
        /// Needed when resolving an ID that an old transaction still points at.
        pub async fn $list_all(db: &Db) -> Result<Vec<$record>, DbError> {
            let rows: Vec<LookupRow> = sqlx::query_as(concat!(
                "SELECT id, name, created_at, updated_at, deleted_at FROM ",
                $table,
                " ORDER BY name COLLATE NOCASE"
            ))
            .fetch_all(db.pool())
            .await?;
            rows.into_iter().map(convert).collect()
        }

        /// One row by ID, whether or not it is soft-deleted.
        pub async fn $get(db: &Db, id: Uuid) -> Result<Option<$record>, DbError> {
            let row: Option<LookupRow> = sqlx::query_as(concat!(
                "SELECT id, name, created_at, updated_at, deleted_at FROM ",
                $table,
                " WHERE id = ?"
            ))
            .bind(id.to_string())
            .fetch_optional(db.pool())
            .await?;
            row.map(convert).transpose()
        }

        /// One live row by name, ignoring case.
        ///
        /// Imports know an account by what it is called, not by its ID.
        pub async fn $find(db: &Db, name: &str) -> Result<Option<$record>, DbError> {
            let row: Option<LookupRow> = sqlx::query_as(concat!(
                "SELECT id, name, created_at, updated_at, deleted_at FROM ",
                $table,
                " WHERE name = ? COLLATE NOCASE AND deleted_at IS NULL"
            ))
            .bind(name.trim())
            .fetch_optional(db.pool())
            .await?;
            row.map(convert).transpose()
        }

        /// Creates a row, returning it.
        ///
        /// A name is the whole record here, so the caller supplies only that
        /// and the ID and timestamps are filled in.
        pub async fn $insert(db: &Db, name: &str) -> Result<$record, DbError> {
            let now = now_timestamp();
            let record = $record {
                id: Uuid::new_v4(),
                name: name.trim().to_string(),
                created_at: now.clone(),
                updated_at: now,
                deleted_at: None,
            };

            sqlx::query(concat!(
                "INSERT INTO ",
                $table,
                " (id, name, created_at, updated_at, deleted_at) VALUES (?, ?, ?, ?, NULL)"
            ))
            .bind(record.id.to_string())
            .bind(&record.name)
            .bind(&record.created_at)
            .bind(&record.updated_at)
            .execute(db.pool())
            .await
            .map_err(error::classify_insert_error)?;

            Ok(record)
        }

        /// Renames a row. `Ok(None)` means there was no such row.
        pub async fn $rename(db: &Db, id: Uuid, name: &str) -> Result<Option<$record>, DbError> {
            sqlx::query(concat!(
                "UPDATE ",
                $table,
                " SET name = ?, updated_at = ? WHERE id = ?"
            ))
            .bind(name.trim())
            .bind(now_timestamp())
            .bind(id.to_string())
            .execute(db.pool())
            .await?;

            $get(db, id).await
        }

        /// Soft-deletes a row, returning whether anything changed.
        ///
        /// The row stays, so transactions already filed against it keep their
        /// meaning; it simply stops being offered.
        pub async fn $soft_delete(db: &Db, id: Uuid) -> Result<bool, DbError> {
            let now = now_timestamp();
            let result = sqlx::query(concat!(
                "UPDATE ",
                $table,
                " SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL"
            ))
            .bind(&now)
            .bind(&now)
            .bind(id.to_string())
            .execute(db.pool())
            .await?;

            Ok(result.rows_affected() > 0)
        }

        /// How many rows exist at all, soft-deleted included.
        ///
        /// The seed routine uses this to decide whether it has already run.
        pub async fn $count(db: &Db) -> Result<i64, DbError> {
            let (count,): (i64,) = sqlx::query_as(concat!("SELECT COUNT(*) FROM ", $table))
                .fetch_one(db.pool())
                .await?;
            Ok(count)
        }

        /// Builds the domain record from a row of this table.
        fn convert(row: LookupRow) -> Result<$record, DbError> {
            Ok($record {
                id: uuid_column($table, &row.id, &row.id)?,
                name: row.name,
                created_at: row.created_at,
                updated_at: row.updated_at,
                deleted_at: row.deleted_at,
            })
        }
    };
}

/// Accounts: where money sits.
pub mod accounts {
    use super::*;

    lookup_table! {
        table = "accounts",
        record = Account,
        list = list_accounts,
        list_all = list_all_accounts,
        get = get_account,
        find_by_name = find_account_by_name,
        insert = insert_account,
        rename = rename_account,
        soft_delete = soft_delete_account,
        count = count_accounts,
    }
}

/// Categories: what money was spent on.
pub mod categories {
    use super::*;

    lookup_table! {
        table = "categories",
        record = Category,
        list = list_categories,
        list_all = list_all_categories,
        get = get_category,
        find_by_name = find_category_by_name,
        insert = insert_category,
        rename = rename_category,
        soft_delete = soft_delete_category,
        count = count_categories,
    }
}

/// Tags: free-form labels, any number per transaction.
pub mod tags {
    use super::*;

    lookup_table! {
        table = "tags",
        record = Tag,
        list = list_tags,
        list_all = list_all_tags,
        get = get_tag,
        find_by_name = find_tag_by_name,
        insert = insert_tag,
        rename = rename_tag,
        soft_delete = soft_delete_tag,
        count = count_tags,
    }
}

/// Removes every tag, along with the transaction links that pointed at them.
///
/// Used by the data wipe. Categories and accounts are re-seeded instead of
/// being emptied, because the app needs some to function.
pub async fn delete_all_tags(db: &Db) -> Result<u64, DbError> {
    let mut conn = db.pool().begin().await?;
    sqlx::query("DELETE FROM transaction_tags").execute(&mut *conn).await?;
    let result = sqlx::query("DELETE FROM tags").execute(&mut *conn).await?;
    conn.commit().await?;
    Ok(result.rows_affected())
}
