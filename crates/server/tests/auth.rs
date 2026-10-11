//! End-to-end tests of the authentication surface.
//!
//! The router is driven directly rather than over a socket, so each test is a
//! request in and a response out with no port juggling. Cookies are handled by
//! hand, which is the point: what's under test is exactly what the server puts
//! in `Set-Cookie` and what it accepts back in `Cookie`.

mod common;

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use common::{
    PASSWORD, USERNAME, app_with_user, call, get, pair_device, post_json, sign_in, test_config,
    with_bearer, with_cookie,
};
use myledger_db::Db;
use myledger_server::config::SESSION_COOKIE;
use myledger_server::{AppState, build_app, create_user, open_database};
use serde_json::{Value, json};
use tempfile::TempDir;

// ---- credentials -----------------------------------------------------------

#[tokio::test]
async fn a_correct_password_starts_a_session() {
    let (app, _db, _dir) = app_with_user().await;

    let reply = call(
        &app,
        post_json("/api/v1/auth/login", json!({ "username": USERNAME, "password": PASSWORD })),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["username"], USERNAME);
    assert!(reply.body["expires_at"].is_string());

    let cookie = reply.set_cookie().expect("a session cookie");
    assert!(cookie.starts_with(SESSION_COOKIE), "unexpected cookie: {cookie}");
    assert!(cookie.contains("HttpOnly"), "script must not be able to read it: {cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_user_are_indistinguishable() {
    let (app, _db, _dir) = app_with_user().await;

    let wrong_password = call(
        &app,
        post_json("/api/v1/auth/login", json!({ "username": USERNAME, "password": "nope" })),
    )
    .await;
    let unknown_user = call(
        &app,
        post_json("/api/v1/auth/login", json!({ "username": "nobody", "password": PASSWORD })),
    )
    .await;

    assert_eq!(wrong_password.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_user.status, StatusCode::UNAUTHORIZED);
    // Same message either way: a probe must not learn which half was right.
    assert_eq!(wrong_password.body["message"], unknown_user.body["message"]);
    assert!(wrong_password.set_cookie().is_none(), "a failed login must not set a cookie");
}

#[tokio::test]
async fn errors_come_back_as_structured_json() {
    let (app, _db, _dir) = app_with_user().await;

    let reply = call(&app, get("/api/v1/me")).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.body["status"], 401);
    assert_eq!(reply.body["error"], "unauthorized");
    assert!(reply.body["message"].as_str().is_some_and(|m| !m.is_empty()));
}

// ---- the two credential types ---------------------------------------------

#[tokio::test]
async fn a_protected_route_accepts_a_session_a_device_token_and_nothing_else() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;
    let device = pair_device(&app, &session).await;

    let by_session = call(&app, with_cookie(get("/api/v1/me"), &session)).await;
    assert_eq!(by_session.status, StatusCode::OK);
    assert_eq!(by_session.body["username"], USERNAME);
    assert_eq!(by_session.body["authenticated_via"], "session");

    let by_device = call(&app, with_bearer(get("/api/v1/me"), &device)).await;
    assert_eq!(by_device.status, StatusCode::OK);
    assert_eq!(by_device.body["username"], USERNAME);
    assert_eq!(by_device.body["authenticated_via"], "device_token");
    assert_eq!(by_device.body["device_label"], "Pixel");

    let with_nothing = call(&app, get("/api/v1/me")).await;
    assert_eq!(with_nothing.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn made_up_credentials_are_refused() {
    let (app, _db, _dir) = app_with_user().await;

    let bad_bearer = call(&app, with_bearer(get("/api/v1/me"), &"f".repeat(64))).await;
    assert_eq!(bad_bearer.status, StatusCode::UNAUTHORIZED);

    let bad_cookie = call(&app, with_cookie(get("/api/v1/me"), "not-a-real-token")).await;
    assert_eq!(bad_cookie.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn every_api_route_except_login_is_behind_the_gate() {
    let (app, _db, _dir) = app_with_user().await;

    for request in [
        get("/api/v1/me"),
        get("/api/v1/auth/device-tokens"),
        post_json("/api/v1/auth/device-tokens", json!({})),
        post_json("/api/v1/auth/logout", json!({})),
    ] {
        let path = request.uri().path().to_string();
        let reply = call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable unauthenticated");
    }

    // …and login itself is not, or nobody could ever get in.
    let login = call(
        &app,
        post_json("/api/v1/auth/login", json!({ "username": USERNAME, "password": PASSWORD })),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
}

// ---- device pairing --------------------------------------------------------

#[tokio::test]
async fn a_device_token_is_returned_once_and_never_again() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;

    let created = call(
        &app,
        with_cookie(post_json("/api/v1/auth/device-tokens", json!({ "label": "Pixel" })), &session),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK);

    let token = created.body["token"].as_str().unwrap();
    assert_eq!(token.len(), 64, "expected 32 random bytes as hex");
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

    let listed = call(&app, with_cookie(get("/api/v1/auth/device-tokens"), &session)).await;
    assert_eq!(listed.status, StatusCode::OK);
    let devices = listed.body.as_array().expect("an array of devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["label"], "Pixel");
    assert!(devices[0].get("token").is_none(), "the raw token must not be recoverable");
    assert!(devices[0].get("token_hash").is_none(), "nor its hash");
}

#[tokio::test]
async fn a_paired_device_cannot_pair_more_devices() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;
    let device = pair_device(&app, &session).await;

    // A stolen phone should cost you one device, not the whole ledger.
    let reply = call(
        &app,
        with_bearer(post_json("/api/v1/auth/device-tokens", json!({ "label": "Another" })), &device),
    )
    .await;

    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.body["error"], "forbidden");
}

#[tokio::test]
async fn pairing_works_without_a_request_body() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;

    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/device-tokens")
        .body(Body::empty())
        .unwrap();
    let reply = call(&app, with_cookie(request, &session)).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.body["token"].is_string());
    assert_eq!(reply.body["label"], Value::Null);
}

