//! The HTTP layer.
//!
//! Every call is same-origin, because the app is served by the very backend it
//! talks to. That matters for authentication: the session lives in a
//! `HttpOnly` cookie, which the browser attaches on its own and this code can
//! neither read nor write. Credentials are declared `SameOrigin` explicitly
//! rather than relying on the fetch default, so the intent is visible.
//!
//! No token is ever kept in `localStorage`, `sessionStorage` or IndexedDB —
//! not the session, and not financial data. The database is canonical.

use gloo_net::http::Request;
use myledger_core::{
    DocumentExtractionRule, ErrorResponse, ExtractionResponse, ImportCsvReport, ImportOptions,
    LoginRequest, LoginResponse, MeResponse, PreferencesRequest, PreferencesResponse,
    StateResponse, StoredExtractionResponse, Transaction,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use web_sys::RequestCredentials;

const API: &str = "/api/v1";

/// A failed call, reduced to what the UI needs to decide what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiFailure {
    /// HTTP status, or 0 when the request never reached the server.
    pub status: u16,
    /// Something worth showing a person.
    pub message: String,
}

impl ApiFailure {
    /// Whether the session has gone. The caller's job is then to show the
    /// login screen, not to retry.
    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }

    fn offline(detail: impl std::fmt::Display) -> Self {
        Self { status: 0, message: format!("Could not reach the server: {detail}") }
    }
}

pub type ApiResult<T> = Result<T, ApiFailure>;

/// `GET /api/v1/me` — who we are, if anyone. The session check on startup.
pub async fn me() -> ApiResult<MeResponse> {
    get("/me").await
}

/// `GET /api/v1/state` — the whole ledger.
pub async fn state() -> ApiResult<StateResponse> {
    get("/state").await
}

/// `POST /api/v1/auth/login`. On success the browser stores the session cookie.
pub async fn login(username: &str, password: &str) -> ApiResult<LoginResponse> {
    let body =
        LoginRequest { username: username.to_string(), password: password.to_string() };
    post("/auth/login", &body).await
}

/// `PUT /api/v1/preferences` — saves a subset of the user's preferences.
///
/// Only the groups present in `request` are touched. The reply carries the
/// preferences as they now stand, but callers still refetch the whole ledger
/// afterwards: a mutation can change transactions too (deleting a category
/// re-points the rows that used it), and the database is canonical.
pub async fn put_preferences(request: &PreferencesRequest) -> ApiResult<PreferencesResponse> {
    put("/preferences", request).await
}

/// One transaction as the Add-entry form sends it.
///
/// Mirrors the server's `NewTransactionRequest`. It is not in `core::wire`
/// because the CSV import and the mobile app build their rows differently and
/// this is the shape of *this* form; the fields it does send are named to
/// match, so a rename on the server fails here loudly.
#[derive(Debug, Clone, Serialize)]
pub struct NewTransaction {
    pub date: String,
    pub merchant: String,
    pub amount_cents: i64,
    /// `expense` or `income`.
    pub kind: String,
    pub category_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub tags: Vec<Uuid>,
    pub source: String,
}

/// What the server made of a batch of transactions.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ImportReport {
    pub inserted: usize,
    pub duplicates: usize,
    pub rejected: usize,
    /// The rows that were written, as stored.
    pub transactions: Vec<Transaction>,
}

/// The fields a `PATCH /transactions/{id}` may carry.
///
/// Absent means "leave alone" and is the reason both fields are `Option`:
/// re-filing a category must not silently clear the tags.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TransactionPatch {
    /// `Some(None)` clears the category; `None` leaves it as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Option<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Uuid>>,
}

/// `POST /api/v1/transactions` — records one transaction.
pub async fn create_transaction(entry: &NewTransaction) -> ApiResult<ImportReport> {
    // The endpoint takes a single object or an array; one object is what this
    // form has.
    post("/transactions", entry).await
}

