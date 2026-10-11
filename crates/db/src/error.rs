//! What can go wrong down here.

use std::fmt;

/// A failure from the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The `transactions.fingerprint` unique constraint rejected a write.
    ///
    /// Its own variant rather than a generic constraint error, because it isn't
    /// really a failure: it is the duplicate guard working, and the API turns it
    /// into a "already had this one" count rather than an error.
    #[error("a transaction with this fingerprint already exists")]
    DuplicateFingerprint,

    /// A stored row couldn't be read back into its domain type — a malformed
    /// UUID, or a token no enum recognises.
    #[error("{table} row {id:?} is stored in a form the app cannot read: {detail}")]
    Malformed {
        /// Table the row came from.
        table: &'static str,
        /// Primary key of the offending row.
        id: String,
        /// What specifically didn't parse.
        detail: String,
    },

    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("running migrations failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Builds a [`DbError::Malformed`] for a row that can't be decoded.
pub(crate) fn malformed(
    table: &'static str,
    id: impl Into<String>,
    detail: impl fmt::Display,
) -> DbError {
    DbError::Malformed {
        table,
        id: id.into(),
        detail: detail.to_string(),
    }
}

/// Recognises the fingerprint clash among general insert failures.
///
/// The check is narrowed to the fingerprint column: any *other* unique
/// violation is a real bug and should keep its original message.
pub(crate) fn classify_insert_error(err: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(ref database_error) = err
        && database_error.is_unique_violation()
        && database_error.message().contains("fingerprint")
    {
        return DbError::DuplicateFingerprint;
    }
    DbError::Sqlx(err)
}
