//! Server-side validation.
//!
//! Nothing that arrives from a client is trusted: not the web app's forms, not
//! a CSV import, not a parsed SMS. Every path into the ledger runs through
//! these functions first, and they are the only definition of what makes a
//! transaction acceptable.

use chrono::NaiveDate;
use std::str::FromStr;

use crate::model::{ParseEnumError, Transaction, TransactionKind};

/// Why a value was rejected. Each variant carries the offending input so the
/// API can tell the user what to fix rather than just that something is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// The merchant was empty, or nothing but whitespace.
    #[error("merchant must not be empty")]
    EmptyMerchant,

    /// The date wasn't a real calendar date written as `YYYY-MM-DD`.
    #[error("date must be a real calendar date in YYYY-MM-DD form, got {got:?}")]
    InvalidDate {
        /// The rejected input.
        got: String,
    },

    /// The amount was zero or negative. Direction belongs to the kind, not the sign.
    #[error("amount_cents must be greater than zero, got {got}")]
    NonPositiveAmount {
        /// The rejected input.
        got: i64,
    },

    /// The kind wasn't one of the two accepted tokens.
    #[error("kind must be one of expense, income; got {got:?}")]
    InvalidKind {
        /// The rejected input.
        got: String,
    },
}

/// The fields of an inbound transaction, checked and normalised.
///
/// Holding these in a distinct type means a caller can't accidentally use the
/// raw input where the cleaned-up value was intended: the merchant here is
/// already trimmed, and the date is already known to exist on the calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTransaction {
    /// The date, parsed. Write it back out with
    /// [`date_string`](Self::date_string) to get the canonical `YYYY-MM-DD`.
    pub date: NaiveDate,
    /// The merchant with surrounding whitespace removed, original casing kept.
    pub merchant: String,
    /// A positive amount in cents.
    pub amount_cents: i64,
    pub kind: TransactionKind,
}

impl ValidatedTransaction {
    /// The date in the canonical `YYYY-MM-DD` form the rest of the app stores.
    pub fn date_string(&self) -> String {
        self.date.format("%Y-%m-%d").to_string()
    }
}

/// Checks that a merchant has some substance to it, returning it trimmed.
///
/// The original casing is kept: the fingerprint folds case for matching, but
/// what the user sees should be what their bank sent.
pub fn validate_merchant(merchant: &str) -> Result<&str, ValidationError> {
    let trimmed = merchant.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::EmptyMerchant);
    }
    Ok(trimmed)
}

/// Checks that a date is a real calendar date written as exactly `YYYY-MM-DD`.
///
/// The format check is deliberately stricter than a date parser would be. Dates
/// are compared and sorted as raw strings throughout the app, so `2026-3-4`
/// would order wrongly against `2026-03-04` even though both name the same day.
/// Rejecting the loose form at the door keeps every stored date comparable.
pub fn validate_date(date: &str) -> Result<NaiveDate, ValidationError> {
    let rejected = || ValidationError::InvalidDate { got: date.to_string() };

    let bytes = date.as_bytes();
    let shaped_correctly = bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        });
    if !shaped_correctly {
        return Err(rejected());
    }

    // Parsing is what rejects the dates that look right but don't exist:
    // month 13, February 30, February 29 in a non-leap year.
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| rejected())
}

/// Checks that an amount is positive.
///
/// Money is stored as a positive count of cents with the direction carried by
/// [`TransactionKind`]; a negative amount would double up with the kind and
/// make every total ambiguous. Zero is rejected as well — it records nothing.
pub fn validate_amount_cents(amount_cents: i64) -> Result<i64, ValidationError> {
    if amount_cents <= 0 {
        return Err(ValidationError::NonPositiveAmount { got: amount_cents });
    }
    Ok(amount_cents)
}

/// Parses a transaction kind from an untrusted token.
pub fn validate_kind(kind: &str) -> Result<TransactionKind, ValidationError> {
    TransactionKind::from_str(kind).map_err(|ParseEnumError { value, .. }| {
        ValidationError::InvalidKind { got: value }
    })
}

