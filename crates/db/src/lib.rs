//! SQLite persistence for MyLedger.
//!
//! Everything that touches the database goes through this crate. Callers get
//! functions that speak in [`myledger_core`] types; they never assemble SQL of
//! their own, and every value crossing the boundary is bound as a parameter.
//!
//! The layout mirrors the schema: one module per group of tables, each with
//! the same small vocabulary — list, get, insert, update, delete.
//!
//! Two rules the schema enforces rather than trusting callers with:
//!
//! - `transactions.fingerprint` is `UNIQUE` in the database, so a duplicate
//!   cannot be written even if application code forgets to check. The attempt
//!   surfaces as [`DbError::DuplicateFingerprint`].
//! - Rows are soft-deleted wherever the table has a `deleted_at`, so a delete
//!   is recoverable and history stays intact.

pub mod auth;
pub mod budgets;
pub mod dismissed;
pub mod documents;
pub mod goals;
pub mod lookups;
pub mod recurring_items;
pub mod rules;
pub mod seed;
pub mod settings;
pub mod sms_rules;
pub mod transactions;
pub mod wipe;

mod convert;
mod error;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

pub use error::DbError;

/// Environment variable holding the path to the SQLite database file.
pub const DB_PATH_ENV: &str = "MYLEDGER_DB_PATH";

/// Environment variable holding the directory where uploaded documents are
/// written to disk.
pub const DATA_DIR_ENV: &str = "MYLEDGER_DATA_DIR";

/// The schema migrations, compiled into the binary so a deployment is one file.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// An open MyLedger database.
///
/// Cheap to clone — the underlying pool is shared.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens the database at `path`, creating it if it isn't there, and brings
    /// the schema up to date.
    ///
    /// Safe to call on every startup: migrations already applied are skipped.
    /// Seeding the starter categories and accounts is deliberately *not* done
    /// here — see [`seed::seed_starter_data`] — so that tests and the data-wipe
    /// path can decide for themselves.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // Referential integrity is off by default in SQLite, and has to be
            // asked for per connection.
            .foreign_keys(true)
            // Write-ahead logging lets a read carry on while a write commits,
            // which matters once the web app and the phone both poll /state.
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        Self::from_pool(pool).await
    }

    /// Opens a private in-memory database, for tests.
    ///
    /// The pool is pinned to a single permanent connection: an in-memory SQLite
    /// database lives inside its connection and would vanish the moment the
    /// pool recycled it.
    pub async fn connect_in_memory() -> Result<Self, DbError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;

        Self::from_pool(pool).await
    }

    async fn from_pool(pool: SqlitePool) -> Result<Self, DbError> {
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// The connection pool, for the rare caller that needs its own query.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes the pool and waits for in-flight work to finish.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
