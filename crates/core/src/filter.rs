//! Search and filters for the transactions list.
//!
//! Here rather than in the web app for the same reason as
//! [`analysis`](crate::analysis): a test written in a `wasm32`-only crate
//! never runs. "Does searching for a tag name find the transaction" is
//! exactly the kind of question that deserves an answer from `cargo test`
//! rather than from clicking around.
//!
//! Filtering happens on the client over the ledger it already holds, not as a
//! query parameter on the server. That is a deliberate consequence of
//! `GET /api/v1/state` returning the whole ledger: the list is already in
//! memory, so narrowing it needs no round trip, and there is no second code
//! path that could disagree with the first about what "matches" means.

use uuid::Uuid;

use crate::analysis::Names;
use crate::model::{Transaction, TransactionKind};
use crate::period::DateRange;

/// Which category the list is restricted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CategoryChoice {
    #[default]
    Any,
    /// Only transactions filed under no category — the ones needing
    /// attention, which is why this is a first-class choice rather than
    /// something to be found by searching.
    None,
    One(Uuid),
}

/// Which account the list is restricted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountChoice {
    #[default]
    Any,
    /// Only transactions with no account recorded.
    None,
    One(Uuid),
}

/// Everything narrowing the transactions list at once.
#[derive(Debug, Clone, Default)]
pub struct TransactionFilter {
    /// Free text, matched against merchant, category name and tag names.
    pub query: String,
    pub category: CategoryChoice,
    pub account: AccountChoice,
    /// The selected period, already resolved to dates.
    pub range: DateRange,
}

impl TransactionFilter {
    /// Whether anything is actually being narrowed.
    ///
    /// Used to decide between "no transactions yet" and "nothing matches what
    /// you asked for", which are different situations and deserve different
    /// empty states.
    pub fn is_narrowing(&self) -> bool {
        !self.query.trim().is_empty()
            || self.category != CategoryChoice::Any
            || self.account != AccountChoice::Any
            || !self.range.is_everything()
    }

    /// Whether one transaction passes every part of the filter.
    pub fn matches(&self, transaction: &Transaction, names: &impl Names) -> bool {
        if transaction.deleted_at.is_some() {
            return false;
        }
        if !self.range.contains(&transaction.date) {
            return false;
        }

        match self.category {
            CategoryChoice::Any => {}
            CategoryChoice::None => {
                if transaction.category_id.is_some() {
                    return false;
                }
            }
            CategoryChoice::One(wanted) => {
                if transaction.category_id != Some(wanted) {
                    return false;
                }
            }
        }

        match self.account {
            AccountChoice::Any => {}
            AccountChoice::None => {
                if transaction.account_id.is_some() {
                    return false;
                }
            }
            AccountChoice::One(wanted) => {
                if transaction.account_id != Some(wanted) {
                    return false;
                }
            }
        }

        self.query_matches(transaction, names)
    }

    /// Whether the free-text query matches.
    ///
    /// Every whitespace-separated word must match somewhere, so typing more
    /// narrows the list rather than widening it. Matching is case-insensitive
    /// and on substrings, because this is a search box, not a rule engine —
    /// "groc" should find "Groceries".
    fn query_matches(&self, transaction: &Transaction, names: &impl Names) -> bool {
        let query = self.query.trim();
        if query.is_empty() {
            return true;
        }

        let mut haystack = transaction.merchant.to_lowercase();
        if let Some(name) = transaction.category_id.and_then(|id| names.category_name(id)) {
            haystack.push('\u{1}');
            haystack.push_str(&name.to_lowercase());
        }
        if let Some(name) = transaction.account_id.and_then(|id| names.account_name(id)) {
            haystack.push('\u{1}');
            haystack.push_str(&name.to_lowercase());
        }
        for tag in &transaction.tags {
            if let Some(name) = names.tag_name(*tag) {
                haystack.push('\u{1}');
                haystack.push_str(&name.to_lowercase());
            }
        }

        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

/// The transactions passing a filter, newest first.
pub fn apply<'a>(
    transactions: &'a [Transaction],
    filter: &TransactionFilter,
    names: &impl Names,
) -> Vec<&'a Transaction> {
    let mut found: Vec<&Transaction> =
        transactions.iter().filter(|t| filter.matches(t, names)).collect();
    found.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.created_at.cmp(&a.created_at)));
    found
}

