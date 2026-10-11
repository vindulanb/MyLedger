-- Browser sessions.
--
-- Kept in the database rather than in process memory so that restarting the
-- server doesn't sign the user out, and so that signing out on one device can
-- actually revoke the session rather than merely forgetting it locally.
--
-- Only the hash of a session token is stored, exactly as for device_tokens: a
-- copy of this table is not enough to impersonate anyone.
CREATE TABLE IF NOT EXISTS sessions (
    token_hash   TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at   TEXT NOT NULL
);

-- Expiry is compared as text, which works because timestamps are fixed-width
-- UTC and therefore sort chronologically.
CREATE INDEX IF NOT EXISTS sessions_expires_at_idx ON sessions (expires_at);
CREATE INDEX IF NOT EXISTS sessions_user_idx ON sessions (user_id);
