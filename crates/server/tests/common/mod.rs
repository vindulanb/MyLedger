//! Shared harness for the integration tests.
//!
//! The router is driven directly rather than over a socket, so a test is a
//! request in and a response out. Cookies are attached by hand, which is the
//! point: what's exercised is exactly what the server puts in `Set-Cookie` and
//! accepts back in `Cookie`.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use myledger_db::Db;
use myledger_server::config::{Config, SESSION_COOKIE};
use myledger_server::{AppState, build_app, create_user, open_database};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

pub const USERNAME: &str = "ledger-owner";
pub const PASSWORD: &str = "a-long-enough-password";

pub fn test_config(db_path: &std::path::Path) -> Config {
    Config {
        db_path: db_path.to_path_buf(),
        data_dir: db_path.parent().unwrap().join("documents"),
        // No built UI in tests: the server should still serve the API, and
        // answer non-API paths with the "not built" page.
        web_dir: db_path.parent().unwrap().join("web-dist"),
        bind: "127.0.0.1:0".parse().unwrap(),
        cookie_secure: false,
    }
}

/// A migrated, seeded database with setup done, and the router over it.
pub async fn app_with_user() -> (Router, Db, TempDir) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("myledger.sqlite");
    let db = open_database(&path).await.expect("open database");
    create_user(&db, USERNAME, PASSWORD).await.expect("create user");
    (build_app(AppState::new(db.clone(), test_config(&path))), db, dir)
}

/// The response, unpacked into the parts a test looks at.
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl Reply {
    pub fn set_cookie(&self) -> Option<&str> {
        self.headers.get(SET_COOKIE)?.to_str().ok()
    }

    /// The session token the server just handed out.
    pub fn session_token(&self) -> String {
        let cookie = self.set_cookie().expect("expected a Set-Cookie header");
        cookie
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, value)| value.to_string())
            .expect("malformed Set-Cookie")
    }

    /// Asserts the status, printing the body when it doesn't match — otherwise
    /// a failure tells you a number and nothing about why.
    #[track_caller]
    pub fn expect(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "unexpected status; body was {}", self.body);
        self.body
    }
}

pub async fn call(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.expect("router should not fail");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.expect("read body").to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!("response was not JSON ({e}): {:?}", String::from_utf8_lossy(&bytes))
        })
    };
    Reply { status, headers, body }
}

/// Sends a request without insisting the reply is JSON.
///
/// Needed for the pages the server serves to a browser — the web app itself,
/// and the stand-in page shown when it hasn't been built.
pub async fn call_raw(app: &Router, request: Request<Body>) -> RawReply {
    let response = app.clone().oneshot(request).await.expect("router should not fail");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.expect("read body").to_bytes();
    RawReply { status, headers, bytes: bytes.to_vec() }
}

