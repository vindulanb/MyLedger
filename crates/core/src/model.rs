//! The shared domain records.
//!
//! These structs are the single definition of a MyLedger record: the database
//! layer reads and writes them, the API serialises them, and both apps consume
//! them. Conventions that hold throughout:
//!
//! - Money is an [`i64`] count of cents. Never a float, and never formatted
//!   until render time.
//! - Identifiers are UUID v4.
//! - Timestamps are ISO-8601 strings in UTC, as produced by [`now_timestamp`].
//!   Storing them as fixed-width strings keeps them sortable as SQLite `TEXT`.
//! - Dates the user sees (a transaction's date, a goal's due date) are plain
//!   `YYYY-MM-DD` strings with no time or zone attached.
//! - A record whose table supports soft deletion carries a nullable
//!   `deleted_at`; a non-`None` value means the row is gone as far as the app
//!   is concerned.

use std::time::Duration;

use chrono::{SecondsFormat, TimeDelta as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Returned when a database or wire token doesn't name a variant of the
/// enum it was being read into.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{value:?} is not a valid {expected}")]
pub struct ParseEnumError {
    /// Human-readable name of the enum that was expected, e.g. `"transaction kind"`.
    pub expected: &'static str,
    /// The token that failed to parse.
    pub value: String,
}

/// Defines an enum whose values travel as short lowercase tokens.
///
/// One token per variant serves every boundary: the database `CHECK`
/// constraints, the JSON API, and anything a human types. serde is implemented
/// in terms of `as_str` and `FromStr` rather than derived, so the three can't
/// drift apart — there is only one list of spellings.
///
/// Reading is deliberately forgiving of case and padding, since the same path
/// parses values typed by a person; writing always emits the canonical token.
macro_rules! token_enum {
    (
        $(#[$enum_meta:meta])*
        $name:ident as $label:literal {
            $($(#[$variant_meta:meta])* $variant:ident => $token:literal),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            /// The lowercase token used in the database and on the wire.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $token),+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let raw = <std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                raw.parse().map_err(::serde::de::Error::custom)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::model::ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($token => Ok(Self::$variant),)+
                    _ => Err($crate::model::ParseEnumError {
                        expected: $label,
                        value: s.to_string(),
                    }),
                }
            }
        }
    };
}

pub(crate) use token_enum;

token_enum! {
    /// Which way money moved. The amount itself is always stored positive; this
    /// is what gives it a direction.
    TransactionKind as "transaction kind" {
        /// Money leaving an account.
        Expense => "expense",
        /// Money arriving in an account.
        Income => "income",
    }
}

token_enum! {
    /// How a transaction got into the ledger.
    TransactionSource as "transaction source" {
        /// Typed in by the user.
        Manual => "manual",
        /// Imported from a bank CSV export.
        Csv => "csv",
        /// Extracted from an uploaded receipt or statement.
        Document => "document",
        /// Parsed out of a bank SMS alert.
        Sms => "sms",
    }
}

token_enum! {
    /// Whether a confirmed repeating charge is a bill or a subscription. The
    /// distinction drives how it is presented, not how it is calculated.
    RecurringKind as "recurring kind" {
        /// A recurring bill: rent, insurance, a loan payment.
        Recurring => "recurring",
        /// A subscription: a streaming service, a gym membership.
        Subscription => "subscription",
    }
}

token_enum! {
    /// What an SMS parsing rule expects the matched message to describe, when
    /// the message text itself doesn't make the direction obvious.
    SmsKindHint as "SMS kind hint" {
        /// Money leaving the account.
        Debit => "debit",
        /// Money arriving in the account.
        Credit => "credit",
        /// Work it out from the message.
        Auto => "auto",
    }
}

token_enum! {
    /// Where an uploaded document has got to.
    DocumentStatus as "document status" {
        /// Stored, not yet looked at.
        Queued => "queued",
        /// Stored and processed.
        Stored => "stored",
        /// Processed, but something needs the user's eyes.
        Review => "review",
    }
}

token_enum! {
    /// The span of time the app is currently showing.
    Period as "period" {
        /// Everything ever recorded.
        AllTime => "all-time",
        ThisMonth => "this-month",
        LastMonth => "last-month",
        LastThreeMonths => "last-3-months",
        LastSixMonths => "last-6-months",
        ThisYear => "this-year",
    }
}

token_enum! {
    /// How often a repeating charge lands.
    Cadence as "cadence" {
        /// Roughly every 7 days.
        Weekly => "weekly",
        /// Roughly every 14 days.
        Biweekly => "biweekly",
        /// Roughly every month.
        Monthly => "monthly",
        /// Roughly every 3 months.
        Quarterly => "quarterly",
        /// Roughly every 12 months.
        Annual => "annual",
    }
}

/// A place money sits: a bank account, a credit card, a cash wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A spending or income category. Every transaction has at most one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A free-form label. A transaction may carry any number of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: Uuid,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A single movement of money.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Uuid,
    /// Calendar date the money moved, as `YYYY-MM-DD`.
    pub date: String,
    /// Merchant as the user sees it, in its original casing.
    pub merchant: String,
    pub category_id: Option<Uuid>,
    /// Always positive; [`kind`](Self::kind) carries the direction.
    pub amount_cents: i64,
    pub kind: TransactionKind,
    pub account_id: Option<Uuid>,
    /// IDs of the tags attached to this transaction.
    pub tags: Vec<Uuid>,
    /// Whether a receipt or statement is attached.
    pub receipt: bool,
    pub source: TransactionSource,
    /// Duplicate-detection key from
    /// [`transaction_fingerprint`](crate::transaction_fingerprint). Unique
    /// across the ledger, enforced by the database.
    pub fingerprint: String,
    /// Hash of the SMS this came from, if it came from one. Lets the phone
    /// re-scan its inbox without re-importing messages it has already read.
    pub sms_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A categorisation rule: when an incoming merchant matches `when_text`, apply
/// the given category and/or tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: Uuid,
    /// Text matched against the merchant, case-insensitively.
    pub when_text: String,
    pub then_category_id: Option<Uuid>,
    pub then_tag_id: Option<Uuid>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// A monthly spending ceiling for one category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub id: Uuid,
    pub category_id: Option<Uuid>,
    pub monthly_limit_cents: i64,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// An amount the user is saving towards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub id: Uuid,
    pub name: String,
    pub target_cents: i64,
    pub current_cents: i64,
    /// Optional target date, as `YYYY-MM-DD`.
    pub due_date: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A repeating charge the user has confirmed.
///
/// This is distinct from a *suggestion*: detection proposes patterns, and only
/// the user's confirmation turns one into a row of this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurringItem {
    pub id: Uuid,
    pub name: String,
    pub category_id: Option<Uuid>,
    pub amount_cents: i64,
    pub cadence: Cadence,
    /// When the next charge is expected, as `YYYY-MM-DD`.
    pub next_date: Option<String>,
    pub account_id: Option<Uuid>,
    pub kind: RecurringKind,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// A rule for turning a bank's SMS alert into a transaction.
///
/// Only the rule itself lives here. The engine that matches messages against
/// these patterns is mobile-specific and built separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmsParseRule {
    pub id: Uuid,
    /// What the user calls this rule.
    pub label: String,
    /// Pattern the sender must match, or `None` to accept any sender.
    pub sender_pattern: Option<String>,
    /// Regex run against the message body.
    pub body_regex: String,
    /// Which capture group of `body_regex` holds the amount.
    pub amount_group: Option<i64>,
    /// Which capture group holds the merchant.
    pub merchant_group: Option<i64>,
    /// Which capture group holds the date.
    pub date_group: Option<i64>,
    pub kind_hint: Option<SmsKindHint>,
    /// Name of the account these messages belong to.
    pub account_hint: Option<String>,
    /// Category to apply when the rule matches and nothing better is known.
    pub default_category_id: Option<Uuid>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Metadata for a file the user uploaded — a receipt, a statement.
///
/// The bytes live on disk; this is only the record of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: Uuid,
    /// The name the file arrived with.
    pub filename: String,
    pub mime_type: Option<String>,
    /// Size in bytes.
    pub size: Option<i64>,
    /// Where the bytes are on disk.
    ///
    /// Never serialised. A client asks for a document by ID and the server
    /// streams it; the layout of the data directory is nobody else's business.
    #[serde(skip_serializing, default)]
    pub storage_path: String,
    pub status: DocumentStatus,
    /// How the file arrived, e.g. an upload or a share from the phone.
    pub source: Option<String>,
    /// The transaction this document backs up, once one is known.
    pub linked_transaction_id: Option<Uuid>,
    pub created_at: String,
}

/// The app's settings, decoded from the key/value table they are stored in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub selected_period: Period,
    /// Whether the user has told us their assets and liabilities. Until they
    /// have, net worth is not shown at all rather than shown as zero.
    pub net_worth_configured: bool,
    pub assets_cents: i64,
    pub liabilities_cents: i64,
    /// When the data was last wiped, if it ever was. The mobile app uses this
    /// so an SMS import doesn't drag back everything the wipe removed.
    pub wipe_reset_at: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            selected_period: Period::AllTime,
            net_worth_configured: false,
            assets_cents: 0,
            liabilities_cents: 0,
            wipe_reset_at: None,
        }
    }
}

