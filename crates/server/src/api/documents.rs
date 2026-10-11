//! Uploaded receipts and statements.
//!
//! Storage and retrieval only. Nothing here reads a receipt or extracts a
//! transaction from one — that belongs to whichever client wants it, and this
//! layer's job is to hold the bytes reliably and hand them back.
//!
//! Two properties the implementation is built around:
//!
//! - **Uploads are streamed, never buffered.** A file goes to disk in chunks
//!   with a running byte count, so an oversized upload is refused after 20 MiB
//!   rather than after the server has held all of it in memory.
//! - **An upload is all or nothing.** If any file in a request is rejected,
//!   every file already written for that request is removed again. A partial
//!   upload that half-succeeded is worse than one that plainly failed.
//!
//! On-disk names are UUIDs. The client never learns them: a document is fetched
//! by ID, and the layout of the data directory is nobody else's business.

use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State, multipart::MultipartError};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use myledger_core::{Document, DocumentStatus, now_timestamp};
use myledger_db::documents as db_documents;
use myledger_db::transactions as db_transactions;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

/// Largest single file accepted, in bytes. 20 MiB.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Most files in one request.
pub const MAX_FILES: usize = 6;

/// Ceiling on the whole request body, with a little slack for the multipart
/// framing itself. Counted by the body-limit layer without buffering.
pub const MAX_REQUEST_BYTES: usize = (MAX_FILES as u64 * MAX_FILE_BYTES + 1024 * 1024) as usize;

/// A file written to disk but not yet recorded in the database.
struct Staged {
    path: PathBuf,
    document: Document,
}

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub uploaded: usize,
    /// Metadata for each stored file. Never the on-disk path.
    pub documents: Vec<Document>,
}

/// Plain text fields the upload understands, besides the files themselves.
#[derive(Debug, Default)]
struct Fields {
    /// Where the upload came from, recorded on each document.
    source: Option<String>,
    /// The transaction these files are receipts for, if any.
    linked_transaction_id: Option<String>,
}

/// Accepts one or more files.
///
/// Any multipart field carrying a filename is treated as a file. Two plain
/// text fields are understood: `source`, recording where the upload came
/// from, and `linked_transaction_id`, attaching the files to a transaction as
/// its receipts.
///
/// Linking also sets that transaction's `receipt` flag. The flag is therefore
/// never a claim the client made — it means a document really is on file,
/// which is what the receipt indicator in the UI is reporting.
pub async fn upload_documents(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let data_dir = state.config.data_dir.clone();
    tokio::fs::create_dir_all(&data_dir).await.map_err(|e| {
        ApiError::internal(format!("could not create {}: {e}", data_dir.display()))
    })?;

    let mut staged: Vec<Staged> = Vec::new();
    let mut fields = Fields::default();

    // Files are written as they arrive, so a failure part-way through leaves
    // orphans on disk unless they are explicitly cleaned up.
    if let Err(error) = collect_files(multipart, &data_dir, &mut staged, &mut fields).await {
        discard(&staged).await;
        return Err(error);
    }

    if staged.is_empty() {
        discard(&staged).await;
        return Err(ApiError::bad_request("no files in the upload"));
    }

    // Everything below is applied once the whole body has been read, so that
    // a client which sends its plain fields after its files — as curl does by
    // default — is not quietly ignored.
    if let Some(source) = fields.source {
        for entry in &mut staged {
            entry.document.source = Some(source.clone());
        }
    }

    // Resolved before anything is written to the database: a bad ID should
    // fail the upload, not leave files linked to nothing.
    let linked = match &fields.linked_transaction_id {
        None => None,
        Some(raw) => {
            let id = match Uuid::parse_str(raw.trim()) {
                Ok(id) => id,
                Err(_) => {
                    discard(&staged).await;
                    return Err(ApiError::bad_request(format!(
                        "linked_transaction_id {raw:?} is not a UUID"
                    )));
                }
            };
            match db_transactions::get_transaction(&state.db, id).await {
                Ok(Some(transaction)) if transaction.deleted_at.is_none() => Some(transaction),
                Ok(_) => {
                    discard(&staged).await;
                    return Err(ApiError::bad_request(format!(
                        "no live transaction with id {id} to attach these files to"
                    )));
                }
                Err(error) => {
                    discard(&staged).await;
                    return Err(error.into());
                }
            }
        }
    };

    if let Some(transaction) = &linked {
        for entry in &mut staged {
            entry.document.linked_transaction_id = Some(transaction.id);
            // A receipt that belongs to a known transaction is filed, not
            // waiting to be sorted.
            entry.document.status = DocumentStatus::Stored;
        }
    }

    for entry in &staged {
        if let Err(error) = db_documents::insert_document(&state.db, &entry.document).await {
            discard(&staged).await;
            return Err(error.into());
        }
    }

    // Last, because the flag should only go true once the rows it describes
    // are really there.
    if let Some(mut transaction) = linked
        && !transaction.receipt
    {
        transaction.receipt = true;
        transaction.updated_at = now_timestamp();
        if let Err(error) = db_transactions::update_transaction(&state.db, &transaction).await {
            tracing::warn!(
                transaction = %transaction.id,
                error = %error,
                "stored the receipt but could not set the transaction's receipt flag"
            );
        }
    }

    let documents: Vec<Document> = staged.into_iter().map(|entry| entry.document).collect();
    tracing::info!(count = documents.len(), "stored uploaded documents");
    Ok((StatusCode::CREATED, Json(UploadResponse { uploaded: documents.len(), documents })))
}

