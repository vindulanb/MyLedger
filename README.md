# MyLedger

A private, single-user personal finance app. Everything runs on infrastructure
you own — no cloud vendor, no SaaS backend. The whole stack is Rust.

MyLedger tracks accounts, transactions, categories, tags, budgets, goals and
recurring bills/subscriptions, with transactions arriving by manual entry, CSV
import, uploaded documents or parsed SMS messages.

## Design rules

- Money is always an `i64` count of **cents**, never a float. It becomes a
  formatted string only at render time.
- Every record has a UUID v4 `id` plus `created_at` / `updated_at` ISO-8601
  timestamps, and soft-deletes through a nullable `deleted_at`.
- Every dataset starts empty. No sample or demo financial data is ever
  inserted — the only seeded rows are starter category and account *names*.
- All input is validated server-side, and all SQL goes through parameterized
  `sqlx` queries.

## Workspace layout

| Path             | Kind | What lives there                                                     |
| ---------------- | ---- | -------------------------------------------------------------------- |
| `crates/core`    | lib  | Shared domain types, validation, fingerprinting, recurring detection  |
| `crates/db`      | lib  | SQLite schema, migrations, seed routine and the query layer           |
| `crates/server`  | bin  | Axum JSON API (`myledger-server`), authentication, document storage   |
| `apps/mobile`    | —    | Mobile client — built later by Master Prompt B                        |
| `apps/web`       | app  | Dioxus web client, compiled to WASM and served by `crates/server`     |

The `core` and `db` packages expose library targets named `myledger_core` and
`myledger_db`, so a package called `core` cannot shadow the standard library's
`core` crate inside dependents.

## Building

```sh
cargo build --workspace
cargo test  --workspace
```

## Running

The server is single-user and needs that user created once, before it will
start. Either run setup explicitly:

```sh
cargo run -p server -- setup --username <name> --password <password>
cargo run -p server
```

…or set `MYLEDGER_SETUP_USERNAME` and `MYLEDGER_SETUP_PASSWORD` and start the
server, which completes setup on its first boot. Passwords are hashed with
argon2id. Starting with no user configured prints these instructions and exits.

A browser signs in at `POST /api/v1/auth/login` and gets a session cookie; a
phone is paired once at `POST /api/v1/auth/device-tokens` and carries a bearer
token thereafter. Both work on every endpoint. Sessions and pairings live in the
database, so restarting the server signs nobody out.

## Configuration

| Variable                 | Purpose                                                    |
| ------------------------ | ---------------------------------------------------------- |
| `MYLEDGER_DB_PATH`       | SQLite database file (default `data/myledger.sqlite`)       |
| `MYLEDGER_DATA_DIR`      | Directory for uploaded documents (default `data/documents`) |
| `MYLEDGER_WEB_DIR`       | Built web app (default: the `dx build --release` output)    |
| `MYLEDGER_BIND`          | Listen address (default `127.0.0.1:8787`)                   |
| `MYLEDGER_COOKIE_SECURE` | Mark session cookies `Secure`. Turn on when serving HTTPS.  |
| `MYLEDGER_LOG`           | Log filter (default `info`)                                 |

## The web app

Built, and served by this backend. From the repository root:

```sh
cd apps/web && dx build --release
```

The server finds the output on its own; `MYLEDGER_WEB_DIR` overrides it. See
[`apps/web/README.md`](apps/web/README.md) for why that crate is outside the
workspace and how it is laid out.

## Build order

The project is built in three phases.

1. **Master Prompt A — Shared Foundation**: workspace, domain logic, database,
   backend API. ✅ Done.
2. **Master Prompt B — Mobile app**, on top of the same API. Not started.
3. **Master Prompt C — Web app**, on top of the same API. ✅ Done.

[`HANDOFF.md`](HANDOFF.md) covers running it, exposing it beyond the house,
how duplicate and recurring detection actually work, what the document
pipeline can and cannot do, and how to pair a phone when you start Prompt B.