// ---- session lifetime ------------------------------------------------------

#[tokio::test]
async fn signing_out_revokes_the_session_server_side() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;

    let logout = call(&app, with_cookie(post_json("/api/v1/auth/logout", json!({})), &session)).await;
    assert_eq!(logout.status, StatusCode::OK);
    assert_eq!(logout.body["signed_out"], true);
    assert!(logout.set_cookie().unwrap().contains("Max-Age=0"), "the cookie should be cleared");

    // Presenting the old cookie again must fail — the row is gone, not merely
    // forgotten by the browser.
    let after = call(&app, with_cookie(get("/api/v1/me"), &session)).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_device_token_survives_the_browser_signing_out() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;
    let device = pair_device(&app, &session).await;

    call(&app, with_cookie(post_json("/api/v1/auth/logout", json!({})), &session)).await;

    let reply = call(&app, with_bearer(get("/api/v1/me"), &device)).await;
    assert_eq!(reply.status, StatusCode::OK, "signing out of the browser must not unpair a phone");
}

#[tokio::test]
async fn sessions_and_pairings_survive_a_restart() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("myledger.sqlite");

    let db = open_database(&path).await.unwrap();
    create_user(&db, USERNAME, PASSWORD).await.unwrap();
    let app = build_app(AppState::new(db.clone(), test_config(&path)));

    let session = sign_in(&app).await;
    let device = pair_device(&app, &session).await;

    // Stop the server and start a new one against the same file.
    drop(app);
    db.close().await;

    let restarted_db = Db::connect(&path).await.unwrap();
    let restarted = build_app(AppState::new(restarted_db, test_config(&path)));

    let by_session = call(&restarted, with_cookie(get("/api/v1/me"), &session)).await;
    assert_eq!(by_session.status, StatusCode::OK, "sessions must outlive the process");

    let by_device = call(&restarted, with_bearer(get("/api/v1/me"), &device)).await;
    assert_eq!(by_device.status, StatusCode::OK, "pairings must outlive the process too");
}

// ---- odds and ends ---------------------------------------------------------

#[tokio::test]
async fn health_is_public_and_says_whether_setup_has_happened() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("myledger.sqlite");
    let db = open_database(&path).await.unwrap();
    let app = build_app(AppState::new(db.clone(), test_config(&path)));

    let before = call(&app, get("/health")).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(before.body["configured"], false);

    create_user(&db, USERNAME, PASSWORD).await.unwrap();

    let after = call(&app, get("/health")).await;
    assert_eq!(after.body["configured"], true);
}

#[tokio::test]
async fn setup_only_runs_once() {
    let dir = TempDir::new().unwrap();
    let db = open_database(&dir.path().join("myledger.sqlite")).await.unwrap();

    create_user(&db, USERNAME, PASSWORD).await.unwrap();
    let second = create_user(&db, "someone-else", PASSWORD).await;
    assert!(second.is_err(), "a single-user app must not grow a second user");

    assert!(create_user(&db, USERNAME, "short").await.is_err());
}

#[tokio::test]
async fn unknown_paths_and_bad_bodies_answer_in_json() {
    let (app, _db, _dir) = app_with_user().await;
    let session = sign_in(&app).await;

    let missing = call(&app, get("/api/v1/nothing-here")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body["error"], "not_found");

    let malformed = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let reply = call(&app, malformed).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.body["error"], "bad_request");

    // An authenticated route with a body that parses but is missing a field.
    let wrong_shape = call(
        &app,
        with_cookie(post_json("/api/v1/auth/login", json!({ "username": USERNAME })), &session),
    )
    .await;
    assert_eq!(wrong_shape.status, StatusCode::BAD_REQUEST);
}
