//! Authentication.
//!
//! Two ways in, one identity out. A browser signs in with a password and gets a
//! session cookie; a phone is paired once and carries a bearer token forever.
//! Both resolve to the same [`Identity`], so no route has to care which was
//! used — except device pairing, which deliberately does.
//!
//! Secrets are handled the same way throughout: what the client holds is a long
//! random string, and what the database holds is only its hash. Losing a copy
//! of the database does not hand anyone a way in.
//!
//! Passwords get argon2id, because a password is low-entropy and a slow hash is
//! the whole defence. Session and device tokens get SHA-256, because they are
//! 256 bits of randomness — there is nothing to brute-force, and they are
//! verified on every single request, where argon2 would be a self-inflicted
//! denial of service.

pub mod middleware;
pub mod routes;

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::SESSION_COOKIE;
use crate::error::ApiError;

/// Bytes of randomness in a session or device token.
const TOKEN_BYTES: usize = 32;

/// Bytes of salt on a password hash. 16 is what argon2's own recommendation is.
const SALT_BYTES: usize = 16;

/// Shortest password the setup flow will accept.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Which credential a request arrived with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A browser session cookie.
    Session {
        /// Hash of the presented token, so logout knows which row to remove.
        token_hash: String,
    },
    /// A paired device's bearer token.
    Device {
        /// Hash of the presented token.
        token_hash: String,
        /// What the user named the device, if they named it.
        label: Option<String>,
    },
}

impl Credential {
    /// Whether this is a browser session.
    pub fn is_session(&self) -> bool {
        matches!(self, Self::Session { .. })
    }

    /// A short word for the kind of credential, for responses and logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session { .. } => "session",
            Self::Device { .. } => "device_token",
        }
    }
}

/// Who is making a request, and how they proved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user_id: Uuid,
    pub username: String,
    pub credential: Credential,
}

/// Generates a fresh secret: [`TOKEN_BYTES`] of randomness, hex-encoded.
///
/// Hex rather than base64 so a token can be read aloud or retyped without
/// worrying about case or padding.
pub fn generate_token() -> Result<String, ApiError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| ApiError::internal(format!("no source of randomness available: {e}")))?;
    Ok(hex(&bytes))
}

/// Hashes a high-entropy secret for storage and lookup.
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Hashes a password with argon2id and default parameters.
///
/// The salt is drawn from the same OS randomness as tokens are, rather than
/// through a random-number-generator crate, so there is one source of
/// randomness in the server and no second version of it to keep in step.
pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let mut salt_bytes = [0u8; SALT_BYTES];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| ApiError::internal(format!("no source of randomness available: {e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| ApiError::internal(format!("could not encode the password salt: {e}")))?;

    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| ApiError::internal(format!("hashing the password failed: {e}")))
}

/// Checks a password against a stored argon2 hash.
///
/// A hash that fails to parse is treated as "wrong password" rather than as an
/// error: it means the row is corrupt, and letting anyone in would be worse.
pub fn verify_password(password: &str, encoded_hash: &str) -> bool {
    match PasswordHash::new(encoded_hash) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Burns the same amount of time as a real password check.
///
/// Called when the username doesn't exist. Without it, a wrong username would
/// return noticeably faster than a wrong password, which tells an attacker
/// which half they got right.
pub fn waste_time_like_a_real_check(password: &str) {
    static DECOY: OnceLock<String> = OnceLock::new();
    let decoy = DECOY.get_or_init(|| {
        hash_password("a password nobody has").unwrap_or_else(|_| String::from("unusable"))
    });
    let _ = verify_password(password, decoy);
}

/// Reads the session cookie out of the request headers.
///
/// Hand-rolled rather than pulled from a cookie crate: exactly one cookie
/// matters, and the parsing is a split on `;` and `=`.
pub fn read_session_cookie(headers: &HeaderMap) -> Option<String> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim().to_string())
    })
}

/// Builds the `Set-Cookie` value that starts a session.
pub fn session_cookie(token: &str, max_age_seconds: u64, secure: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_seconds}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Builds the `Set-Cookie` value that ends one.
pub fn cleared_session_cookie(secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn a_password_verifies_against_its_own_hash_and_nothing_else() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$"), "expected argon2id, got {hash}");
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("Correct horse battery staple", &hash));
        assert!(!verify_password("", &hash));
    }

    #[test]
    fn hashing_the_same_password_twice_gives_different_hashes() {
        // Different salts. Two users with the same password must not be
        // visibly identical in the table.
        let first = hash_password("hunter2hunter2").unwrap();
        let second = hash_password("hunter2hunter2").unwrap();
        assert_ne!(first, second);
        assert!(verify_password("hunter2hunter2", &first));
        assert!(verify_password("hunter2hunter2", &second));
    }

    #[test]
    fn a_corrupt_hash_rejects_rather_than_admitting_everyone() {
        assert!(!verify_password("anything", "not-a-hash"));
        assert!(!verify_password("anything", ""));
    }

    #[test]
    fn generated_tokens_are_long_and_never_repeat() {
        let first = generate_token().unwrap();
        let second = generate_token().unwrap();
        assert_eq!(first.len(), TOKEN_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn hashing_a_token_is_stable_and_hides_the_token() {
        let token = generate_token().unwrap();
        let hash = hash_token(&token);
        assert_eq!(hash, hash_token(&token));
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains(&token));
        assert_ne!(hash, hash_token(&generate_token().unwrap()));
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; myledger_session=abc123; other=1"),
        );
        assert_eq!(read_session_cookie(&headers), Some("abc123".to_string()));
    }

    #[test]
    fn no_cookie_header_means_no_session() {
        assert_eq!(read_session_cookie(&HeaderMap::new()), None);

        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("theme=dark"));
        assert_eq!(read_session_cookie(&headers), None);
    }

    #[test]
    fn the_session_cookie_is_locked_down() {
        let cookie = session_cookie("abc", 3600, false);
        assert!(cookie.contains("HttpOnly"), "script must not be able to read it");
        assert!(cookie.contains("SameSite=Lax"), "guards against cross-site posts");
        assert!(!cookie.contains("Secure"), "off unless asked for, for plain-HTTP LANs");
        assert!(session_cookie("abc", 3600, true).contains("; Secure"));
        assert!(cleared_session_cookie(false).contains("Max-Age=0"));
    }
}
