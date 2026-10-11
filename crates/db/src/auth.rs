//! The single user and their paired devices.
//!
//! These records deliberately do **not** implement `Serialize`. A password hash
//! and a device-token hash have no business reaching a client, and the simplest
//! way to guarantee that is for them to have no way of being serialised at all.
//!
//! Hashing is the server's job: this module stores whatever string it is given
//! and compares by exact match. It never sees a plaintext password or a raw
//! device token.

use myledger_core::now_timestamp;
use uuid::Uuid;

use crate::convert::uuid_column;
use crate::error::DbError;
use crate::{Db, error};

/// The account that owns this ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub id: Uuid,
    pub username: String,
    /// An argon2id hash. Never a password.
    pub password_hash: String,
    pub created_at: String,
}

/// A signed-in browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Hash of the session token held in the user's cookie.
    pub token_hash: String,
    pub user_id: Uuid,
    pub created_at: String,
    pub last_seen_at: String,
    /// When this session stops being accepted, as an ISO-8601 timestamp.
    pub expires_at: String,
}

/// A paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTokenRecord {
    /// Hash of the pairing token. The token itself is shown once and never
    /// stored, so a copy of this table cannot be used to log in.
    pub token_hash: String,
    /// What the user called this device.
    pub label: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    password_hash: String,
    created_at: String,
}

impl UserRow {
    fn into_user(self) -> Result<UserRecord, DbError> {
        Ok(UserRecord {
            id: uuid_column("users", &self.id, &self.id)?,
            username: self.username,
            password_hash: self.password_hash,
            created_at: self.created_at,
        })
    }
}

/// How many users exist. Zero means the server still needs first-run setup.
pub async fn count_users(db: &Db) -> Result<i64, DbError> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(db.pool())
        .await?;
    Ok(count)
}

/// Creates the user. Usernames are unique, so a second one with the same name
/// fails rather than shadowing the first.
pub async fn insert_user(
    db: &Db,
    username: &str,
    password_hash: &str,
) -> Result<UserRecord, DbError> {
    let record = UserRecord {
        id: Uuid::new_v4(),
        username: username.trim().to_string(),
        password_hash: password_hash.to_string(),
        created_at: now_timestamp(),
    };

    sqlx::query("INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)")
        .bind(record.id.to_string())
        .bind(&record.username)
        .bind(&record.password_hash)
        .bind(&record.created_at)
        .execute(db.pool())
        .await
        .map_err(error::classify_insert_error)?;

    Ok(record)
}

/// Looks up a user by name, ignoring case.
pub async fn find_user_by_username(
    db: &Db,
    username: &str,
) -> Result<Option<UserRecord>, DbError> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, username, password_hash, created_at FROM users \
         WHERE username = ? COLLATE NOCASE",
    )
    .bind(username.trim())
    .fetch_optional(db.pool())
    .await?;
    row.map(UserRow::into_user).transpose()
}

/// Looks up a user by ID.
pub async fn find_user_by_id(db: &Db, id: Uuid) -> Result<Option<UserRecord>, DbError> {
    let row: Option<UserRow> =
        sqlx::query_as("SELECT id, username, password_hash, created_at FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(db.pool())
            .await?;
    row.map(UserRow::into_user).transpose()
}

/// The one user this ledger belongs to, if setup has been done.
///
/// MyLedger is single-user by design, and a device token is bound to the
/// ledger rather than to a person — so this is how a paired phone resolves
/// whose data it is looking at.
pub async fn find_only_user(db: &Db) -> Result<Option<UserRecord>, DbError> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, username, password_hash, created_at FROM users ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(db.pool())
    .await?;
    row.map(UserRow::into_user).transpose()
}

