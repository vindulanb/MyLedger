-- MyLedger initial schema.
--
-- Conventions, applied throughout:
--   * IDs are TEXT holding a UUID v4.
--   * Timestamps are TEXT holding the fixed-width ISO-8601 form written by
--     core::now_timestamp, so lexicographic ordering is chronological ordering.
--   * User-facing dates are TEXT holding exactly YYYY-MM-DD.
--   * Money is INTEGER cents. Never a float.
--   * Booleans are INTEGER 0/1.
--   * A nullable deleted_at means the row supports soft deletion; non-NULL is
--     gone as far as the app is concerned.
--   * Every CHECK list matches the tokens a core::model enum emits.
--
-- Every statement is IF NOT EXISTS so that re-running is harmless, on top of
-- sqlx's own record of which migrations have run.

-- The single account that owns this ledger. Not wiped by a data reset.
CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at    TEXT NOT NULL
);

-- Paired devices. Only the hash of each token is kept; the token itself is
-- shown once, at creation, and is not recoverable afterwards.
CREATE TABLE IF NOT EXISTS device_tokens (
    token_hash   TEXT PRIMARY KEY,
    label        TEXT,
    created_at   TEXT NOT NULL,
    last_used_at TEXT
);

CREATE TABLE IF NOT EXISTS accounts (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS categories (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS tags (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS transactions (
    id           TEXT PRIMARY KEY,
    date         TEXT NOT NULL,
    merchant     TEXT NOT NULL,
    category_id  TEXT REFERENCES categories (id) ON DELETE SET NULL,
    amount_cents INTEGER NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('expense', 'income')),
    account_id   TEXT REFERENCES accounts (id) ON DELETE SET NULL,
    receipt      INTEGER NOT NULL DEFAULT 0 CHECK (receipt IN (0, 1)),
    source       TEXT NOT NULL CHECK (source IN ('manual', 'csv', 'document', 'sms')),
    -- The duplicate guard. Enforced here rather than in application code so a
    -- second import of the same statement cannot double-count, whatever path
    -- it arrives by.
    fingerprint  TEXT NOT NULL UNIQUE,
    -- Hash of the SMS a transaction came from, so re-reading the inbox on the
    -- phone doesn't re-import messages already seen.
    sms_hash     TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    deleted_at   TEXT
);

CREATE INDEX IF NOT EXISTS transactions_date_idx ON transactions (date DESC);
CREATE INDEX IF NOT EXISTS transactions_live_idx ON transactions (deleted_at);
CREATE INDEX IF NOT EXISTS transactions_category_idx ON transactions (category_id);
CREATE INDEX IF NOT EXISTS transactions_sms_hash_idx ON transactions (sms_hash);

CREATE TABLE IF NOT EXISTS transaction_tags (
    transaction_id TEXT NOT NULL REFERENCES transactions (id) ON DELETE CASCADE,
    tag_id         TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (transaction_id, tag_id)
);

CREATE INDEX IF NOT EXISTS transaction_tags_tag_idx ON transaction_tags (tag_id);

-- Categorisation rules: when the merchant contains when_text, apply these.
CREATE TABLE IF NOT EXISTS rules (
    id               TEXT PRIMARY KEY,
    when_text        TEXT NOT NULL,
    then_category_id TEXT REFERENCES categories (id) ON DELETE SET NULL,
    then_tag_id      TEXT REFERENCES tags (id) ON DELETE SET NULL,
    enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

-- Storage for SMS parsing rules. The engine that applies them is mobile-side.
CREATE TABLE IF NOT EXISTS sms_parse_rules (
    id                  TEXT PRIMARY KEY,
    label               TEXT NOT NULL,
    sender_pattern      TEXT,
    body_regex          TEXT NOT NULL,
    amount_group        INTEGER,
    merchant_group      INTEGER,
    date_group          INTEGER,
    kind_hint           TEXT CHECK (kind_hint IN ('debit', 'credit', 'auto')),
    account_hint        TEXT,
    default_category_id TEXT REFERENCES categories (id) ON DELETE SET NULL,
    enabled             INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS budgets (
    id                  TEXT PRIMARY KEY,
    category_id         TEXT REFERENCES categories (id) ON DELETE SET NULL,
    monthly_limit_cents INTEGER NOT NULL,
    active              INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    deleted_at          TEXT
);

CREATE TABLE IF NOT EXISTS goals (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    target_cents  INTEGER NOT NULL,
    current_cents INTEGER NOT NULL DEFAULT 0,
    due_date      TEXT,
    note          TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    deleted_at    TEXT
);

-- Repeating charges the user has confirmed. Suggestions are computed on the
-- fly and never stored; only an accepted one lands here.
CREATE TABLE IF NOT EXISTS recurring_items (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    category_id  TEXT REFERENCES categories (id) ON DELETE SET NULL,
    amount_cents INTEGER NOT NULL,
    cadence      TEXT NOT NULL
        CHECK (cadence IN ('weekly', 'biweekly', 'monthly', 'quarterly', 'annual')),
    next_date    TEXT,
    account_id   TEXT REFERENCES accounts (id) ON DELETE SET NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('recurring', 'subscription')),
    active       INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    deleted_at   TEXT
);

-- Suggestions the user has waved away, keyed by normalised merchant.
CREATE TABLE IF NOT EXISTS dismissed_patterns (
    pattern_key TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents (
    id                    TEXT PRIMARY KEY,
    filename              TEXT NOT NULL,
    mime_type             TEXT,
    size                  INTEGER,
    storage_path          TEXT NOT NULL,
    status                TEXT NOT NULL CHECK (status IN ('queued', 'stored', 'review')),
    source                TEXT,
    linked_transaction_id TEXT REFERENCES transactions (id) ON DELETE SET NULL,
    created_at            TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS documents_created_at_idx ON documents (created_at DESC);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
