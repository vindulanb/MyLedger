# MyLedger — Master Prompt A: Shared Foundation

Run this file **first**, before either the Mobile or the Web master prompt. It builds the parts both apps depend on: the Rust workspace, the shared domain logic, the database, and the backend API. Once this file's steps are done, run **Master Prompt B (Mobile)** and **Master Prompt C (Web)** in whichever order you like — each of those assumes this foundation already exists.

## How to use this file

Each `## Step N` section below is one self-contained prompt. Copy everything under **"Prompt:"** (down to, but not including, "Quick check:") and paste it as your next message to Claude Code. Let it finish, run the **Quick check** for that step yourself, and only then move to the next step. Don't skip ahead — later steps assume earlier ones are done.

Progress checklist:

- [ ] Step 1 — Workspace scaffold
- [ ] Step 2 — Core domain models & fingerprinting
- [ ] Step 3 — Recurring/subscription detection algorithm
- [ ] Step 4 — Database schema & migrations
- [ ] Step 5 — Backend skeleton & auth
- [ ] Step 6 — Core data API (state, transactions, preferences)
- [ ] Step 7 — Recurring/subscription suggestions API
- [ ] Step 8 — Documents API & SMS-rule storage API
- [ ] Step 9 — Data-wipe API
- [ ] Step 10 — Foundation verification pass

## Ground rules that apply to every step in this file

