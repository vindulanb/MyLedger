//! The MyLedger backend.
//!
//! A library as well as a binary, so the integration tests can build the same
//! router the binary serves and drive it directly.
//!
//! Routing is arranged so that authentication is structural rather than
//! remembered: `/api/v1` is assembled from a public sub-router holding only
//! login, and a protected sub-router carrying the auth middleware. Adding a
//! route to the protected tree is the default; leaving one unprotected takes a
//! deliberate edit in the one place where that is visible.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod text;
pub mod extract;

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, patch, post, put};
use axum::{Router, middleware};
use myledger_core::{APP_NAME, now_timestamp};
use myledger_db::auth as db_auth;
use myledger_db::{Db, DbError, seed};
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::auth::MIN_PASSWORD_LENGTH;
use crate::config::Config;
use crate::error::{ApiError, ErrorBody};

/// What every handler is handed.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        Self { db, config: Arc::new(config) }
    }
}

/// Opens the database, migrates it, seeds the starter lookups and clears out
/// expired sessions.
///
/// Every part is idempotent, so this is what runs on every startup. Failures
/// come back as a message rather than a typed error: nothing can recover from
/// the database being unopenable, so the only consumer is the text the binary
/// prints before exiting.
pub async fn open_database(db_path: &Path) -> Result<Db, String> {
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("could not create the database directory {}: {e}", parent.display())
        })?;
    }

    open_and_prepare(db_path).await.map_err(|e: DbError| {
        format!("could not open the database at {}: {e}", db_path.display())
    })
}

async fn open_and_prepare(db_path: &Path) -> Result<Db, DbError> {
    let db = Db::connect(db_path).await?;
    let outcome = seed::seed_starter_data(&db).await?;
    if !outcome.is_empty() {
        tracing::info!(
            categories = outcome.categories_inserted,
            accounts = outcome.accounts_inserted,
            settings = outcome.settings_inserted,
            "seeded starter data"
        );
    }

    let pruned = db_auth::delete_expired_sessions(&db, &now_timestamp()).await?;
    if pruned > 0 {
        tracing::info!(pruned, "removed expired sessions");
    }

    Ok(db)
}

/// Creates the one and only user.
///
/// Refuses if a user already exists — this is a first-run action, not a way to
/// add a second account to a single-user app.
pub async fn create_user(db: &Db, username: &str, password: &str) -> Result<String, String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("the username must not be empty".to_string());
    }
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(format!("the password must be at least {MIN_PASSWORD_LENGTH} characters"));
    }
    if db_auth::count_users(db).await.map_err(|e| e.to_string())? > 0 {
        return Err("this ledger already has a user; setup only runs once".to_string());
    }

    let hash = auth::hash_password(password).map_err(|e| e.to_string())?;
    let user = db_auth::insert_user(db, username, &hash).await.map_err(|e| e.to_string())?;
    Ok(user.username)
}