/// Income and expense totals for a filtered list.
///
/// The list the user is looking at should have its own footer totals; taking
/// them from the same slice that is rendered means the number under the table
/// always describes the rows in it.
pub fn totals(rows: &[&Transaction]) -> (i64, i64) {
    let mut income = 0i64;
    let mut expense = 0i64;
    for transaction in rows {
        match transaction.kind {
            TransactionKind::Income => income = income.saturating_add(transaction.amount_cents),
            TransactionKind::Expense => expense = expense.saturating_add(transaction.amount_cents),
        }
    }
    (income, expense)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Period, TransactionSource, now_timestamp};

    struct Lookup {
        categories: Vec<(Uuid, &'static str)>,
        accounts: Vec<(Uuid, &'static str)>,
        tags: Vec<(Uuid, &'static str)>,
    }

    impl Names for Lookup {
        fn category_name(&self, id: Uuid) -> Option<&str> {
            self.categories.iter().find(|(k, _)| *k == id).map(|(_, n)| *n)
        }
        fn account_name(&self, id: Uuid) -> Option<&str> {
            self.accounts.iter().find(|(k, _)| *k == id).map(|(_, n)| *n)
        }
        fn tag_name(&self, id: Uuid) -> Option<&str> {
            self.tags.iter().find(|(k, _)| *k == id).map(|(_, n)| *n)
        }
    }

    fn transaction(merchant: &str, date: &str) -> Transaction {
        Transaction {
            id: Uuid::new_v4(),
            date: date.to_string(),
            merchant: merchant.to_string(),
            category_id: None,
            amount_cents: 1_000,
            kind: TransactionKind::Expense,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Manual,
            fingerprint: format!("{date}|{}|1000|", merchant.to_lowercase()),
            sms_hash: None,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
            deleted_at: None,
        }
    }

    /// A ledger with one of everything worth filtering on.
    fn fixture() -> (Vec<Transaction>, Lookup, Uuid, Uuid, Uuid) {
        let groceries = Uuid::new_v4();
        let dining = Uuid::new_v4();
        let main = Uuid::new_v4();
        let cash = Uuid::new_v4();
        let work = Uuid::new_v4();

        let names = Lookup {
            categories: vec![(groceries, "Groceries"), (dining, "Dining")],
            accounts: vec![(main, "Main Account"), (cash, "Cash")],
            tags: vec![(work, "work-expense")],
        };

        let mut ledger = vec![
            transaction("Corner Grocer", "2026-10-05"),
            transaction("Pasta Place", "2026-09-20"),
            transaction("Unfiled Shop", "2026-08-02"),
        ];
        ledger[0].category_id = Some(groceries);
        ledger[0].account_id = Some(main);
        ledger[1].category_id = Some(dining);
        ledger[1].account_id = Some(cash);
        ledger[1].tags = vec![work];

        (ledger, names, groceries, main, work)
    }

    #[test]
    fn an_empty_filter_keeps_everything_and_sorts_newest_first() {
        let (ledger, names, _, _, _) = fixture();
        let rows = apply(&ledger, &TransactionFilter::default(), &names);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].date, "2026-10-05");
        assert_eq!(rows[2].date, "2026-08-02");
    }

    #[test]
    fn an_empty_filter_is_not_narrowing() {
        assert!(!TransactionFilter::default().is_narrowing());
    }

    #[test]
    fn a_period_counts_as_narrowing() {
        // So the empty state can say "nothing in this period" rather than
        // "no transactions yet", which would be untrue.
        let filter = TransactionFilter {
            range: Period::ThisMonth.range("2026-10-11"),
            ..Default::default()
        };
        assert!(filter.is_narrowing());
    }

    #[test]
    fn deleted_transactions_never_match() {
        let (mut ledger, names, _, _, _) = fixture();
        ledger[0].deleted_at = Some(now_timestamp());
        assert_eq!(apply(&ledger, &TransactionFilter::default(), &names).len(), 2);
    }

    // ---- search --------------------------------------------------------

    #[test]
    fn search_matches_the_merchant_case_insensitively() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "GROCER".into(), ..Default::default() };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Corner Grocer");
    }

    #[test]
    fn search_matches_a_partial_word() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "groc".into(), ..Default::default() };
        assert_eq!(apply(&ledger, &filter, &names).len(), 1);
    }

    #[test]
    fn search_matches_the_category_name() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "dining".into(), ..Default::default() };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Pasta Place");
    }

    #[test]
    fn search_matches_a_tag_name() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "work-expense".into(), ..Default::default() };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Pasta Place");
    }

    #[test]
    fn search_matches_the_account_name() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "cash".into(), ..Default::default() };
        assert_eq!(apply(&ledger, &filter, &names).len(), 1);
    }

    #[test]
    fn every_word_has_to_match_so_typing_more_narrows() {
        let (ledger, names, _, _, _) = fixture();
        // "grocer" is in the merchant and "groceries" in the category: both
        // words hit the same row.
        let both = TransactionFilter { query: "grocer groceries".into(), ..Default::default() };
        assert_eq!(apply(&ledger, &both, &names).len(), 1);

        // Adding a word that matches nothing empties the list rather than
        // widening it.
        let neither = TransactionFilter { query: "grocer dining".into(), ..Default::default() };
        assert!(apply(&ledger, &neither, &names).is_empty());
    }

    #[test]
    fn whitespace_only_search_is_no_search() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { query: "   ".into(), ..Default::default() };
        assert_eq!(apply(&ledger, &filter, &names).len(), 3);
        assert!(!filter.is_narrowing());
    }

    #[test]
    fn search_does_not_run_words_together_across_fields() {
        // Merchant and category are joined by a separator, so a query
        // spanning the join cannot match something that isn't there.
        let (ledger, names, _, _, _) = fixture();
        let filter =
            TransactionFilter { query: "grocergroceries".into(), ..Default::default() };
        assert!(apply(&ledger, &filter, &names).is_empty());
    }

    // ---- category and account -------------------------------------------

    #[test]
    fn filtering_by_category_keeps_only_that_category() {
        let (ledger, names, groceries, _, _) = fixture();
        let filter = TransactionFilter {
            category: CategoryChoice::One(groceries),
            ..Default::default()
        };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Corner Grocer");
    }

    #[test]
    fn filtering_by_no_category_finds_the_unfiled_ones() {
        let (ledger, names, _, _, _) = fixture();
        let filter =
            TransactionFilter { category: CategoryChoice::None, ..Default::default() };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Unfiled Shop");
    }

    #[test]
    fn filtering_by_account_keeps_only_that_account() {
        let (ledger, names, _, main, _) = fixture();
        let filter =
            TransactionFilter { account: AccountChoice::One(main), ..Default::default() };
        assert_eq!(apply(&ledger, &filter, &names).len(), 1);
    }

    #[test]
    fn filtering_by_no_account_finds_the_unassigned_ones() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter { account: AccountChoice::None, ..Default::default() };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].merchant, "Unfiled Shop");
    }

    #[test]
    fn filters_combine_rather_than_compete() {
        let (ledger, names, groceries, main, _) = fixture();
        let filter = TransactionFilter {
            query: "corner".into(),
            category: CategoryChoice::One(groceries),
            account: AccountChoice::One(main),
            range: Period::ThisMonth.range("2026-10-11"),
        };
        assert_eq!(apply(&ledger, &filter, &names).len(), 1);

        // One part failing is enough to exclude the row.
        let filter = TransactionFilter {
            query: "corner".into(),
            category: CategoryChoice::One(groceries),
            account: AccountChoice::None,
            range: DateRange::everything(),
        };
        assert!(apply(&ledger, &filter, &names).is_empty());
    }

    #[test]
    fn a_period_restricts_the_list() {
        let (ledger, names, _, _, _) = fixture();
        let filter = TransactionFilter {
            range: Period::ThisMonth.range("2026-10-11"),
            ..Default::default()
        };
        let rows = apply(&ledger, &filter, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "2026-10-05");
    }

    // ---- totals ---------------------------------------------------------

    #[test]
    fn totals_describe_the_rows_shown() {
        let (mut ledger, names, _, _, _) = fixture();
        ledger[0].kind = TransactionKind::Income;
        ledger[0].amount_cents = 5_000;

        let rows = apply(&ledger, &TransactionFilter::default(), &names);
        let (income, expense) = totals(&rows);
        assert_eq!(income, 5_000);
        assert_eq!(expense, 2_000);
    }

    #[test]
    fn totals_of_nothing_are_zero() {
        assert_eq!(totals(&[]), (0, 0));
    }
}