1. This is a personal single-user finance app called **MyLedger** (rename freely with a global find-and-replace if you'd rather call it something else).
2. Everything runs on infrastructure I own — no cloud vendor, no SaaS/BaaS. Language is Rust throughout.
3. Money is always an `i64` of integer **cents** — never a float — converted to a display string only at render time.
4. Every record gets a `uuid` v4 `id`, `created_at`/`updated_at` ISO-8601 timestamps, and soft delete via nullable `deleted_at` where the table describes one.
5. On first run, every dataset is empty. Never insert sample/demo financial data. The only exception is the starter *lookup* names in Step 4 (categories/accounts) — names only, no balances, no transactions attached, user-deletable.
6. Every step should be **idempotent**: if you re-run a step, or a later step finds something from an earlier step already exists, extend/reuse it — don't duplicate or fork it.
7. Validate everything server-side, use parameterized `sqlx` queries, never trust client input alone.
8. Do the actual work — write real, compiling, runnable code — not a plan or stubs.

---

## Step 1: Workspace scaffold

**Prompt:**

Set up a new Rust workspace for a personal finance app called MyLedger. Create the top-level `Cargo.toml` as a workspace, and these member crates with minimal placeholder content that compiles: `crates/core` (lib), `crates/db` (lib), `crates/server` (bin). Also create empty placeholder directories `apps/mobile` and `apps/web` (these will be filled in by later, separate master prompts — don't build anything into them now). Initialize git if it isn't already, add a sensible `.gitignore` for Rust (target/, etc.), and add a top-level `README.md` that briefly describes the project and lists the workspace layout. Confirm `cargo build --workspace` succeeds with no warnings before finishing.

**Quick check:**

- `cargo build --workspace` succeeds.
- The directory tree matches: `crates/core`, `crates/db`, `crates/server`, `apps/mobile`, `apps/web`.

---

## Step 2: Core domain models & fingerprinting

**Prompt:**

In the `crates/core` crate of the MyLedger workspace, define the shared domain types used by both the backend and (later) both apps: `Account`, `Category`, `Tag`, `Transaction` (fields: id, date as `YYYY-MM-DD` string, merchant, category_id, amount_cents: i64, kind: enum `Expense`/`Income`, account_id, tags: Vec<tag id>, receipt: bool, source: enum `Manual`/`Csv`/`Document`/`Sms`, fingerprint, created_at, updated_at), `Rule` (categorization rule: when_text, then_category_id, then_tag_id, enabled), `Budget`, `Goal`, `RecurringItem` (kind: `Recurring`/`Subscription`). Use `serde::Serialize`/`Deserialize` on everything and `uuid::Uuid` for IDs.

Implement a pure function that computes the duplicate-detection fingerprint for a transaction:

```
fingerprint = date + "|" + merchant.trim().to_lowercase() + "|" + amount_cents.to_string() + "|" + account_name.trim().to_lowercase()
```

Implement validation functions: a transaction needs a non-empty merchant, a valid `YYYY-MM-DD` date, a positive finite `amount_cents`, and a valid `kind`. Write unit tests for the fingerprint function (including case-insensitivity and whitespace-trimming behavior) and for the validation functions (valid input passes, each invalid case is rejected with a clear error). Run `cargo test -p core` and make sure everything passes before finishing.

**Quick check:**

- `cargo test -p core` passes.
- Skim the fingerprint tests yourself — confirm `"Starbucks "` and `"starbucks"` produce the same fingerprint.

---

## Step 3: Recurring/subscription detection algorithm

**Prompt:**

Still in `crates/core`, implement the recurring/subscription detection algorithm as a set of pure, unit-tested functions operating on a slice of `Transaction`:

1. **Merchant normalization**: lowercase, trim, strip punctuation, strip a trailing `#<digits>`, collapse whitespace, strip long reference-number digit runs. Keep the original display merchant separately — normalization is only for grouping.
2. **Grouping & cadence**: group expense transactions by normalized merchant; require at least 2 distinct dates. Compute consecutive-day intervals and classify the dominant interval into one of: weekly (5–9 days), biweekly (12–17), monthly (24–40), quarterly (75–110), annual (330–400). Reject any candidate whose dominant interval doesn't fit one of those windows.
3. **Amount stability**: reject a subscription candidate if average amount variation exceeds 20%; reject any other recurring candidate above 35%.
4. **Subscription hints**: treat a candidate as a subscription if its category/tag includes "subscription" or its normalized merchant contains one of: `netflix, spotify, hulu, disney, youtube, icloud, dropbox, adobe, microsoft, amazon prime, patreon, membership, studio, gym, openai, chatgpt, canva, notion, zoom, slack, github`.
5. **Recurring-bill hints**: `mortgage, rent, loan, insurance, utility, utilities, electric, water, internet, phone, mobile, daycare, tuition, lease, car payment, auto payment, hoa, property tax`.
6. **No-hint merchants**: only suggest if there are at least 3 stable monthly/quarterly/annual occurrences with ≤3% amount variation — never suggest routine weekly grocery/shopping spend just because it repeats.
7. **Confidence**: High = ≥3 occurrences, ≤12% amount variation, ≤5 days interval jitter; otherwise Likely.
8. **Next-date calculation**: calendar-aware, preserving day-of-month behavior where possible.
9. **Monthly-equivalent conversion**: weekly `amount * 52 / 12`, biweekly `amount * 26 / 12`, monthly `amount`, quarterly `amount / 3`, annual `amount / 12`.

The output of the top-level function should be a list of "suggestion" structs (merchant, category, cadence, occurrence count, confidence, average charge, estimated monthly equivalent, next expected date) — it must never mutate data or silently create a confirmed recurring item; that decision belongs to the user later, in the UI.

Write unit tests covering: a clean monthly subscription pattern, a candidate rejected for inconsistent intervals, a candidate rejected for amount variation, a no-hint merchant correctly requiring 3+ occurrences, and the monthly-equivalent math for each cadence. Run `cargo test -p core` and confirm everything passes.

**Quick check:**

- `cargo test -p core` passes, including the new recurring-detection tests.
- Read through 2–3 of the test cases yourself and confirm the expected output matches the spec above (e.g. a weekly grocery run with no hint should NOT be suggested).

---

## Step 4: Database schema & migrations

**Prompt:**

In the `crates/db` crate, set up `sqlx` (SQLite driver) with a `migrations/` directory and idempotent, versioned migrations creating these tables (all IDs are `TEXT PRIMARY KEY` UUIDs; timestamps are `TEXT` ISO-8601; soft delete via nullable `deleted_at TEXT` where noted):

- `users(id, username, password_hash, created_at)`
- `device_tokens(token_hash, label, created_at, last_used_at)`
- `accounts(id, name, created_at, updated_at, deleted_at)`
- `categories(id, name, created_at, updated_at, deleted_at)`
- `tags(id, name, created_at, updated_at, deleted_at)`
- `transactions(id, date, merchant, category_id, amount_cents INTEGER NOT NULL, kind TEXT CHECK(kind IN ('expense','income')), account_id, receipt INTEGER NOT NULL DEFAULT 0, source TEXT CHECK(source IN ('manual','csv','document','sms')), fingerprint TEXT NOT NULL UNIQUE, sms_hash TEXT, created_at, updated_at, deleted_at)`
- `transaction_tags(transaction_id, tag_id)`
- `rules(id, when_text, then_category_id, then_tag_id, enabled INTEGER NOT NULL DEFAULT 1, created_at, updated_at)`
- `sms_parse_rules(id, label, sender_pattern, body_regex, amount_group, merchant_group, date_group, kind_hint TEXT CHECK(kind_hint IN ('debit','credit','auto')), account_hint, default_category_id, enabled INTEGER NOT NULL DEFAULT 1, created_at, updated_at)`
- `budgets(id, category_id, monthly_limit_cents INTEGER NOT NULL, active INTEGER NOT NULL DEFAULT 1, created_at, updated_at, deleted_at)`
- `goals(id, name, target_cents INTEGER NOT NULL, current_cents INTEGER NOT NULL DEFAULT 0, due_date, note, created_at, updated_at, deleted_at)`
- `recurring_items(id, name, category_id, amount_cents INTEGER NOT NULL, cadence, next_date, account_id, kind TEXT CHECK(kind IN ('recurring','subscription')), active INTEGER NOT NULL DEFAULT 1, created_at, updated_at, deleted_at)`
- `dismissed_patterns(pattern_key TEXT PRIMARY KEY, created_at)`
- `documents(id, filename, mime_type, size INTEGER, storage_path NOT NULL, status TEXT CHECK(status IN ('queued','stored','review')), source, linked_transaction_id, created_at)`
- `settings(key TEXT PRIMARY KEY, value NOT NULL, updated_at)`

Enforce the `transactions.fingerprint` `UNIQUE` constraint at the database level (not just in application code). Write a query layer (functions, not raw SQL scattered around callers) for basic CRUD on each table using `core`'s types, using compile-time-checked `sqlx::query!`/`query_as!` where practical.

Add a one-time seed routine (idempotent — only runs if the categories/accounts tables are empty) that inserts these **starter lookup names only, no balances**: categories `Housing, Groceries, Shopping, Dining, Transportation, Utilities, Subscriptions, Insurance, Health, Entertainment, Income, Needs review, Other`; accounts `Main Account, Credit Card, Cash`. Also seed default `settings` rows: `selected_period = "all-time"`, `net_worth_configured = false`, `assets_cents = 0`, `liabilities_cents = 0`.

Write a small integration test that runs the migrations against a temporary SQLite file, inserts a transaction, and confirms inserting a second transaction with an identical fingerprint fails with a uniqueness violation. Run it and confirm it passes.

**Quick check:**

- The migration runs cleanly against a fresh SQLite file.
- Your duplicate-fingerprint integration test passes.
- Manually inspect the seeded categories/accounts — confirm no transactions or balances were created alongside them.

---

## Step 5: Backend skeleton & auth

**Prompt:**

In `crates/server`, build an Axum + tokio binary that serves the MyLedger API. Wire it to the `crates/db` SQLite database (path from an env var, e.g. `MYLEDGER_DB_PATH`, running migrations automatically on startup — idempotently).

Implement dual authentication so the same endpoints work for both a browser session and a paired mobile device:

- If no `users` row exists yet, print instructions on first startup for a one-time setup: accept a username and password via env vars or a `myledger-server setup` CLI subcommand, hash the password with argon2id, and store the user row.
- `POST /api/v1/auth/login` — verify username/password, issue a server-side session cookie (e.g. via `tower-sessions` or an equivalent, backed by a signed cookie or a sessions table — your choice, just make it real, not an in-memory-only hack that loses sessions on restart).
- `POST /api/v1/auth/logout`.
- `POST /api/v1/auth/device-tokens` (requires an existing valid session) — generates a new random long device pairing token, stores only its hash in `device_tokens`, and returns the raw token exactly once in the response (never retrievable again).
- A middleware/extractor that accepts *either* a valid session cookie *or* a valid `Bearer <device-token>` header (hashed and checked against `device_tokens`, updating `last_used_at` on success) and rejects anything else with 401.

Protect every `/api/v1/*` route except `/api/v1/auth/login` behind that middleware. Return clear structured JSON errors (status code + message) rather than panicking. Write an integration test that logs in, generates a device token, and confirms a protected route works with both the session cookie and the bearer token, and rejects requests with neither.

**Quick check:**

- `cargo run -p server` starts cleanly against a fresh database and completes first-run setup.
- Your auth integration test passes.
- Manually confirm (`curl`) that a request with no credentials to a protected route returns 401.

---

## Step 6: Core data API (state, transactions, preferences)

**Prompt:**

In `crates/server`, implement these protected JSON endpoints, backed by the `crates/db` query layer from Step 4 and validated with `crates/core`'s validation functions from Step 2:

- `GET /api/v1/state` — returns transactions (newest first, capped at 5,000), all categories, accounts, tags, rules, sms_parse_rules, budgets, goals, recurring_items, decoded settings, and up to 100 recent document metadata rows. Never include file bytes, password hashes, or token secrets.
- `POST /api/v1/transactions` — accepts one transaction or a batch. Validate merchant/date/amount/kind per Step 2's rules. Compute the fingerprint and rely on the database's unique constraint (plus a pre-check) to reject duplicates. Apply any enabled `rules` (categorization) only *after* duplicate detection — a rule must never cause a duplicate to be inserted. Return clear inserted/duplicate counts for a batch.
- `PATCH /api/v1/transactions/:id` — updates category and/or tags only, returns the saved row.
- `DELETE /api/v1/transactions/:id`.
- `PUT /api/v1/preferences` — accepts and validates/normalizes any subset of: categories, accounts, tags, rules, budgets, goals, recurring_items, dismissed_patterns, net worth totals (`assets_cents`/`liabilities_cents`/`net_worth_configured`), and `selected_period` (one of `all-time`, `this-month`, `last-month`, `last-3-months`, `last-6-months`, `this-year`). Updating one group must never reset or clear an unrelated group.

Write integration tests: inserting a transaction and reading it back via `/state`; inserting the same transaction twice and confirming the second is reported as a duplicate, not a second row; a `PATCH` that changes only the category and confirms tags are untouched; a `PUT /preferences` that changes `selected_period` and confirms categories are unaffected.

**Quick check:**

- All new integration tests pass.
- `curl` a `POST /api/v1/transactions` twice with identical data and confirm the second call reports a duplicate, and `GET /api/v1/state` shows only one row.

---

## Step 7: Recurring/subscription suggestions API

**Prompt:**

In `crates/server`, wire Step 3's recurring-detection algorithm (from `crates/core`) up to the database:

- `GET /api/v1/recurring/suggestions` — loads recent expense transactions, runs the detection algorithm, filters out any pattern whose key is present in `dismissed_patterns`, and returns the resulting suggestions (merchant, category, cadence, occurrence count, confidence, average charge, monthly-equivalent, next expected date).
- `POST /api/v1/recurring/:pattern_key/keep` — accepts the suggestion's details and inserts a confirmed `recurring_items` row (with the correct `kind`, `recurring` or `subscription`, based on which hint list matched).
- `POST /api/v1/recurring/:pattern_key/ignore` — inserts a `dismissed_patterns` row so the suggestion stays hidden.
- Also add basic CRUD for confirmed items directly: `POST/PATCH/DELETE /api/v1/recurring-items/:id` for manual add/edit/delete (not from a suggestion).

Add an integration test: seed a set of transactions that form a clean monthly pattern, confirm the suggestion appears; call `ignore`, confirm it disappears from the list; add a fresh clean pattern, call `keep`, confirm it now appears in a `GET` of confirmed recurring items instead of the suggestions list.

**Quick check:**

- The integration test passes.
- Confirm Ignore and Keep are genuinely mutually exclusive — a kept pattern shouldn't still show up as a suggestion, and an ignored one shouldn't reappear on the next `GET /recurring/suggestions` call.

---

## Step 8: Documents API & SMS-rule storage API

**Prompt:**

In `crates/server`, implement:

1. **Documents**: `POST /api/v1/documents` — multipart upload, one or more files, reject anything over 20MB with a readable error, write original bytes to local disk under a directory from `MYLEDGER_DATA_DIR` (UUID-based filenames), write a `documents` row with `status = 'queued'`. `GET /api/v1/documents/:id/download` — protected, streams the file back (never expose the raw on-disk path to the client). Keep extraction/OCR logic **out of scope for this step** — that's built later by whichever app-specific prompt needs it; this step only needs reliable storage + metadata + retrieval.
2. **SMS parsing rules** (storage only — the actual regex-matching engine that *uses* these rules against SMS text is mobile-specific and built in a later, separate master prompt): `GET/POST/PATCH/DELETE /api/v1/sms-rules` — CRUD for rows shaped like `sms_parse_rules` in Step 4 (label, sender_pattern, body_regex, amount_group, merchant_group, date_group, kind_hint, account_hint, default_category_id, enabled). No rules are pre-seeded.

Write integration tests: upload a small test file under 20MB and confirm it's retrievable and metadata appears in `/state`; upload a file over 20MB and confirm it's rejected; create/update/delete an SMS parsing rule via the CRUD endpoints.

**Quick check:**

- Both sets of integration tests pass.
- Confirm the 20MB-plus upload is actually rejected, not silently truncated or accepted.
- Confirm no SMS parsing rules exist after a fresh migration (empty list from `GET /api/v1/sms-rules`).

---

## Step 9: Data-wipe API

**Prompt:**

In `crates/server`, implement `DELETE /api/v1/state`. It must require an exact JSON body `{"confirm": "DELETE ALL MYLEDGER DATA"}` — reject anything else with a 400 and no data change. On a valid request:

1. Delete all rows from `transactions`, `transaction_tags`, `documents` (also deleting the underlying files from disk), `rules`, `sms_parse_rules`, `budgets`, `goals`, `recurring_items`, `dismissed_patterns`, `tags`.
2. Re-run the Step 4 starter-lookup seed for categories/accounts (names only, no balances) so the app returns to the same empty-start state as a fresh install.
3. Reset `settings`: `assets_cents = 0`, `liabilities_cents = 0`, `net_worth_configured = false`, `selected_period = "all-time"`.
4. Save a fresh `wipe_reset_at` timestamp into `settings` (this will matter later for the mobile app's SMS import, so it doesn't re-import anything from before the wipe).
5. Return a clear success payload with counts of what was deleted.

Do **not** delete `users` or `device_tokens` — wiping financial data shouldn't sign you out or unpair your phone.

Write an integration test: seed some transactions/budgets/goals/documents, call the wipe with the correct confirmation, confirm everything financial is gone and `GET /api/v1/state` matches the fresh-install shape; also confirm calling it with a wrong or missing `confirm` value changes nothing.

**Quick check:**

- The integration test passes.
- Manually confirm a wrong confirmation string is rejected and no data is touched.
- Confirm your login/device pairing still works immediately after a wipe.

---

## Step 10: Foundation verification pass

**Prompt:**

Do a full pass over everything built in this foundation phase (Steps 1–9) of the MyLedger backend and confirm it's genuinely solid before I start building the mobile and web apps on top of it:

1. Run the entire test suite (`cargo test --workspace`) and confirm everything passes.
2. Start the server fresh against an empty database and confirm `GET /api/v1/state` returns the correct empty-start shape (empty transactions/budgets/goals/etc., `selected_period = "all-time"`, `net_worth_configured = false`), with only the starter category/account lookup names present.
3. Walk through the full auth flow manually: one-time setup, login, generate a device token, confirm a protected endpoint works with both credential types, confirm it's rejected with neither.
4. Walk through the full transaction lifecycle manually: create, confirm a duplicate is rejected, patch its category, delete it.
5. Walk through recurring suggestions manually: seed a clean pattern, confirm it's suggested, ignore it, confirm it disappears, confirm a fresh call to `/state` still shows the rest of the data intact.
6. Walk through the wipe flow manually and confirm it resets correctly without deleting the user/auth data.
7. Fix anything broken you find during this pass.
8. Give me a short status report: what's built, what's tested, any known rough edges, and confirmation that both `apps/mobile` and `apps/web` can now build on top of this without needing any backend changes for the core pieces described above.

**Quick check:**

- Read the status report. If it flags any rough edges, decide whether to have it fix them now or note them for later before moving on to Master Prompt B or C.
