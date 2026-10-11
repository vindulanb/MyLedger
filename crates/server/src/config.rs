//! Runtime configuration, all of it from the environment.
//!
//! Defaults are chosen for the common case: one person, one machine, listening
//! only on loopback until told otherwise.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Where to listen if `MYLEDGER_BIND` says nothing.
const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// How long a browser session lasts without use. Sliding: every authenticated
/// request pushes it back out.
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Name of the cookie holding the session token.
pub const SESSION_COOKIE: &str = "myledger_session";

/// Everything the server needs to know before it starts.
#[derive(Debug, Clone)]
pub struct Config {
    /// SQLite file. Created, with its parent directory, if absent.
    pub db_path: PathBuf,
    /// Directory uploaded documents are written to.
    pub data_dir: PathBuf,
    /// Address to listen on.
    pub bind: SocketAddr,
    /// Whether the session cookie is marked `Secure`.
    ///
    /// Off by default: MyLedger is usually reached over plain HTTP on a home
    /// network, and a `Secure` cookie there would simply never be sent, making
    /// login appear to succeed and then fail. Turn it on when serving over
    /// HTTPS, which you should if the server is reachable from outside.
    pub cookie_secure: bool,
}

impl Config {
    /// Reads the configuration from the environment.
    ///
    /// Returns the first problem found rather than falling back silently — a
    /// mistyped port should stop the server, not quietly move it.
    pub fn from_env() -> Result<Self, String> {
        let db_path = env::var(myledger_db::DB_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/myledger.sqlite"));

        let data_dir = env::var(myledger_db::DATA_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/documents"));

        let bind_raw = env::var("MYLEDGER_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
        let bind: SocketAddr = bind_raw
            .parse()
            .map_err(|e| format!("MYLEDGER_BIND is not a valid address ({bind_raw:?}): {e}"))?;

        let cookie_secure = match env::var("MYLEDGER_COOKIE_SECURE") {
            Ok(value) => parse_bool(&value)
                .ok_or_else(|| format!("MYLEDGER_COOKIE_SECURE must be true or false, got {value:?}"))?,
            Err(_) => false,
        };

        Ok(Self { db_path, data_dir, bind, cookie_secure })
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