/// `PATCH /api/v1/transactions/{id}` — re-files a transaction.
pub async fn patch_transaction(id: Uuid, patch: &TransactionPatch) -> ApiResult<Transaction> {
    let request = Request::patch(&url(&format!("/transactions/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .json(patch)
        .map_err(ApiFailure::offline)?;
    send(request).await
}

/// `DELETE /api/v1/transactions/{id}`.
pub async fn delete_transaction(id: Uuid) -> ApiResult<()> {
    let request = Request::delete(&url(&format!("/transactions/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

/// A file picked in the browser, already read into memory.
///
/// `Clone` because it lives in a signal, and reading a signal by calling it
/// clones. A receipt is a few hundred kilobytes at most, so the copy is not
/// worth the ceremony of avoiding.
#[derive(Clone)]
pub struct Upload {
    pub filename: String,
    pub mime_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// `POST /api/v1/documents` — stores files, optionally as a transaction's
/// receipts.
pub async fn upload_documents(
    files: &[Upload],
    source: &str,
    linked_transaction_id: Option<Uuid>,
) -> ApiResult<()> {
    if files.is_empty() {
        return Err(ApiFailure { status: 0, message: "No files to upload.".to_string() });
    }

    let id = linked_transaction_id.map(|id| id.to_string());
    let mut fields: Vec<(&str, &str)> = Vec::new();
    if !source.is_empty() {
        fields.push(("source", source));
    }
    if let Some(id) = &id {
        fields.push(("linked_transaction_id", id));
    }

    let body = multipart(&fields, files);
    let request = Request::post(&url("/documents"))
        .credentials(RequestCredentials::SameOrigin)
        .header("Content-Type", &format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(body)
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

const BOUNDARY: &str = "myledger-boundary-7f3c1a9e2b";

/// Builds a `multipart/form-data` body.
///
/// Assembled here rather than handed to the browser as a `FormData`, so the
/// field order is ours: the plain fields go first, before any bytes, which
/// means the server has them even if a later file is refused.
fn multipart(fields: &[(&str, &str)], files: &[Upload]) -> js_sys::Uint8Array {
    // Every line in a multipart body ends CRLF, per RFC 7578.
    const CRLF: &[u8] = b"\r\n";

    fn line(body: &mut Vec<u8>, text: &str) {
        body.extend_from_slice(text.as_bytes());
        body.extend_from_slice(CRLF);
    }

    let mut body: Vec<u8> = Vec::new();

    for (name, value) in fields {
        line(&mut body, &format!("--{BOUNDARY}"));
        line(&mut body, &format!("Content-Disposition: form-data; name=\"{name}\""));
        line(&mut body, "");
        line(&mut body, value);
    }

    for (index, file) in files.iter().enumerate() {
        let name = format!("file{index}");
        // Quotes and newlines in a filename would break out of the header, so
        // they are stripped rather than escaped — the server keeps its own
        // cleaned copy of the name regardless.
        let safe: String =
            file.filename.chars().filter(|c| !matches!(c, '"' | '\r' | '\n')).collect();
        let mime = file.mime_type.as_deref().unwrap_or("application/octet-stream");

        line(&mut body, &format!("--{BOUNDARY}"));
        line(
            &mut body,
            &format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{safe}\""),
        );
        line(&mut body, &format!("Content-Type: {mime}"));
        line(&mut body, "");
        body.extend_from_slice(&file.bytes);
        body.extend_from_slice(CRLF);
    }
    line(&mut body, &format!("--{BOUNDARY}--"));

    js_sys::Uint8Array::from(body.as_slice())
}

/// `POST /api/v1/import/csv` — imports a bank or card statement.
///
/// The options carry only the answers the user had to give; a tidy statement
/// sends an empty object. The server parses the bytes itself and never trusts
/// whatever this client made of them.
pub async fn import_csv(
    file: &Upload,
    options: &ImportOptions,
) -> ApiResult<ImportCsvReport> {
    let encoded = serde_json::to_string(options)
        .map_err(|e| ApiFailure { status: 0, message: format!("Could not encode options: {e}") })?;

    let body = multipart(&[("options", encoded.as_str())], std::slice::from_ref(file));
    let request = Request::post(&url("/import/csv"))
        .credentials(RequestCredentials::SameOrigin)
        .header("Content-Type", &format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(body)
        .map_err(ApiFailure::offline)?;
    send(request).await
}

/// `POST /api/v1/documents/{id}/extract` — reads a document and runs the
/// rules over it.
pub async fn extract_document(id: Uuid) -> ApiResult<ExtractionResponse> {
    let request = Request::post(&url(&format!("/documents/{id}/extract")))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send(request).await
}

/// `GET /api/v1/documents/{id}/extraction` — what the last read found.
///
/// A 404 means the document has never been read, which is not an error worth
/// showing: the screen simply offers to read it.
pub async fn stored_extraction(id: Uuid) -> ApiResult<Option<StoredExtractionResponse>> {
    match get::<StoredExtractionResponse>(&format!("/documents/{id}/extraction")).await {
        Ok(found) => Ok(Some(found)),
        Err(failure) if failure.status == 404 => Ok(None),
        Err(failure) => Err(failure),
    }
}

/// `PATCH /api/v1/documents/{id}` — links a document, or moves its status.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DocumentPatch {
    /// `Some(None)` clears the link; `None` leaves it as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_transaction_id: Option<Option<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

pub async fn patch_document(id: Uuid, patch: &DocumentPatch) -> ApiResult<()> {
    let request = Request::patch(&url(&format!("/documents/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .json(patch)
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

/// `DELETE /api/v1/documents/{id}` — removes the row and the bytes behind it.
pub async fn delete_document(id: Uuid) -> ApiResult<()> {
    let request = Request::delete(&url(&format!("/documents/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

/// Where the browser can fetch a stored document.
///
/// A plain URL rather than a fetch: the server streams it with the right
/// `Content-Disposition`, so an ordinary link downloads it and the on-disk
/// layout stays private.
pub fn document_download_url(id: Uuid) -> String {
    url(&format!("/documents/{id}/download"))
}

/// The fields of a document extraction rule, as the form sends them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtractionRuleInput {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename_pattern: Option<String>,
    pub body_regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merchant_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_group: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_category_id: Option<Uuid>,
    pub enabled: bool,
}

/// `POST /api/v1/extraction-rules`.
pub async fn create_extraction_rule(
    input: &ExtractionRuleInput,
) -> ApiResult<DocumentExtractionRule> {
    post("/extraction-rules", input).await
}

/// `PATCH /api/v1/extraction-rules/{id}` — only the enabled flag, for the
/// toggle. A full edit deletes and recreates, so the server validates the
/// whole rule rather than a patchwork of it.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractionRuleToggle {
    pub enabled: bool,
}

pub async fn toggle_extraction_rule(id: Uuid, enabled: bool) -> ApiResult<()> {
    let request = Request::patch(&url(&format!("/extraction-rules/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .json(&ExtractionRuleToggle { enabled })
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

/// `DELETE /api/v1/extraction-rules/{id}`.
pub async fn delete_extraction_rule(id: Uuid) -> ApiResult<()> {
    let request = Request::delete(&url(&format!("/extraction-rules/{id}")))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

/// `POST /api/v1/auth/logout`.
pub async fn logout() -> ApiResult<()> {
    let request = Request::post(&url("/auth/logout"))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send_discarding(request).await
}

fn url(path: &str) -> String {
    format!("{API}{path}")
}

async fn get<T: DeserializeOwned>(path: &str) -> ApiResult<T> {
    let request = Request::get(&url(path))
        .credentials(RequestCredentials::SameOrigin)
        .build()
        .map_err(ApiFailure::offline)?;
    send(request).await
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> ApiResult<T> {
    let request = Request::post(&url(path))
        .credentials(RequestCredentials::SameOrigin)
        .json(body)
        .map_err(ApiFailure::offline)?;
    send(request).await
}

async fn put<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> ApiResult<T> {
    let request = Request::put(&url(path))
        .credentials(RequestCredentials::SameOrigin)
        .json(body)
        .map_err(ApiFailure::offline)?;
    send(request).await
}

async fn send<T: DeserializeOwned>(request: Request) -> ApiResult<T> {
    let response = request.send().await.map_err(ApiFailure::offline)?;
    let status = response.status();

    if !(200..300).contains(&status) {
        return Err(failure_from(status, response).await);
    }

    response
        .json::<T>()
        .await
        .map_err(|e| ApiFailure { status, message: format!("Unreadable reply: {e}") })
}

async fn send_discarding(request: Request) -> ApiResult<()> {
    let response = request.send().await.map_err(ApiFailure::offline)?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(failure_from(status, response).await);
    }
    Ok(())
}

/// Turns an error response into an [`ApiFailure`].
///
/// The backend renders every failure as `{status, error, message}`, so the
/// message is usually worth showing verbatim. If the body isn't that shape —
/// a proxy returning HTML, say — fall back to naming the status.
async fn failure_from(status: u16, response: gloo_net::http::Response) -> ApiFailure {
    let message = match response.json::<ErrorResponse>().await {
        Ok(body) if !body.message.is_empty() => body.message,
        _ => format!("The server returned HTTP {status}."),
    };
    ApiFailure { status, message }
}
