-- Document extraction: rules for reading a receipt, and what reading one
-- found.
--
-- The rules mirror `sms_parse_rules` column for column, because they are the
-- same idea applied to a different kind of text and both are matched by the
-- one engine in `core::textrules`. They are a separate table rather than a
-- flag on that one so that a rule written for a bank's SMS alerts is never
-- quietly applied to a receipt, or the other way round.

CREATE TABLE IF NOT EXISTS document_extraction_rules (
    id                  TEXT PRIMARY KEY,
    label               TEXT NOT NULL,
    -- Matched against the document's filename, or NULL for any document.
    filename_pattern    TEXT,
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

-- What pulling text out of one document produced.
--
-- Only the text is kept, never the fields a rule read from it. Rules change,
-- and a stored reading would go stale the moment one did; re-running the
-- current rules over the stored text is cheap and is always what the rules
-- say now.
--
-- `note` carries why there is no text, when there isn't — a scanned image
-- with no text layer, say. That belongs in the row rather than in a log,
-- because the user is owed an explanation on the screen.
CREATE TABLE IF NOT EXISTS document_extractions (
    document_id    TEXT PRIMARY KEY REFERENCES documents (id) ON DELETE CASCADE,
    extracted_text TEXT,
    note           TEXT,
    created_at     TEXT NOT NULL
);
