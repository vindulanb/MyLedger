//! Sign in, sign out, pair a device.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::SET_COOKIE;
use axum::response::IntoResponse;
use myledger_core::{
    CreateDeviceTokenRequest, CreateDeviceTokenResponse, DeviceTokenSummary, LoginRequest,
    LoginResponse, MeResponse, timestamp_after,
};
use myledger_db::auth as db_auth;
use serde::Serialize;

use crate::AppState;
use crate::auth::{
    Credential, Identity, cleared_session_cookie, generate_token, hash_token, session_cookie,
    verify_password, waste_time_like_a_real_check,
};
use crate::config::SESSION_TTL;
use crate::error::ApiError;
use crate::extract::{ApiJson, optional_json};

// The login, identity and device-pairing shapes all live in
// `myledger_core::wire`, because the web and mobile apps speak them too.

/// Exchanges a username and password for a session cookie.
pub async fn login(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<LoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // One message for both "no such user" and "wrong password", and the same
    // work done either way, so a probe learns nothing from either the answer
    // or how long it took.
    let rejected = || ApiError::unauthorized("that username and password don't match");

    let Some(user) = db_auth::find_user_by_username(&state.db, &request.username).await? else {
        waste_time_like_a_real_check(&request.password);
        return Err(rejected());
    };
    if !verify_password(&request.password, &user.password_hash) {
        return Err(rejected());
    }

    let token = generate_token()?;
    let expires_at = timestamp_after(SESSION_TTL);
    db_auth::insert_session(&state.db, &hash_token(&token), user.id, &expires_at).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        session_cookie(&token, SESSION_TTL.as_secs(), state.config.cookie_secure)
            .parse()
            .map_err(|_| ApiError::internal("could not build the session cookie"))?,
    );

    tracing::info!(username = %user.username, "signed in");
    Ok((headers, Json(LoginResponse { username: user.username, expires_at })))
}

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub signed_out: bool,
}

/// Ends the current session.
///
/// Called with a device token there is no session to end, so this clears the
/// cookie and reports that nothing was signed out — a phone cannot unpair
/// itself by accident, and the browser's cookie is cleared either way.
pub async fn logout(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<impl IntoResponse, ApiError> {
    let signed_out = match &identity.credential {
        Credential::Session { token_hash } => {
            db_auth::delete_session(&state.db, token_hash).await?
        }
        Credential::Device { .. } => false,
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        cleared_session_cookie(state.config.cookie_secure)
            .parse()
            .map_err(|_| ApiError::internal("could not build the session cookie"))?,
    );

    Ok((headers, Json(LogoutResponse { signed_out })))
}

/// Mints a pairing token for a new device.
///
/// Deliberately session-only. A phone holding a device token must not be able
/// to mint more of them: one stolen phone should cost you one device, not the
/// whole ledger.
pub async fn create_device_token(
    State(state): State<AppState>,
    identity: Identity,
    body: Bytes,
) -> Result<Json<CreateDeviceTokenResponse>, ApiError> {
    if !identity.credential.is_session() {
        return Err(ApiError::forbidden(
            "pairing a new device requires a signed-in browser, not a device token",
        ));
    }

    let request: CreateDeviceTokenRequest = optional_json(&body)?;
    let label = request
        .label
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty());

    let token = generate_token()?;
    let record =
        db_auth::insert_device_token(&state.db, &hash_token(&token), label.as_deref()).await?;

    tracing::info!(label = ?record.label, "paired a new device");
    Ok(Json(CreateDeviceTokenResponse {
        token,
        label: record.label,
        created_at: record.created_at,
        warning: "This token is shown once and cannot be recovered. Store it now."
            .to_string(),
    }))
}

/// Lists paired devices — without, of course, their tokens.
pub async fn list_device_tokens(
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceTokenSummary>>, ApiError> {
    let records = db_auth::list_device_tokens(&state.db).await?;
    Ok(Json(
        records
            .into_iter()
            .map(|record| DeviceTokenSummary {
                label: record.label,
                created_at: record.created_at,
                last_used_at: record.last_used_at,
            })
            .collect(),
    ))
}

/// Reports who the caller is. The smallest possible protected route, and what
/// a client calls on startup to find out whether it is still signed in.
pub async fn me(identity: Identity) -> Json<MeResponse> {
    let device_label = match &identity.credential {
        Credential::Device { label, .. } => label.clone(),
        Credential::Session { .. } => None,
    };
    Json(MeResponse {
        authenticated_via: identity.credential.kind().to_string(),
        username: identity.username,
        device_label,
    })
}
