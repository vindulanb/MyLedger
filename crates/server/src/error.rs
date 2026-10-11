//! API errors.
//!
//! Every failure leaves the server as a JSON body with a status code, a stable
//! machine-readable `error` slug and a human-readable `message`. Nothing
//! panics its way to the client, and nothing internal leaks: a database failure
//! is logged in full here and reported as a bare "internal error" outside.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use myledger_db::DbError;
use serde::Serialize;

/// A failure on its way back to the client.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// No usable credentials, or credentials that don't check out.
    #[error("{0}")]
    Unauthorized(String),

    /// Authenticated, but not with the kind of credential this route needs.
    #[error("{0}")]
    Forbidden(String),

    /// The request itself was malformed or failed validation.
    #[error("{0}")]
    BadRequest(String),

    /// No such thing.
    #[error("{0}")]
    NotFound(String),

    /// The request was fine but conflicts with what is already stored.
    #[error("{0}")]
    Conflict(String),

    /// The upload was bigger than the server will accept.
    #[error("{0}")]
    TooLarge(String),

    /// Something broke on our side. The detail is logged, never sent.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::TooLarge(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable slug a client can branch on without parsing prose.
    fn slug(&self) -> &'static str {
        match self {
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::BadRequest(_) => "bad_request",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::TooLarge(_) => "payload_too_large",
            Self::Internal(_) => "internal_error",
        }
    }
}

/// The JSON body every error is rendered as.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub status: u16,
    pub error: &'static str,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();

        // An internal error is the one case where what the client is told and
        // what actually happened differ, so it is the one case that must be
        // logged before the detail is discarded.
        let message = match &self {
            Self::Internal(detail) => {
                tracing::error!(detail, "request failed");
                "something went wrong on the server".to_string()
            }
            other => other.to_string(),
        };

        let body = ErrorBody { status: status.as_u16(), error: self.slug(), message };
        (status, Json(body)).into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            // Not really a failure — the duplicate guard doing its job. Routes
            // that expect it intercept it before reaching here; anything else
            // treating it as a conflict is the honest answer.
            DbError::DuplicateFingerprint => {
                Self::conflict("a transaction with this fingerprint already exists")
            }
            other => Self::internal(other.to_string()),
        }
    }
}