/// Validates the raw fields of an inbound transaction, before it becomes a
/// [`Transaction`].
///
/// This is the entry point for anything crossing the API boundary. The first
/// problem found is the one reported.
pub fn validate_new_transaction(
    date: &str,
    merchant: &str,
    amount_cents: i64,
    kind: &str,
) -> Result<ValidatedTransaction, ValidationError> {
    Ok(ValidatedTransaction {
        date: validate_date(date)?,
        merchant: validate_merchant(merchant)?.to_string(),
        amount_cents: validate_amount_cents(amount_cents)?,
        kind: validate_kind(kind)?,
    })
}

/// Re-checks an already-typed [`Transaction`], for use on the way back out of
/// storage or after an edit.
///
/// The kind needs no check here — the type system already guarantees it.
pub fn validate_transaction(transaction: &Transaction) -> Result<(), ValidationError> {
    validate_date(&transaction.date)?;
    validate_merchant(&transaction.merchant)?;
    validate_amount_cents(transaction.amount_cents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TransactionSource, now_timestamp};
    use crate::transaction_fingerprint;
    use uuid::Uuid;

    fn sample_transaction() -> Transaction {
        let now = now_timestamp();
        Transaction {
            id: Uuid::new_v4(),
            date: "2026-03-04".to_string(),
            merchant: "Starbucks".to_string(),
            category_id: None,
            amount_cents: 545,
            kind: TransactionKind::Expense,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Manual,
            fingerprint: transaction_fingerprint("2026-03-04", "Starbucks", 545, "Main Account"),
            sms_hash: None,
            created_at: now.clone(),
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn merchant_is_accepted_and_trimmed() {
        assert_eq!(validate_merchant("Starbucks"), Ok("Starbucks"));
        assert_eq!(validate_merchant("  Whole Foods  "), Ok("Whole Foods"));
    }

    #[test]
    fn merchant_casing_survives_validation() {
        // Only the fingerprint folds case; the display value is left alone.
        assert_eq!(validate_merchant(" IKEA "), Ok("IKEA"));
    }

    #[test]
    fn empty_or_blank_merchant_is_rejected() {
        assert_eq!(validate_merchant(""), Err(ValidationError::EmptyMerchant));
        assert_eq!(validate_merchant("   "), Err(ValidationError::EmptyMerchant));
        assert_eq!(validate_merchant("\t\n"), Err(ValidationError::EmptyMerchant));
        assert_eq!(
            ValidationError::EmptyMerchant.to_string(),
            "merchant must not be empty"
        );
    }

    #[test]
    fn well_formed_dates_are_accepted() {
        assert_eq!(
            validate_date("2026-03-04"),
            Ok(NaiveDate::from_ymd_opt(2026, 3, 4).unwrap())
        );
        assert_eq!(
            validate_date("2024-02-29"),
            Ok(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()),
            "2024 is a leap year"
        );
    }

    #[test]
    fn dates_that_are_not_exactly_yyyy_mm_dd_are_rejected() {
        for bad in [
            "2026-3-4",     // unpadded — would sort wrongly as a string
            "2026/03/04",   // wrong separator
            "20260304",     // no separators
            "26-03-04",     // two-digit year
            "04-03-2026",   // reversed
            "2026-03-04 ",  // trailing space
            " 2026-03-04",  // leading space
            "2026-03-04T00:00:00Z", // timestamp, not a date
            "",
            "not a date",
        ] {
            assert_eq!(
                validate_date(bad),
                Err(ValidationError::InvalidDate { got: bad.to_string() }),
                "{bad:?} should have been rejected"
            );
        }
    }

    #[test]
    fn dates_that_look_right_but_do_not_exist_are_rejected() {
        for bad in ["2026-13-01", "2026-00-10", "2026-02-30", "2026-04-31", "2023-02-29"] {
            assert_eq!(
                validate_date(bad),
                Err(ValidationError::InvalidDate { got: bad.to_string() }),
                "{bad:?} is not a real date"
            );
        }
    }

    #[test]
    fn invalid_date_error_names_the_input() {
        assert_eq!(
            validate_date("2026-02-30").unwrap_err().to_string(),
            "date must be a real calendar date in YYYY-MM-DD form, got \"2026-02-30\""
        );
    }

    #[test]
    fn positive_amounts_are_accepted() {
        assert_eq!(validate_amount_cents(1), Ok(1));
        assert_eq!(validate_amount_cents(545), Ok(545));
        assert_eq!(validate_amount_cents(i64::MAX), Ok(i64::MAX));
    }

    #[test]
    fn zero_and_negative_amounts_are_rejected() {
        assert_eq!(
            validate_amount_cents(0),
            Err(ValidationError::NonPositiveAmount { got: 0 })
        );
        assert_eq!(
            validate_amount_cents(-545),
            Err(ValidationError::NonPositiveAmount { got: -545 })
        );
        assert_eq!(
            validate_amount_cents(-1).unwrap_err().to_string(),
            "amount_cents must be greater than zero, got -1"
        );
    }

    #[test]
    fn known_kinds_are_accepted_in_any_casing() {
        assert_eq!(validate_kind("expense"), Ok(TransactionKind::Expense));
        assert_eq!(validate_kind("income"), Ok(TransactionKind::Income));
        assert_eq!(validate_kind(" Income "), Ok(TransactionKind::Income));
    }

    #[test]
    fn unknown_kinds_are_rejected() {
        assert_eq!(
            validate_kind("transfer"),
            Err(ValidationError::InvalidKind { got: "transfer".to_string() })
        );
        assert_eq!(
            validate_kind("").unwrap_err().to_string(),
            "kind must be one of expense, income; got \"\""
        );
    }

    #[test]
    fn a_good_inbound_transaction_passes_and_comes_back_normalised() {
        let validated =
            validate_new_transaction("2026-03-04", "  Starbucks  ", 545, "EXPENSE").unwrap();

        assert_eq!(validated.merchant, "Starbucks");
        assert_eq!(validated.amount_cents, 545);
        assert_eq!(validated.kind, TransactionKind::Expense);
        assert_eq!(validated.date_string(), "2026-03-04");
    }

    #[test]
    fn each_bad_field_fails_the_inbound_check_on_its_own() {
        assert_eq!(
            validate_new_transaction("2026-02-30", "Starbucks", 545, "expense"),
            Err(ValidationError::InvalidDate { got: "2026-02-30".to_string() })
        );
        assert_eq!(
            validate_new_transaction("2026-03-04", "   ", 545, "expense"),
            Err(ValidationError::EmptyMerchant)
        );
        assert_eq!(
            validate_new_transaction("2026-03-04", "Starbucks", 0, "expense"),
            Err(ValidationError::NonPositiveAmount { got: 0 })
        );
        assert_eq!(
            validate_new_transaction("2026-03-04", "Starbucks", 545, "refund"),
            Err(ValidationError::InvalidKind { got: "refund".to_string() })
        );
    }

    #[test]
    fn a_well_formed_stored_transaction_passes() {
        assert_eq!(validate_transaction(&sample_transaction()), Ok(()));
    }

    #[test]
    fn a_stored_transaction_is_rejected_on_each_bad_field() {
        let mut bad_date = sample_transaction();
        bad_date.date = "2026-3-4".to_string();
        assert_eq!(
            validate_transaction(&bad_date),
            Err(ValidationError::InvalidDate { got: "2026-3-4".to_string() })
        );

        let mut bad_merchant = sample_transaction();
        bad_merchant.merchant = "  ".to_string();
        assert_eq!(validate_transaction(&bad_merchant), Err(ValidationError::EmptyMerchant));

        let mut bad_amount = sample_transaction();
        bad_amount.amount_cents = -1;
        assert_eq!(
            validate_transaction(&bad_amount),
            Err(ValidationError::NonPositiveAmount { got: -1 })
        );
    }
}
