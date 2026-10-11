//! Duplicate detection.
//!
//! The same purchase can reach MyLedger more than once — a CSV import that
//! overlaps a previous one, an SMS alert for a transaction already typed in by
//! hand. A fingerprint gives every transaction a stable identity derived from
//! what the user would consider "the same purchase", so the second arrival can
//! be recognised and dropped.

/// Computes the duplicate-detection fingerprint for a transaction.
///
/// The shape is:
///
/// ```text
/// date | merchant | amount_cents | account_name
/// ```
///
/// Merchant and account name are trimmed and lowercased, so the same purchase
/// fingerprints identically no matter how a bank export happened to capitalise
/// or pad it. Nothing else is normalised: two different dates, amounts or
/// accounts are two different transactions.
///
/// The account is identified by *name* rather than ID, because an import may
/// only know what the account was called.
///
/// ```
/// # use myledger_core::transaction_fingerprint;
/// assert_eq!(
///     transaction_fingerprint("2026-03-04", "Starbucks ", 545, "Main Account"),
///     transaction_fingerprint("2026-03-04", "starbucks", 545, "main account"),
/// );
/// ```
pub fn transaction_fingerprint(
    date: &str,
    merchant: &str,
    amount_cents: i64,
    account_name: &str,
) -> String {
    format!(
        "{}|{}|{}|{}",
        date,
        merchant.trim().to_lowercase(),
        amount_cents,
        account_name.trim().to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::transaction_fingerprint;

    #[test]
    fn has_the_documented_shape() {
        assert_eq!(
            transaction_fingerprint("2026-03-04", "Starbucks", 545, "Main Account"),
            "2026-03-04|starbucks|545|main account"
        );
    }

    #[test]
    fn merchant_casing_and_padding_are_ignored() {
        let padded = transaction_fingerprint("2026-03-04", "  Starbucks  ", 545, "Main Account");
        let bare = transaction_fingerprint("2026-03-04", "starbucks", 545, "Main Account");
        let shouted = transaction_fingerprint("2026-03-04", "STARBUCKS", 545, "Main Account");

        assert_eq!(padded, bare);
        assert_eq!(bare, shouted);
    }

    #[test]
    fn account_casing_and_padding_are_ignored() {
        assert_eq!(
            transaction_fingerprint("2026-03-04", "Starbucks", 545, "\tCredit Card\n"),
            transaction_fingerprint("2026-03-04", "Starbucks", 545, "credit card"),
        );
    }

    #[test]
    fn interior_whitespace_is_preserved() {
        // Only the ends are trimmed. "Whole Foods" and "WholeFoods" are two
        // different merchants as far as duplicate detection is concerned.
        assert_ne!(
            transaction_fingerprint("2026-03-04", "Whole Foods", 1200, "Main Account"),
            transaction_fingerprint("2026-03-04", "WholeFoods", 1200, "Main Account"),
        );
    }

    #[test]
    fn non_ascii_merchants_fold_case_too() {
        assert_eq!(
            transaction_fingerprint("2026-03-04", "CAFÉ MÜNCHEN", 900, "Cash"),
            transaction_fingerprint("2026-03-04", "café münchen", 900, "Cash"),
        );
    }

    #[test]
    fn each_component_changes_the_fingerprint() {
        let base = transaction_fingerprint("2026-03-04", "Starbucks", 545, "Main Account");

        assert_ne!(base, transaction_fingerprint("2026-03-05", "Starbucks", 545, "Main Account"));
        assert_ne!(base, transaction_fingerprint("2026-03-04", "Peets", 545, "Main Account"));
        assert_ne!(base, transaction_fingerprint("2026-03-04", "Starbucks", 546, "Main Account"));
        assert_ne!(base, transaction_fingerprint("2026-03-04", "Starbucks", 545, "Credit Card"));
    }

    #[test]
    fn two_identical_purchases_on_the_same_day_collide_by_design() {
        // Buying the same coffee twice in one day looks like a duplicate. That is
        // the intended trade-off: silently dropping the second is safer than
        // silently double-counting an import, and the user can still add it by
        // hand with a nudged amount or a distinct merchant.
        let first = transaction_fingerprint("2026-03-04", "Starbucks", 545, "Main Account");
        let second = transaction_fingerprint("2026-03-04", "Starbucks", 545, "Main Account");
        assert_eq!(first, second);
    }

    #[test]
    fn amounts_are_rendered_as_plain_integers() {
        assert_eq!(
            transaction_fingerprint("2026-03-04", "Refund Co", -2500, "Main Account"),
            "2026-03-04|refund co|-2500|main account"
        );
        assert_eq!(
            transaction_fingerprint("2026-03-04", "Zero Co", 0, "Cash"),
            "2026-03-04|zero co|0|cash"
        );
    }
}
