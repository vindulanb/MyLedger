//! The gate in front of `/api/v1`.
//!
//! One middleware resolves whichever credential the request carried into an
//! [`Identity`] and stashes it on the request. Handlers that need to know who
//! is calling take `Identity` as an extractor; the rest never think about it.
//!
//! Anything that fails to resolve gets a 401 — no partial states, no routes
//! quietly readable without credentials.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use myledger_core::{now_timestamp, timestamp_after};
use myledger_db::auth as db_auth;

use crate::AppState;
use crate::auth::{Credential, Identity, hash_token, read_session_cookie};
use crate::config::SESSION_TTL;
use crate::error::ApiError;

/// Rejects any request that can't prove who it is.
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let identity = authenticate(&state, request.headers()).await?;
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

/// Resolves a request's credentials, whichever kind it brought.
///
/// A bearer token is checked first: a client that sent one meant to use it, and
/// silently falling back to a cookie would make a revoked device look like it
/// still worked.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Identity, ApiError> {
    if let Some(token) = bearer_token(headers) {
        return authenticate_device(state, &token).await;
    }
    if let Some(token) = read_session_cookie(headers) {
        return authenticate_session(state, &token).await;
    }
    Err(ApiError::unauthorized(
        "sign in, or send a paired device token as `Authorization: Bearer <token>`",
    ))
}

/// A paired device.
async fn authenticate_device(state: &AppState, token: &str) -> Result<Identity, ApiError> {
    let token_hash = hash_token(token);
    let Some(record) = db_auth::find_device_token(&state.db, &token_hash).await? else {
        return Err(ApiError::unauthorized("that device token is not valid"));
    };

    // The ledger has exactly one owner, and a device token is bound to the
    // ledger rather than to a person.
    let Some(user) = db_auth::find_only_user(&state.db).await? else {
        return Err(ApiError::unauthorized("this server has not been set up yet"));
    };

    db_auth::touch_device_token(&state.db, &token_hash).await?;

    Ok(Identity {
        user_id: user.id,
        username: user.username,
        credential: Credential::Device { token_hash, label: record.label },
    })
}

/// A signed-in browser.
async fn authenticate_session(state: &AppState, token: &str) -> Result<Identity, ApiError> {
    let token_hash = hash_token(token);
    let Some(session) = db_auth::find_session(&state.db, &token_hash).await? else {
        return Err(ApiError::unauthorized("your session has ended — sign in again"));
    };

    // Expiry is a string comparison, which holds because timestamps are
    // fixed-width UTC.
    if session.expires_at <= now_timestamp() {
        db_auth::delete_session(&state.db, &token_hash).await?;
        return Err(ApiError::unauthorized("your session has expired — sign in again"));
    }

    let Some(user) = db_auth::find_user_by_id(&state.db, session.user_id).await? else {
        // The user was removed but the session outlived them.
        db_auth::delete_session(&state.db, &token_hash).await?;
        return Err(ApiError::unauthorized("your session has ended — sign in again"));
    };

    // Sliding expiry: staying active keeps you signed in.
    db_auth::touch_session(&state.db, &token_hash, &timestamp_after(SESSION_TTL)).await?;

    Ok(Identity {
        user_id: user.id,
        username: user.username,
        credential: Credential::Session { token_hash },
    })
}

/// Pulls the token out of an `Authorization: Bearer …` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

impl<S> FromRequestParts<S> for Identity
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    /// Reads the identity [`require_auth`] put on the request.
    ///
    /// A miss means the route was mounted outside the authenticated tree, which
    /// is a wiring mistake rather than a client one — but the safe answer is
    /// still to refuse.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Identity>()
            .cloned()
            .ok_or_else(|| ApiError::unauthorized("this endpoint requires authentication"))
    }
}

#[cfg(test)]
mod tests {
    use super::bearer_token;
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    fn headers_with(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn a_bearer_token_is_read_whatever_the_case_of_the_scheme() {
        assert_eq!(bearer_token(&headers_with("Bearer abc123")), Some("abc123".to_string()));
        assert_eq!(bearer_token(&headers_with("bearer abc123")), Some("abc123".to_string()));
        assert_eq!(bearer_token(&headers_with("BEARER  abc123 ")), Some("abc123".to_string()));
    }

    #[test]
    fn other_schemes_and_empty_tokens_are_ignored() {
        assert_eq!(bearer_token(&headers_with("Basic abc123")), None);
        assert_eq!(bearer_token(&headers_with("Bearer ")), None);
        assert_eq!(bearer_token(&headers_with("abc123")), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }
}
