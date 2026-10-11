//! Metadata for uploaded receipts and statements.
//!
//! The bytes live on disk under the data directory; these rows say where. The
//! delete functions hand back the rows they removed so the caller can clean up
//! the files — this module never touches the filesystem itself.

use myledger_core::{Document, DocumentStatus};
use uuid::Uuid;

use crate::convert::{enum_column, opt_uuid_column, opt_uuid_text, uuid_column};
use crate::error::DbError;
use crate::{Db, error};

const TABLE: &str = "documents";

const SELECT: &str = "SELECT id, filename, mime_type, size, storage_path, status, source, \
                      linked_transaction_id, created_at FROM documents";

#[derive(sqlx::FromRow)]
struct DocumentRow {
    id: String,
    filename: String,
    mime_type: Option<String>,
    size: Option<i64>,
    storage_path: String,
    status: String,
    source: Option<String>,
    linked_transaction_id: Option<String>,
    created_at: String,
}

impl DocumentRow {
    fn into_document(self) -> Result<Document, DbError> {
        Ok(Document {
            id: uuid_column(TABLE, &self.id, &self.id)?,
            filename: self.filename,
            mime_type: self.mime_type,
            size: self.size,
            storage_path: self.storage_path,
            status: enum_column::<DocumentStatus>(TABLE, &self.id, &self.status)?,
            source: self.source,
            linked_transaction_id: opt_uuid_column(
                TABLE,
                &self.id,
                self.linked_transaction_id.as_deref(),
            )?,
            created_at: self.created_at,
        })
    }
}

/// The most recently uploaded documents, newest first.
pub async fn list_recent_documents(db: &Db, limit: i64) -> Result<Vec<Document>, DbError> {
    let rows: Vec<DocumentRow> =
        sqlx::query_as(&format!("{SELECT} ORDER BY created_at DESC LIMIT ?"))
            .bind(limit)
            .fetch_all(db.pool())
            .await?;
    rows.into_iter().map(DocumentRow::into_document).collect()
}

/// Every document. Used by the data wipe, which needs all the paths.
pub async fn list_all_documents(db: &Db) -> Result<Vec<Document>, DbError> {
    let rows: Vec<DocumentRow> = sqlx::query_as(&format!("{SELECT} ORDER BY created_at DESC"))
        .fetch_all(db.pool())
        .await?;
    rows.into_iter().map(DocumentRow::into_document).collect()
}

/// One document by ID.
pub async fn get_document(db: &Db, id: Uuid) -> Result<Option<Document>, DbError> {
    let row: Option<DocumentRow> = sqlx::query_as(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    row.map(DocumentRow::into_document).transpose()
}

/// Writes a new document row. The bytes are the caller's responsibility.
pub async fn insert_document(db: &Db, document: &Document) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO documents (id, filename, mime_type, size, storage_path, status, source, \
         linked_transaction_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(document.id.to_string())
    .bind(&document.filename)
    .bind(&document.mime_type)
    .bind(document.size)
    .bind(&document.storage_path)
    .bind(document.status.as_str())
    .bind(&document.source)
    .bind(opt_uuid_text(document.linked_transaction_id))
    .bind(&document.created_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;
    Ok(())
}

/// Moves a document to a new status, returning whether it existed.
pub async fn set_document_status(
    db: &Db,
    id: Uuid,
    status: DocumentStatus,
) -> Result<bool, DbError> {
    let result = sqlx::query("UPDATE documents SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Points a document at the transaction it backs up, or clears the link.
pub async fn link_document(
    db: &Db,
    id: Uuid,
    transaction_id: Option<Uuid>,
) -> Result<bool, DbError> {
    let result = sqlx::query("UPDATE documents SET linked_transaction_id = ? WHERE id = ?")
        .bind(opt_uuid_text(transaction_id))
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Writes a document's status and link together, returning whether it existed.
///
/// One statement rather than two calls, so a document cannot end up linked to
/// a transaction while still sitting at `review` because the second write
/// failed. Only these two fields are writable: the filename, type and size
/// are what the file *is*, and the bytes on disk are the record of that.
pub async fn update_document(db: &Db, document: &Document) -> Result<bool, DbError> {
    let result =
        sqlx::query("UPDATE documents SET status = ?, linked_transaction_id = ? WHERE id = ?")
            .bind(document.status.as_str())
            .bind(opt_uuid_text(document.linked_transaction_id))
            .bind(document.id.to_string())
            .execute(db.pool())
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes a document row, returning it so the caller can delete the file.
pub async fn delete_document(db: &Db, id: Uuid) -> Result<Option<Document>, DbError> {
    let Some(document) = get_document(db, id).await? else {
        return Ok(None);
    };
    sqlx::query("DELETE FROM documents WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(Some(document))
}

/// Deletes every document row, returning them all so their files can be
/// removed. Used by the data wipe.
pub async fn delete_all_documents(db: &Db) -> Result<Vec<Document>, DbError> {
    let documents = list_all_documents(db).await?;
    sqlx::query("DELETE FROM documents").execute(db.pool()).await?;
    Ok(documents)
}