/// Streams every file in the request to disk.
async fn collect_files(
    mut multipart: Multipart,
    data_dir: &FsPath,
    staged: &mut Vec<Staged>,
    fields: &mut Fields,
) -> Result<(), ApiError> {
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        let Some(filename) = field.file_name().map(str::to_string) else {
            // A plain form field rather than a file.
            let name = field.name().unwrap_or_default().to_string();
            let value = field.text().await.map_err(multipart_error)?;
            let value = Some(value.trim().to_string()).filter(|s| !s.is_empty());
            match name.as_str() {
                "source" => fields.source = value,
                "linked_transaction_id" => fields.linked_transaction_id = value,
                // Anything else is ignored rather than refused, so a form
                // gaining a field doesn't break the upload.
                _ => {}
            }
            continue;
        };

        if staged.len() >= MAX_FILES {
            return Err(ApiError::bad_request(format!(
                "too many files in one upload; the limit is {MAX_FILES}"
            )));
        }

        let filename = clean_filename(&filename);
        let mime_type = field.content_type().map(str::to_string).or_else(|| guess_mime(&filename));

        let id = Uuid::new_v4();
        let stored_name = format!("{id}{}", extension_of(&filename));
        let path = data_dir.join(&stored_name);

        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| ApiError::internal(format!("could not write {}: {e}", path.display())))?;

        let mut size: u64 = 0;
        loop {
            let chunk = match field.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => {
                    // The partial file is not in `staged` yet, so remove it here.
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(multipart_error(e));
                }
            };

            size += chunk.len() as u64;
            if size > MAX_FILE_BYTES {
                // Refused mid-stream: the rest of the body is never written and
                // what has been written is thrown away.
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(ApiError::too_large(format!(
                    "{filename:?} is larger than the {} MiB limit for a single file",
                    MAX_FILE_BYTES / (1024 * 1024)
                )));
            }

            if let Err(e) = file.write_all(&chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(ApiError::internal(format!("could not write {}: {e}", path.display())));
            }
        }

        file.flush()
            .await
            .map_err(|e| ApiError::internal(format!("could not finish writing {}: {e}", path.display())))?;
        drop(file);

        staged.push(Staged {
            path,
            document: Document {
                id,
                filename,
                mime_type,
                size: Some(size as i64),
                // Relative to the data directory, so the whole store can be
                // moved without rewriting every row.
                storage_path: stored_name,
                status: DocumentStatus::Queued,
                // Filled in by the caller once the whole body has been read.
                source: None,
                linked_transaction_id: None,
                created_at: now_timestamp(),
            },
        });
    }

    Ok(())
}