/// Replaces a user's password hash, returning whether the user existed.
pub async fn set_password_hash(db: &Db, id: Uuid, password_hash: &str) -> Result<bool, DbError> {
    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Records a newly paired device.
pub async fn insert_device_token(
    db: &Db,
    token_hash: &str,
    label: Option<&str>,
) -> Result<DeviceTokenRecord, DbError> {
    let record = DeviceTokenRecord {
        token_hash: token_hash.to_string(),
        label: label.map(str::to_string),
        created_at: now_timestamp(),
        last_used_at: None,
    };

    sqlx::query(
        "INSERT INTO device_tokens (token_hash, label, created_at, last_used_at) \
         VALUES (?, ?, ?, NULL)",
    )
    .bind(&record.token_hash)
    .bind(&record.label)
    .bind(&record.created_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;

    Ok(record)
}

/// Finds a paired device by the hash of its token.
pub async fn find_device_token(
    db: &Db,
    token_hash: &str,
) -> Result<Option<DeviceTokenRecord>, DbError> {
    let row: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT token_hash, label, created_at, last_used_at FROM device_tokens \
         WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|(token_hash, label, created_at, last_used_at)| DeviceTokenRecord {
        token_hash,
        label,
        created_at,
        last_used_at,
    }))
}

/// Notes that a device just authenticated.
pub async fn touch_device_token(db: &Db, token_hash: &str) -> Result<bool, DbError> {
    let result = sqlx::query("UPDATE device_tokens SET last_used_at = ? WHERE token_hash = ?")
        .bind(now_timestamp())
        .bind(token_hash)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Every paired device, newest first.
pub async fn list_device_tokens(db: &Db) -> Result<Vec<DeviceTokenRecord>, DbError> {
    let rows: Vec<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT token_hash, label, created_at, last_used_at FROM device_tokens \
         ORDER BY created_at DESC",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(token_hash, label, created_at, last_used_at)| DeviceTokenRecord {
            token_hash,
            label,
            created_at,
            last_used_at,
        })
        .collect())
}

/// Unpairs a device, returning whether it was paired.
pub async fn delete_device_token(db: &Db, token_hash: &str) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM device_tokens WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Starts a session for a signed-in browser.
pub async fn insert_session(
    db: &Db,
    token_hash: &str,
    user_id: Uuid,
    expires_at: &str,
) -> Result<SessionRecord, DbError> {
    let now = now_timestamp();
    let record = SessionRecord {
        token_hash: token_hash.to_string(),
        user_id,
        created_at: now.clone(),
        last_seen_at: now,
        expires_at: expires_at.to_string(),
    };

    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&record.token_hash)
    .bind(record.user_id.to_string())
    .bind(&record.created_at)
    .bind(&record.last_seen_at)
    .bind(&record.expires_at)
    .execute(db.pool())
    .await
    .map_err(error::classify_insert_error)?;

    Ok(record)
}

/// Looks a session up by the hash of its token.
///
/// Expired sessions are *not* filtered out here — the caller decides what
/// counts as expired, and a caller that treats "expired" the same as "absent"
/// would have no way to clean the row up.
pub async fn find_session(db: &Db, token_hash: &str) -> Result<Option<SessionRecord>, DbError> {
    let row: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT token_hash, user_id, created_at, last_seen_at, expires_at FROM sessions \
         WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(db.pool())
    .await?;

    let Some((token_hash, user_id, created_at, last_seen_at, expires_at)) = row else {
        return Ok(None);
    };
    Ok(Some(SessionRecord {
        user_id: uuid_column("sessions", &token_hash, &user_id)?,
        token_hash,
        created_at,
        last_seen_at,
        expires_at,
    }))
}

/// Notes activity on a session and pushes its expiry out.
///
/// Sessions slide rather than expiring at a fixed time, so a browser in daily
/// use is never signed out mid-task.
pub async fn touch_session(db: &Db, token_hash: &str, expires_at: &str) -> Result<bool, DbError> {
    let result =
        sqlx::query("UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE token_hash = ?")
            .bind(now_timestamp())
            .bind(expires_at)
            .bind(token_hash)
            .execute(db.pool())
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Ends one session, returning whether it existed.
pub async fn delete_session(db: &Db, token_hash: &str) -> Result<bool, DbError> {
    let result = sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Ends every session belonging to a user — for a password change.
pub async fn delete_sessions_for_user(db: &Db, user_id: Uuid) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected())
}

/// Clears out sessions that have expired, returning how many went.
///
/// Run at startup: nothing else prunes them, and an expired row is dead weight
/// that still names a valid user.
pub async fn delete_expired_sessions(db: &Db, now: &str) -> Result<u64, DbError> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected())
}