pub fn request_with_json(method: &str, path: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn request(method: &str, path: &str) -> Request<Body> {
    Request::builder().method(method).uri(path).body(Body::empty()).unwrap()
}

pub fn post_json(path: &str, body: Value) -> Request<Body> {
    request_with_json("POST", path, &body)
}

pub fn get(path: &str) -> Request<Body> {
    request("GET", path)
}

pub fn with_cookie(request: Request<Body>, token: &str) -> Request<Body> {
    let (mut parts, body) = request.into_parts();
    parts.headers.insert(COOKIE, format!("{SESSION_COOKIE}={token}").parse().unwrap());
    Request::from_parts(parts, body)
}

pub fn with_bearer(request: Request<Body>, token: &str) -> Request<Body> {
    let (mut parts, body) = request.into_parts();
    parts.headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    Request::from_parts(parts, body)
}

/// Signs in and returns the session token.
pub async fn sign_in(app: &Router) -> String {
    let reply = call(
        app,
        post_json("/api/v1/auth/login", json!({ "username": USERNAME, "password": PASSWORD })),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "login failed: {}", reply.body);
    reply.session_token()
}

/// Signs in and pairs a device, returning the raw device token.
pub async fn pair_device(app: &Router, session: &str) -> String {
    let reply = call(
        app,
        with_cookie(post_json("/api/v1/auth/device-tokens", json!({ "label": "Pixel" })), session),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "pairing failed: {}", reply.body);
    reply.body["token"].as_str().expect("a token in the response").to_string()
}

/// A signed-in client, for tests about data rather than about credentials.
pub struct Client {
    pub app: Router,
    pub db: Db,
    pub session: String,
    /// Held so the temporary database outlives the test.
    dir: TempDir,
}

impl Client {
    pub async fn start() -> Self {
        let (app, db, dir) = app_with_user().await;
        let session = sign_in(&app).await;
        Self { app, db, session, dir }
    }

    /// Where uploaded documents land, for tests that check the disk directly.
    pub fn data_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("documents")
    }

    async fn send(&self, request: Request<Body>) -> Reply {
        call(&self.app, with_cookie(request, &self.session)).await
    }

    pub async fn get(&self, path: &str) -> Reply {
        self.send(request("GET", path)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.send(request_with_json("POST", path, &body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Reply {
        self.send(request_with_json("PATCH", path, &body)).await
    }

    pub async fn put(&self, path: &str, body: Value) -> Reply {
        self.send(request_with_json("PUT", path, &body)).await
    }

    pub async fn delete(&self, path: &str) -> Reply {
        self.send(request("DELETE", path)).await
    }

    /// A `DELETE` carrying a body — which the wipe requires, as its guard.
    pub async fn delete_with(&self, path: &str, body: Value) -> Reply {
        self.send(request_with_json("DELETE", path, &body)).await
    }

    /// Uploads files as `multipart/form-data`.
    pub async fn upload(&self, path: &str, parts: &[Part<'_>]) -> Reply {
        self.send(multipart_request(path, parts)).await
    }

    /// A raw response, for the cases where the body isn't JSON — a file download.
    pub async fn raw(&self, path: &str) -> RawReply {
        let request = with_cookie(request("GET", path), &self.session);
        let response = self.app.clone().oneshot(request).await.expect("router should not fail");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.expect("read body").to_bytes();
        RawReply { status, headers, bytes: bytes.to_vec() }
    }

    /// `GET /state`, asserting it succeeded.
    pub async fn state(&self) -> Value {
        self.get("/api/v1/state").await.expect(StatusCode::OK)
    }

    /// The ID of a seeded category, by name.
    pub async fn category_id(&self, name: &str) -> String {
        find_named(&self.state().await["categories"], name)
    }

    /// The ID of a seeded account, by name.
    pub async fn account_id(&self, name: &str) -> String {
        find_named(&self.state().await["accounts"], name)
    }
}

/// A response whose body is not JSON.
pub struct RawReply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
}

impl RawReply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }
}

/// One part of a multipart body: either a file or a plain form field.
pub struct Part<'a> {
    pub name: &'a str,
    /// `Some` makes this a file part.
    pub filename: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub bytes: Vec<u8>,
}

impl<'a> Part<'a> {
    pub fn file(name: &'a str, filename: &'a str, content_type: &'a str, bytes: Vec<u8>) -> Self {
        Self { name, filename: Some(filename), content_type: Some(content_type), bytes }
    }

    pub fn field(name: &'a str, value: &str) -> Self {
        Self { name, filename: None, content_type: None, bytes: value.as_bytes().to_vec() }
    }
}

/// Builds a `multipart/form-data` request by hand.
///
/// Assembled rather than pulled from a crate so that the bytes on the wire are
/// visible in the test — including, deliberately, an oversized file.
pub fn multipart_request(path: &str, parts: &[Part<'_>]) -> Request<Body> {
    const BOUNDARY: &str = "----myledgertestboundary7f3a";
    let mut body: Vec<u8> = Vec::new();

    for part in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match part.filename {
            Some(filename) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{filename}\"\r\n",
                    part.name
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{}\"\r\n", part.name).as_bytes(),
            ),
        }
        if let Some(content_type) = part.content_type {
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&part.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri(path)
        .header(CONTENT_TYPE, format!("multipart/form-data; boundary={BOUNDARY}"))
        .body(Body::from(body))
        .unwrap()
}

/// Picks the `id` of the entry called `name` out of a list.
#[track_caller]
pub fn find_named(list: &Value, name: &str) -> String {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list, got {list}"))
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("no entry called {name:?} in {list}"))["id"]
        .as_str()
        .expect("an id")
        .to_string()
}
