//! Turning stored columns back into domain values.
//!
//! SQLite has no UUID type and no boolean, so IDs are `TEXT` and flags are
//! `INTEGER`. These helpers do the conversion in one place and, when a value
//! doesn't survive the trip, report *which row* of *which table* is at fault —
//! the only thing that makes such a failure actionable.

use std::str::FromStr;

use myledger_core::ParseEnumError;
use uuid::Uuid;

use crate::error::{DbError, malformed};

/// Reads a required UUID column.
pub(crate) fn uuid_column(table: &'static str, row_id: &str, raw: &str) -> Result<Uuid, DbError> {
    Uuid::parse_str(raw).map_err(|e| malformed(table, row_id, format_args!("bad uuid {raw:?}: {e}")))
}

/// Reads a nullable UUID column.
pub(crate) fn opt_uuid_column(
    table: &'static str,
    row_id: &str,
    raw: Option<&str>,
) -> Result<Option<Uuid>, DbError> {
    raw.map(|value| uuid_column(table, row_id, value)).transpose()
}

/// Reads a required token column into its enum.
pub(crate) fn enum_column<T>(table: &'static str, row_id: &str, raw: &str) -> Result<T, DbError>
where
    T: FromStr<Err = ParseEnumError>,
{
    raw.parse().map_err(|e| malformed(table, row_id, e))
}

/// Reads a nullable token column into its enum.
pub(crate) fn opt_enum_column<T>(
    table: &'static str,
    row_id: &str,
    raw: Option<&str>,
) -> Result<Option<T>, DbError>
where
    T: FromStr<Err = ParseEnumError>,
{
    raw.map(|value| enum_column(table, row_id, value)).transpose()
}

/// Reads an `INTEGER` flag. Anything non-zero is true, matching SQLite's own
/// notion of truthiness — the `CHECK (… IN (0, 1))` constraints keep it honest.
pub(crate) fn flag(stored: i64) -> bool {
    stored != 0
}

/// Writes a flag back as an `INTEGER`.
pub(crate) fn flag_value(value: bool) -> i64 {
    i64::from(value)
}

/// Renders an optional UUID for binding.
pub(crate) fn opt_uuid_text(id: Option<Uuid>) -> Option<String> {
    id.map(|id| id.to_string())
}