/// Removes files written for a request that then failed.
async fn discard(staged: &[Staged]) {
    for entry in staged {
        if let Err(e) = tokio::fs::remove_file(&entry.path).await {
            tracing::warn!(path = %entry.path.display(), error = %e, "could not clean up an abandoned upload");
        }
    }
}

/// Turns a multipart failure into something a client can read.
///
/// The body-limit layer surfaces here as a 413, which deserves the size message
/// rather than a generic parse error.
fn multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::too_large(format!(
            "the upload is larger than the {} MiB this server accepts in one request",
            MAX_REQUEST_BYTES / (1024 * 1024)
        ));
    }
    ApiError::bad_request(format!("could not read the upload: {}", error.body_text()))
}

/// Strips anything path-like out of a client-supplied filename.
///
/// Only ever used for display and for the extension; the name on disk is a
/// UUID. Sanitising it anyway means a filename of `../../etc/passwd` cannot
/// become interesting later.
fn clean_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != ':' && *c != '\0')
        .take(255)
        .collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return "upload".to_string();
    }
    cleaned
}

/// The extension of a filename, with its dot, or empty. Restricted to short
/// alphanumeric extensions so nothing odd reaches the filesystem.
fn extension_of(filename: &str) -> String {
    match filename.rsplit_once('.') {
        Some((_, ext))
            if !ext.is_empty() && ext.len() <= 16 && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            format!(".{}", ext.to_ascii_lowercase())
        }
        _ => String::new(),
    }
}

/// A content type for the handful of things a receipt is likely to be, when the
/// client didn't say.
fn guess_mime(filename: &str) -> Option<String> {
    let ext = extension_of(filename);
    let mime = match ext.trim_start_matches('.') {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "gif" => "image/gif",
        "csv" => "text/csv",
        "txt" => "text/plain",
        _ => return None,
    };
    Some(mime.to_string())
}

/// Metadata for one document.
pub async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Document>, ApiError> {
    db_documents::get_document(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no document with id {id}")))
}

/// Streams a stored file back.
///
/// The bytes are read off disk as they are sent rather than loaded into memory,
/// so a large statement costs the same as a small one.
pub async fn download_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let Some(document) = db_documents::get_document(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no document with id {id}")));
    };

    let path = resolve(&state.config.data_dir, &document.storage_path)?;
    let file = tokio::fs::File::open(&path).await.map_err(|e| {
        ApiError::internal(format!("the file for document {id} could not be opened: {e}"))
    })?;
    let length = file
        .metadata()
        .await
        .map(|meta| meta.len())
        .unwrap_or_else(|_| document.size.unwrap_or_default().max(0) as u64);

    let body = Body::from_stream(ReaderStream::new(file));
    let content_type =
        document.mime_type.clone().unwrap_or_else(|| "application/octet-stream".to_string());

    Ok((
        [
            (CONTENT_TYPE, content_type),
            (CONTENT_LENGTH, length.to_string()),
            (CONTENT_DISPOSITION, content_disposition(&document.filename)),
        ],
        body,
    )
        .into_response())
}

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
    pub id: Uuid,
}

/// Removes a document, bytes and all.
///
/// A hard delete: `documents` has no `deleted_at`, and leaving the file behind
/// with no row pointing at it would be litter nobody could find again.
pub async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let Some(document) = db_documents::delete_document(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no document with id {id}")));
    };

    let _ = remove_file(&state.config.data_dir, &document.storage_path).await;
    Ok(Json(DeleteResponse { deleted: true, id }))
}

