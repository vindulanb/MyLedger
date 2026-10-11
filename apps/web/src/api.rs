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
    ErrorResponse, LoginRequest, LoginResponse, MeResponse, PreferencesRequest,
    PreferencesResponse, StateResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
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