/// Builds the router.
pub fn build_app(state: AppState) -> Router {
    // Everything here is behind `require_auth`. `route_layer` rather than
    // `layer`, so a request to a path that doesn't exist gets a 404 instead of
    // being told it is unauthenticated.
    //
    // Path parameters use axum 0.8's `{id}` syntax; these are the routes the
    // spec writes as `/transactions/:id`.
    let protected = Router::new()
        .route("/me", get(auth::routes::me))
        .route("/auth/logout", post(auth::routes::logout))
        .route(
            "/auth/device-tokens",
            post(auth::routes::create_device_token).get(auth::routes::list_device_tokens),
        )
        .route("/state", get(api::state::get_state).delete(api::state::wipe_state))
        .route("/transactions", post(api::transactions::create_transactions))
        .route(
            "/transactions/{id}",
            patch(api::transactions::patch_transaction)
                .delete(api::transactions::delete_transaction),
        )
        .route("/preferences", put(api::preferences::put_preferences))
        // Its own body limit, as uploads need: a statement is bigger than
        // JSON but much smaller than a receipt photo.
        .route(
            "/import/csv",
            post(api::import::import_csv)
                .layer(DefaultBodyLimit::max(api::import::MAX_REQUEST_BYTES)),
        )
        .route("/recurring/suggestions", get(api::recurring::get_suggestions))
        .route("/recurring/{pattern_key}/keep", post(api::recurring::keep_suggestion))
        .route("/recurring/{pattern_key}/ignore", post(api::recurring::ignore_suggestion))
        .route(
            "/recurring-items",
            get(api::recurring::list_items).post(api::recurring::create_item),
        )
        .route(
            "/recurring-items/{id}",
            get(api::recurring::get_item)
                .patch(api::recurring::patch_item)
                .delete(api::recurring::delete_item),
        )
        // The body-limit layer is raised only for uploads. Everywhere else keeps
        // axum's small default, which is the right size for JSON.
        .route(
            "/documents",
            post(api::documents::upload_documents)
                .layer(DefaultBodyLimit::max(api::documents::MAX_REQUEST_BYTES)),
        )
        .route(
            "/documents/{id}",
            get(api::documents::get_document)
                .patch(api::documents::patch_document)
                .delete(api::documents::delete_document),
        )
        .route("/documents/{id}/download", get(api::documents::download_document))
        .route("/documents/{id}/extract", post(api::extraction::extract_document))
        .route("/documents/{id}/extraction", get(api::extraction::get_extraction))
        .route(
            "/extraction-rules",
            get(api::extraction::list_rules).post(api::extraction::create_rule),
        )
        .route(
            "/extraction-rules/{id}",
            get(api::extraction::get_rule)
                .patch(api::extraction::patch_rule)
                .delete(api::extraction::delete_rule),
        )
        .route(
            "/sms-rules",
            get(api::sms_rules::list_rules).post(api::sms_rules::create_rule),
        )
        .route(
            "/sms-rules/{id}",
            get(api::sms_rules::get_rule)
                .patch(api::sms_rules::patch_rule)
                .delete(api::sms_rules::delete_rule),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::middleware::require_auth));

    // The only unauthenticated route under /api/v1: you cannot sign in if
    // signing in requires being signed in.
    let public = Router::new().route("/auth/login", post(auth::routes::login));

    // The API keeps its own JSON 404, so an unknown `/api/v1/...` path is
    // never answered with the single-page app's HTML — which would leave a
    // client parsing a document where it expected an error.
    let api = public.merge(protected).fallback(not_found);

    let base = Router::new().nest("/api/v1", api).route("/health", get(health));

    // Unknown paths fall back to `index.html` rather than 404ing, because the
    // single-page app owns its own routing: a browser reloading
    // `/transactions` has to be handed the app, which then reads the URL for
    // itself. `/api/v1/...` is unaffected — the API set its own fallback above.
    let web_dir = state.config.web_dir.clone();
    let index = web_dir.join("index.html");
    let with_ui = if index.is_file() {
        tracing::info!(path = %web_dir.display(), "serving the web app");
        base.fallback_service(ServeDir::new(&web_dir).fallback(ServeFile::new(index)))
    } else {
        // Not an error: the API is perfectly usable on its own, and a paired
        // phone never needs these files at all.
        tracing::warn!(
            path = %web_dir.display(),
            "no built web app found; the API is running but there is no UI to serve"
        );
        base.fallback(web_app_missing)
    };

    with_ui.layer(TraceLayer::new_for_http()).with_state(state)
}

/// Stands in for the UI when it hasn't been built.
///
/// Plain HTML with the actual command in it, because whoever sees this is in a
/// browser expecting an app and a JSON error would tell them nothing.
async fn web_app_missing(State(state): State<AppState>) -> impl IntoResponse {
    // A raw string so the markup reads as markup, with placeholders filled by
    // `replace` rather than `format!` — which would need every CSS brace in
    // here doubled. The placeholders are underscore-delimited so that neither
    // can be a substring of real text on the page: plain `WEB_DIR` would also
    // match inside `MYLEDGER_WEB_DIR`.
    const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>__APP__ — interface not built</title>
<style>
  body { font: 16px/1.6 system-ui, sans-serif; max-width: 40rem;
         margin: 4rem auto; padding: 0 1.5rem; color: #1c1917; background: #fafaf9 }
  code, pre { font-family: ui-monospace, monospace }
  pre { background: #f5f5f4; padding: .9rem 1.1rem; border-radius: .5rem; overflow-x: auto }
  @media (prefers-color-scheme: dark) {
    body { color: #e7e5e4; background: #1c1917 }
    pre { background: #292524 }
  }
</style>
</head>
<body>
<h1>__APP__ is running</h1>
<p>The API is up, but the web interface hasn't been built yet. Build it with:</p>
<pre>cd apps/web &amp;&amp; dx build --release</pre>
<p>Then point <code>MYLEDGER_WEB_DIR</code> at the output directory and restart.
It is currently looking in <code>__DIR__</code>.</p>
<p>The API itself is available under <code>/api/v1</code>.</p>
</body>
</html>
"#;

    Html(
        PAGE.replace("__APP__", APP_NAME)
            .replace("__DIR__", &state.config.web_dir.display().to_string()),
    )
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    /// Whether first-run setup has been done.
    configured: bool,
}

/// Liveness check. Outside `/api/v1` and unauthenticated on purpose, so a
/// monitor or a reverse proxy can use it without holding a credential. It
/// reveals only whether the server is up and whether setup has happened.
async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<Health>, ApiError> {
    let configured = db_auth::count_users(&state.db).await? > 0;
    Ok(Json(Health { status: "ok", configured }))
}

/// Unknown paths answer in JSON like everything else.
async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody {
            status: StatusCode::NOT_FOUND.as_u16(),
            error: "not_found",
            message: "no such endpoint".to_string(),
        }),
    )
}