/// Deletes the bytes behind a document.
///
/// Reports whether the file is now gone — which includes it having been gone
/// already. Only a real failure is `false`, and that is logged rather than
/// raised: a file that resists deletion should not make the delete of the row
/// look like it failed.
pub async fn remove_file(data_dir: &FsPath, storage_path: &str) -> bool {
    let Ok(path) = resolve(data_dir, storage_path) else {
        tracing::warn!(storage_path, "refusing to delete a suspicious storage path");
        return false;
    };

    match tokio::fs::remove_file(&path).await {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not delete a document file");
            false
        }
    }
}

/// Turns a stored name into a path inside the data directory.
///
/// The stored name is one this server generated, so it is always a plain UUID
/// filename. Checking anyway costs nothing and means a corrupted or
/// hand-edited row can never point outside the store.
fn resolve(data_dir: &FsPath, storage_path: &str) -> Result<PathBuf, ApiError> {
    let looks_safe = !storage_path.is_empty()
        && !storage_path.contains("..")
        && !storage_path.contains(['/', '\\'])
        && FsPath::new(storage_path).components().count() == 1;

    if !looks_safe {
        return Err(ApiError::internal(format!(
            "document storage path {storage_path:?} is not a plain filename"
        )));
    }
    Ok(data_dir.join(storage_path))
}

/// Builds a `Content-Disposition` header that survives odd filenames.
///
/// A plain `filename=` for anything ASCII, plus an RFC 5987 `filename*` so
/// non-ASCII names still arrive intact.
fn content_disposition(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() { c } else { '_' })
        .collect();

    let encoded: String = filename
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();

    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filename_cannot_smuggle_a_path() {
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(clean_filename(r"C:\Users\me\receipt.pdf"), "receipt.pdf");
        assert_eq!(clean_filename("receipt.pdf"), "receipt.pdf");
    }

    #[test]
    fn a_useless_filename_gets_a_placeholder() {
        assert_eq!(clean_filename(""), "upload");
        assert_eq!(clean_filename("   "), "upload");
        assert_eq!(clean_filename(".."), "upload");
    }

    #[test]
    fn only_short_alphanumeric_extensions_reach_the_filesystem() {
        assert_eq!(extension_of("receipt.PDF"), ".pdf");
        assert_eq!(extension_of("scan.jpeg"), ".jpeg");
        assert_eq!(extension_of("receipt"), "");
        assert_eq!(extension_of("archive.tar.gz"), ".gz");
        assert_eq!(extension_of("odd.na me"), "", "a space is not an extension");
        assert_eq!(extension_of("odd.averyverylongextension"), "");
    }

    #[test]
    fn a_stored_path_must_be_a_plain_filename() {
        let dir = FsPath::new("/data");
        assert!(resolve(dir, "abc.pdf").is_ok());
        assert!(resolve(dir, "").is_err());
        assert!(resolve(dir, "../secrets").is_err());
        assert!(resolve(dir, "sub/abc.pdf").is_err());
        assert!(resolve(dir, r"sub\abc.pdf").is_err());
    }

    #[test]
    fn content_disposition_survives_awkward_names() {
        let header = content_disposition("bill \"march\".pdf");
        assert!(header.contains(r#"filename="bill _march_.pdf""#), "{header}");
        assert!(header.contains("filename*=UTF-8''"), "{header}");

        let unicode = content_disposition("reçu.pdf");
        assert!(unicode.contains(r#"filename="re_u.pdf""#), "{unicode}");
        assert!(unicode.contains("re%C3%A7u.pdf"), "{unicode}");
    }

    #[test]
    fn mime_is_guessed_only_for_the_obvious_cases() {
        assert_eq!(guess_mime("receipt.pdf").as_deref(), Some("application/pdf"));
        assert_eq!(guess_mime("scan.JPG").as_deref(), Some("image/jpeg"));
        assert_eq!(guess_mime("mystery.xyz"), None);
        assert_eq!(guess_mime("noextension"), None);
    }
}
