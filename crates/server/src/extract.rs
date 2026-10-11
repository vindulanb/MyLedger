//! Request extractors that fail the way the rest of the API does.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// [`Json`], but a malformed body comes back as an [`ApiError`].
///
/// Axum's own `Json` rejection is plain text, which would make bad JSON the one
/// failure a client couldn't parse the same way as every other.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::bad_request(rejection.body_text())),
        }
    }
}

/// Parses a request body that is allowed to be absent entirely.
///
/// A `POST` whose fields are all optional is reasonably sent with no body at
/// all, which is not valid JSON and would otherwise be a 400. An empty body
/// means "all defaults"; anything else has to parse.
pub fn optional_json<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::bad_request(format!("could not parse the request body: {e}")))
}
