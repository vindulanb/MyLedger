//! Tests for serving the built web app alongside the API.
//!
//! Two things have to stay true however the routing is rearranged: an unknown
//! `/api/v1/...` path answers in JSON, and an unknown *page* path answers with
//! the single-page app. Getting that backwards means a client parsing HTML
//! where it expected an error, or a browser reload showing a JSON 404.

mod common;

use axum::Router;
use axum::http::StatusCode;
use common::{PASSWORD, USERNAME, call, call_raw, get, request_with_json, sign_in, with_cookie};
use myledger_server::config::Config;
use myledger_server::{AppState, build_app, create_user, open_database};
use serde_json::json;
use tempfile::TempDir;

/// An app whose web directory holds whatever `files` describes.
async fn app_with_web_dir(files: &[(&str, &str)]) -> (Router, TempDir) {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("myledger.sqlite");
    let web_dir = dir.path().join("web-dist");

    if !files.is_empty() {
        std::fs::create_dir_all(web_dir.join("assets")).unwrap();
        for (name, contents) in files {
            std::fs::write(web_dir.join(name), contents).unwrap();
        }
    }

    let db = open_database(&db_path).await.unwrap();
    create_user(&db, USERNAME, PASSWORD).await.unwrap();

    let config = Config {
        db_path: db_path.clone(),
        data_dir: dir.path().join("documents"),
        web_dir,
        bind: "127.0.0.1:0".parse().unwrap(),
        cookie_secure: false,
    };
    (build_app(AppState::new(db, config)), dir)
}

#[tokio::test]
async fn a_built_app_is_served_at_the_root() {
    let (app, _dir) = app_with_web_dir(&[
        ("index.html", "<!doctype html><title>MyLedger</title><div id=main></div>"),
        ("assets/main.css", "body{margin:0}"),
    ])
    .await;

    let index = call_raw(&app, get("/")).await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&index.bytes).contains("<div id=main>"),
        "the root should serve index.html itself"
    );

    let css = call_raw(&app, get("/assets/main.css")).await;
    assert_eq!(css.status, StatusCode::OK);
    assert_eq!(css.bytes, b"body{margin:0}");
    assert!(
        css.header("content-type").is_some_and(|t| t.starts_with("text/css")),
        "a stylesheet has to arrive as CSS or the browser ignores it; got {:?}",
        css.header("content-type")
    );
}

#[tokio::test]
async fn an_unknown_page_falls_back_to_the_app_so_a_reload_works() {
    let (app, _dir) = app_with_web_dir(&[("index.html", "<!doctype html><title>MyLedger</title>")])
        .await;

    // The single-page app owns its own routing: reloading /transactions has to
    // hand the browser the app, which then reads the URL for itself.
    for path in ["/transactions", "/settings", "/rules", "/deeply/nested/thing"] {
        let reply = call_raw(&app, get(path)).await;
        assert_eq!(reply.status, StatusCode::OK, "{path} should serve the app");
        assert!(
            String::from_utf8_lossy(&reply.bytes).contains("<title>MyLedger</title>"),
            "{path} should have been handed index.html"
        );
    }
}

#[tokio::test]
async fn the_api_keeps_its_json_404_even_with_a_built_app_present() {
    let (app, _dir) = app_with_web_dir(&[("index.html", "<!doctype html><title>MyLedger</title>")])
        .await;

    let reply = call(&app, get("/api/v1/nothing-here")).await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(
        reply.body["error"], "not_found",
        "an unknown API path must not be answered with the app's HTML"
    );
}

#[tokio::test]
async fn without_a_built_app_the_api_still_works_and_the_root_explains_itself() {
    let (app, _dir) = app_with_web_dir(&[]).await;

    // The page a browser gets is HTML with the build command in it, not a bare
    // 404 — whoever sees it is expecting an app, and JSON would tell them
    // nothing.
    let root = call_raw(&app, get("/")).await;
    assert_eq!(root.status, StatusCode::OK);
    let page = String::from_utf8_lossy(&root.bytes);
    assert!(page.contains("dx build --release"), "it should name the actual command: {page}");
    assert!(page.contains("MYLEDGER_WEB_DIR"), "and the env var that points at the output");

    // The API is entirely unaffected: a paired phone never needs these files.
    let session = sign_in(&app).await;
    let state = call(&app, with_cookie(get("/api/v1/state"), &session)).await;
    assert_eq!(state.status, StatusCode::OK);
    assert_eq!(state.body["transactions"], json!([]));

    let health = call(&app, get("/health")).await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.body["configured"], true);
}

#[tokio::test]
async fn static_assets_are_public_but_the_api_is_not() {
    let (app, _dir) = app_with_web_dir(&[("index.html", "<!doctype html><title>MyLedger</title>")])
        .await;

    // The app's own files have to load before anyone can sign in.
    assert_eq!(call_raw(&app, get("/")).await.status, StatusCode::OK);

    // Everything behind them still needs credentials.
    assert_eq!(call(&app, get("/api/v1/state")).await.status, StatusCode::UNAUTHORIZED);
    let wipe = call(&app, request_with_json("DELETE", "/api/v1/state", &json!({}))).await;
    assert_eq!(wipe.status, StatusCode::UNAUTHORIZED);
}