/// The current UTC time in the ISO-8601 form used for every `created_at`,
/// `updated_at` and `deleted_at` in the app.
///
/// Fixed width to the millisecond, so lexicographic ordering of the stored
/// `TEXT` matches chronological ordering.
pub fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// A timestamp `offset` into the future, in the same form as [`now_timestamp`].
///
/// Used for expiry times, which are compared as plain strings — so they have to
/// be written the same way as the timestamps they are compared against.
pub fn timestamp_after(offset: Duration) -> String {
    let offset = ChronoDuration::from_std(offset).unwrap_or(ChronoDuration::MAX);
    (Utc::now() + offset).to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn tokens_match_the_database_check_constraints() {
        assert_eq!(TransactionKind::Expense.as_str(), "expense");
        assert_eq!(TransactionKind::Income.as_str(), "income");
        assert_eq!(TransactionSource::Csv.as_str(), "csv");
        assert_eq!(TransactionSource::Sms.as_str(), "sms");
        assert_eq!(RecurringKind::Subscription.as_str(), "subscription");
        assert_eq!(Cadence::Biweekly.as_str(), "biweekly");
        assert_eq!(Cadence::Annual.as_str(), "annual");
    }

    #[test]
    fn serde_emits_the_same_tokens_as_as_str() {
        // The JSON representation and the database representation have to agree,
        // otherwise a value round-tripping through one and then the other drifts.
        let json = serde_json::to_string(&TransactionSource::Document).unwrap();
        assert_eq!(json, "\"document\"");
        assert_eq!(json, format!("\"{}\"", TransactionSource::Document.as_str()));

        let back: TransactionSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TransactionSource::Document);
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        for kind in [TransactionKind::Expense, TransactionKind::Income] {
            assert_eq!(TransactionKind::from_str(kind.as_str()), Ok(kind));
        }
        for source in [
            TransactionSource::Manual,
            TransactionSource::Csv,
            TransactionSource::Document,
            TransactionSource::Sms,
        ] {
            assert_eq!(TransactionSource::from_str(source.as_str()), Ok(source));
        }
        for cadence in [
            Cadence::Weekly,
            Cadence::Biweekly,
            Cadence::Monthly,
            Cadence::Quarterly,
            Cadence::Annual,
        ] {
            assert_eq!(Cadence::from_str(cadence.as_str()), Ok(cadence));
        }
    }

    #[test]
    fn multi_word_tokens_survive_the_round_trip() {
        // `Period` is the one enum whose tokens aren't a plain lowercasing of
        // the variant name, which is what a derived `rename_all` would give.
        assert_eq!(Period::LastThreeMonths.as_str(), "last-3-months");
        assert_eq!(
            serde_json::to_string(&Period::LastThreeMonths).unwrap(),
            "\"last-3-months\""
        );
        assert_eq!(Period::from_str("last-3-months"), Ok(Period::LastThreeMonths));
        assert_eq!(
            serde_json::from_str::<Period>("\"all-time\"").unwrap(),
            Period::AllTime
        );
    }

    #[test]
    fn deserialising_an_unknown_token_fails_with_the_parse_message() {
        let err = serde_json::from_str::<Period>("\"last-week\"").unwrap_err();
        assert!(
            err.to_string().contains("is not a valid period"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn from_str_tolerates_casing_and_padding() {
        assert_eq!(TransactionKind::from_str("  EXPENSE "), Ok(TransactionKind::Expense));
        assert_eq!(RecurringKind::from_str("Subscription"), Ok(RecurringKind::Subscription));
    }

    #[test]
    fn from_str_rejects_unknown_tokens_with_a_readable_error() {
        let err = TransactionKind::from_str("refund").unwrap_err();
        assert_eq!(err.expected, "transaction kind");
        assert_eq!(err.value, "refund");
        assert_eq!(err.to_string(), "\"refund\" is not a valid transaction kind");
    }

    #[test]
    fn timestamps_are_fixed_width_utc_and_sort_chronologically() {
        let earlier = now_timestamp();
        let later = now_timestamp();

        assert_eq!(earlier.len(), 24, "expected `YYYY-MM-DDTHH:MM:SS.mmmZ`, got {earlier}");
        assert!(earlier.ends_with('Z'));
        // Equal is fine — two calls can land in the same millisecond — but time
        // must never appear to run backwards under string comparison.
        assert!(earlier <= later);
    }
}
