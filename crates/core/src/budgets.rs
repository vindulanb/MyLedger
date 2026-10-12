//! Budget and goal progress.
//!
//! In `core` for the same reason as the dashboard's arithmetic: the web app
//! compiles only for `wasm32`, so a test written there never runs, and "is
//! this budget over" is not a question to answer by eye.
//!
//! Two decisions worth stating, because both are places a plausible
//! implementation gets it wrong:
//!
//! - **Budgets are measured over the calendar month, not the selected
//!   period.** A monthly limit means this month. Measuring it against
//!   whatever period the user last picked on the Dashboard would make a
//!   budget look breached because they were looking at the year.
//! - **Only expenses count against a budget.** A refund filed under the same
//!   category reduces spending, which it should, but income filed there does
//!   not inflate the budget's headroom.

use uuid::Uuid;

use crate::analysis::in_range;
use crate::model::{Budget, Goal, Transaction, TransactionKind};
use crate::period::DateRange;

/// Where one budget stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetProgress {
    pub limit_cents: i64,
    pub spent_cents: i64,
    /// How many transactions went into `spent_cents`.
    pub counted: usize,
}

impl BudgetProgress {
    /// What is left. Negative once the limit is passed.
    pub fn remaining_cents(&self) -> i64 {
        self.limit_cents - self.spent_cents
    }

    /// Whether spending has passed the limit.
    ///
    /// Strictly greater: spending exactly the limit is not over it.
    pub fn is_over(&self) -> bool {
        self.spent_cents > self.limit_cents
    }

    /// How much over, or zero.
    pub fn over_by_cents(&self) -> i64 {
        (self.spent_cents - self.limit_cents).max(0)
    }

    /// Share of the limit used, as a whole percentage.
    ///
    /// A zero limit gives 100% once anything is spent and 0% before that,
    /// rather than dividing by zero. Not capped at 100 — a bar that stops at
    /// full would hide how far over the user is, so the caller clamps the
    /// *width* and keeps the number.
    pub fn percent_used(&self) -> i64 {
        if self.limit_cents <= 0 {
            return if self.spent_cents > 0 { 100 } else { 0 };
        }
        percent(self.spent_cents, self.limit_cents)
    }

    /// The percentage to draw, which is the one place clamping belongs.
    pub fn bar_percent(&self) -> i64 {
        self.percent_used().clamp(0, 100)
    }
}

/// Where one budget stands over a span of dates.
///
/// A budget with no category measures *all* spending in the range — which is
/// how a total monthly allowance is expressed, rather than a special case.
pub fn progress(
    transactions: &[Transaction],
    range: &DateRange,
    budget: &Budget,
) -> BudgetProgress {
    let mut spent = 0i64;
    let mut counted = 0usize;

    for transaction in in_range(transactions, range) {
        if transaction.kind != TransactionKind::Expense {
            continue;
        }
        if let Some(wanted) = budget.category_id
            && transaction.category_id != Some(wanted)
        {
            continue;
        }
        spent = spent.saturating_add(transaction.amount_cents);
        counted += 1;
    }

    BudgetProgress { limit_cents: budget.monthly_limit_cents, spent_cents: spent, counted }
}

/// Everything across the active budgets, for the health summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetHealth {
    pub budgets: usize,
    pub total_limit_cents: i64,
    pub total_spent_cents: i64,
    pub over_count: usize,
    /// Budgets at or above four fifths of their limit without being over.
    pub near_count: usize,
}

/// How close to the line the summary ring starts warning.
pub const NEAR_LIMIT_PERCENT: i64 = 80;

impl BudgetHealth {
    pub fn remaining_cents(&self) -> i64 {
        self.total_limit_cents - self.total_spent_cents
    }

    /// Share of the combined limits used, as a whole percentage.
    pub fn percent_used(&self) -> i64 {
        if self.total_limit_cents <= 0 {
            return if self.total_spent_cents > 0 { 100 } else { 0 };
        }
        percent(self.total_spent_cents, self.total_limit_cents)
    }

    pub fn bar_percent(&self) -> i64 {
        self.percent_used().clamp(0, 100)
    }

    pub fn is_empty(&self) -> bool {
        self.budgets == 0
    }
}

/// Totals the active budgets.
///
/// Inactive ones are left out entirely: a paused budget is one the user has
/// said not to measure, so counting its limit would make the ring look
/// healthier than it is.
pub fn health(transactions: &[Transaction], range: &DateRange, budgets: &[Budget]) -> BudgetHealth {
    let mut summary = BudgetHealth::default();

    for budget in budgets.iter().filter(|b| b.deleted_at.is_none() && b.active) {
        let standing = progress(transactions, range, budget);
        summary.budgets += 1;
        summary.total_limit_cents = summary.total_limit_cents.saturating_add(standing.limit_cents);
        summary.total_spent_cents = summary.total_spent_cents.saturating_add(standing.spent_cents);
        if standing.is_over() {
            summary.over_count += 1;
        } else if standing.percent_used() >= NEAR_LIMIT_PERCENT {
            summary.near_count += 1;
        }
    }

    summary
}

/// Where one goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalProgress {
    pub target_cents: i64,
    pub current_cents: i64,
}

impl GoalProgress {
    /// How much is still needed. Zero once the target is reached.
    pub fn remaining_cents(&self) -> i64 {
        (self.target_cents - self.current_cents).max(0)
    }

    pub fn is_reached(&self) -> bool {
        self.current_cents >= self.target_cents && self.target_cents > 0
    }

    /// Share of the target saved, as a whole percentage.
    ///
    /// A zero or negative target reads as reached when anything is saved,
    /// since there is nothing left to save towards.
    pub fn percent_saved(&self) -> i64 {
        if self.target_cents <= 0 {
            return 100;
        }
        percent(self.current_cents.max(0), self.target_cents)
    }

    pub fn bar_percent(&self) -> i64 {
        self.percent_saved().clamp(0, 100)
    }
}

/// `part` as a whole percentage of `whole`.
///
/// Computed in `i128` so the multiply cannot overflow, then *clamped* rather
/// than cast — a ledger with an `i64::MAX` charge against a one-cent budget
/// produces a percentage larger than an `i64` holds, and a bare cast would
/// wrap it to a negative and draw an empty bar for the most over-budget
/// situation possible.
fn percent(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        return if part > 0 { 100 } else { 0 };
    }
    let scaled = part as i128 * 100 / whole as i128;
    scaled.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Reads a goal's standing.
pub fn goal_progress(goal: &Goal) -> GoalProgress {
    GoalProgress { target_cents: goal.target_cents, current_cents: goal.current_cents }
}

/// The name to show for a budget's category.
pub fn budget_label(budget: &Budget, name: Option<&str>) -> String {
    match (budget.category_id, name) {
        (None, _) => "All spending".to_string(),
        (Some(_), Some(name)) => name.to_string(),
        // The category was deleted and its name is gone; say so rather than
        // printing a bare UUID.
        (Some(_), None) => "A category that no longer exists".to_string(),
    }
}

/// Whether a budget's category is one of several the user already has a
/// budget for.
///
/// Two budgets on one category would both measure the same spending and both
/// look breached, so the form refuses it; this is the check behind that.
pub fn category_taken(budgets: &[Budget], category_id: Option<Uuid>, except: Option<Uuid>) -> bool {
    budgets
        .iter()
        .filter(|b| b.deleted_at.is_none())
        .filter(|b| Some(b.id) != except)
        .any(|b| b.category_id == category_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Period, TransactionSource, now_timestamp};

    const TODAY: &str = "2026-10-11";

    fn month() -> DateRange {
        Period::ThisMonth.range(TODAY)
    }

    fn budget(limit: i64, category: Option<Uuid>) -> Budget {
        Budget {
            id: Uuid::new_v4(),
            category_id: category,
            monthly_limit_cents: limit,
            active: true,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
            deleted_at: None,
        }
    }

    fn spend(date: &str, cents: i64, category: Option<Uuid>) -> Transaction {
        Transaction {
            id: Uuid::new_v4(),
            date: date.to_string(),
            merchant: "Shop".to_string(),
            category_id: category,
            amount_cents: cents,
            kind: TransactionKind::Expense,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Manual,
            fingerprint: format!("{date}|shop|{cents}|{category:?}"),
            sms_hash: None,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
            deleted_at: None,
        }
    }

    fn earn(date: &str, cents: i64, category: Option<Uuid>) -> Transaction {
        let mut transaction = spend(date, cents, category);
        transaction.kind = TransactionKind::Income;
        transaction
    }

    fn goal(target: i64, current: i64) -> Goal {
        Goal {
            id: Uuid::new_v4(),
            name: "A goal".to_string(),
            target_cents: target,
            current_cents: current,
            due_date: None,
            note: None,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
            deleted_at: None,
        }
    }

    // ---- one budget -----------------------------------------------------

    #[test]
    fn a_budget_with_no_spending_is_untouched() {
        let standing = progress(&[], &month(), &budget(50_000, None));
        assert_eq!(standing.spent_cents, 0);
        assert_eq!(standing.remaining_cents(), 50_000);
        assert_eq!(standing.percent_used(), 0);
        assert!(!standing.is_over());
    }

    #[test]
    fn spending_counts_against_the_matching_category() {
        let groceries = Uuid::new_v4();
        let dining = Uuid::new_v4();
        let ledger = vec![
            spend("2026-10-02", 20_000, Some(groceries)),
            spend("2026-10-03", 5_000, Some(dining)),
        ];

        let standing = progress(&ledger, &month(), &budget(50_000, Some(groceries)));
        assert_eq!(standing.spent_cents, 20_000);
        assert_eq!(standing.counted, 1);
    }

    #[test]
    fn a_budget_with_no_category_measures_all_spending() {
        // Which is how a total monthly allowance is expressed.
        let ledger = vec![
            spend("2026-10-02", 20_000, Some(Uuid::new_v4())),
            spend("2026-10-03", 5_000, None),
        ];
        let standing = progress(&ledger, &month(), &budget(50_000, None));
        assert_eq!(standing.spent_cents, 25_000);
    }

    #[test]
    fn only_this_month_counts_against_a_monthly_limit() {
        let ledger = vec![spend("2026-09-30", 40_000, None), spend("2026-10-01", 1_000, None)];
        let standing = progress(&ledger, &month(), &budget(50_000, None));
        assert_eq!(standing.spent_cents, 1_000, "last month's spending is not this month's");
    }

    #[test]
    fn spending_later_this_month_still_counts() {
        // The month's range is open-ended, so a dated-ahead charge inside the
        // month is measured rather than hidden.
        let ledger = vec![spend("2026-10-31", 1_000, None)];
        assert_eq!(progress(&ledger, &month(), &budget(50_000, None)).spent_cents, 1_000);
    }

    #[test]
    fn income_filed_under_the_category_does_not_create_headroom() {
        let groceries = Uuid::new_v4();
        let ledger = vec![
            spend("2026-10-02", 20_000, Some(groceries)),
            earn("2026-10-03", 100_000, Some(groceries)),
        ];
        let standing = progress(&ledger, &month(), &budget(50_000, Some(groceries)));
        assert_eq!(standing.spent_cents, 20_000);
        assert_eq!(standing.remaining_cents(), 30_000);
    }

    #[test]
    fn deleted_transactions_do_not_count() {
        let mut ledger = vec![spend("2026-10-02", 20_000, None)];
        ledger[0].deleted_at = Some(now_timestamp());
        assert_eq!(progress(&ledger, &month(), &budget(50_000, None)).spent_cents, 0);
    }

    // ---- over budget -----------------------------------------------------

    #[test]
    fn spending_exactly_the_limit_is_not_over_it() {
        let ledger = vec![spend("2026-10-02", 50_000, None)];
        let standing = progress(&ledger, &month(), &budget(50_000, None));
        assert!(!standing.is_over());
        assert_eq!(standing.remaining_cents(), 0);
        assert_eq!(standing.percent_used(), 100);
        assert_eq!(standing.over_by_cents(), 0);
    }

    #[test]
    fn a_cent_past_the_limit_is_over_it() {
        let ledger = vec![spend("2026-10-02", 50_001, None)];
        let standing = progress(&ledger, &month(), &budget(50_000, None));
        assert!(standing.is_over());
        assert_eq!(standing.remaining_cents(), -1);
        assert_eq!(standing.over_by_cents(), 1);
    }

    #[test]
    fn the_percentage_is_not_capped_but_the_bar_is() {
        // A bar that stops at full would hide how far over the user is, so
        // the number keeps going and only the drawn width is clamped.
        let ledger = vec![spend("2026-10-02", 150_000, None)];
        let standing = progress(&ledger, &month(), &budget(50_000, None));
        assert_eq!(standing.percent_used(), 300);
        assert_eq!(standing.bar_percent(), 100);
    }

    #[test]
    fn a_zero_limit_does_not_divide_by_zero() {
        let empty = progress(&[], &month(), &budget(0, None));
        assert_eq!(empty.percent_used(), 0);

        let ledger = vec![spend("2026-10-02", 100, None)];
        let spent = progress(&ledger, &month(), &budget(0, None));
        assert_eq!(spent.percent_used(), 100);
        assert!(spent.is_over());
    }

    #[test]
    fn an_enormous_budget_does_not_overflow_the_percentage() {
        let ledger = vec![spend("2026-10-02", i64::MAX, None)];
        let standing = progress(&ledger, &month(), &budget(1, None));
        assert!(standing.percent_used() > 0, "no overflow to a negative");
    }

    // ---- health ----------------------------------------------------------

    #[test]
    fn no_budgets_is_an_empty_summary() {
        let summary = health(&[], &month(), &[]);
        assert!(summary.is_empty());
        assert_eq!(summary.percent_used(), 0);
    }

    #[test]
    fn the_summary_totals_every_active_budget() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let budgets = vec![budget(50_000, Some(a)), budget(20_000, Some(b))];
        let ledger = vec![spend("2026-10-02", 10_000, Some(a)), spend("2026-10-03", 5_000, Some(b))];

        let summary = health(&ledger, &month(), &budgets);
        assert_eq!(summary.budgets, 2);
        assert_eq!(summary.total_limit_cents, 70_000);
        assert_eq!(summary.total_spent_cents, 15_000);
        assert_eq!(summary.remaining_cents(), 55_000);
        assert_eq!(summary.percent_used(), 21);
        assert_eq!(summary.over_count, 0);
    }

    #[test]
    fn a_paused_budget_is_left_out_of_the_summary_entirely() {
        // Counting its limit would make the ring look healthier than it is.
        let a = Uuid::new_v4();
        let mut budgets = vec![budget(50_000, Some(a)), budget(20_000, None)];
        budgets[1].active = false;

        let summary = health(&[], &month(), &budgets);
        assert_eq!(summary.budgets, 1);
        assert_eq!(summary.total_limit_cents, 50_000);
    }

    #[test]
    fn a_deleted_budget_is_left_out_too() {
        let mut budgets = vec![budget(50_000, None)];
        budgets[0].deleted_at = Some(now_timestamp());
        assert!(health(&[], &month(), &budgets).is_empty());
    }

    #[test]
    fn the_summary_counts_which_budgets_are_over_and_which_are_close() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let budgets =
            vec![budget(10_000, Some(a)), budget(10_000, Some(b)), budget(10_000, Some(c))];
        let ledger = vec![
            spend("2026-10-02", 12_000, Some(a)), // over
            spend("2026-10-02", 9_000, Some(b)),  // 90%, near
            spend("2026-10-02", 1_000, Some(c)),  // fine
        ];

        let summary = health(&ledger, &month(), &budgets);
        assert_eq!(summary.over_count, 1);
        assert_eq!(summary.near_count, 1);
    }

    #[test]
    fn a_budget_that_is_over_is_not_also_counted_as_near() {
        let a = Uuid::new_v4();
        let budgets = vec![budget(10_000, Some(a))];
        let ledger = vec![spend("2026-10-02", 50_000, Some(a))];
        let summary = health(&ledger, &month(), &budgets);
        assert_eq!(summary.over_count, 1);
        assert_eq!(summary.near_count, 0);
    }

    // ---- labels ----------------------------------------------------------

    #[test]
    fn a_budget_without_a_category_is_called_all_spending() {
        assert_eq!(budget_label(&budget(1, None), None), "All spending");
    }

    #[test]
    fn a_budget_with_a_category_uses_its_name() {
        let groceries = Uuid::new_v4();
        assert_eq!(budget_label(&budget(1, Some(groceries)), Some("Groceries")), "Groceries");
    }

    #[test]
    fn a_budget_whose_category_is_gone_says_so_rather_than_printing_a_uuid() {
        let gone = Uuid::new_v4();
        assert_eq!(
            budget_label(&budget(1, Some(gone)), None),
            "A category that no longer exists"
        );
    }

    #[test]
    fn a_category_can_only_have_one_budget() {
        // Two would measure the same spending and both look breached.
        let groceries = Uuid::new_v4();
        let existing = vec![budget(10_000, Some(groceries))];
        assert!(category_taken(&existing, Some(groceries), None));
        assert!(!category_taken(&existing, Some(Uuid::new_v4()), None));

        // Editing the budget that holds it is not a clash with itself.
        assert!(!category_taken(&existing, Some(groceries), Some(existing[0].id)));
    }

    #[test]
    fn the_all_spending_budget_is_also_unique() {
        let existing = vec![budget(10_000, None)];
        assert!(category_taken(&existing, None, None));
    }

    #[test]
    fn a_deleted_budget_does_not_hold_its_category() {
        let groceries = Uuid::new_v4();
        let mut existing = vec![budget(10_000, Some(groceries))];
        existing[0].deleted_at = Some(now_timestamp());
        assert!(!category_taken(&existing, Some(groceries), None));
    }

    // ---- goals -----------------------------------------------------------

    #[test]
    fn a_goal_with_nothing_saved_is_at_zero() {
        let standing = goal_progress(&goal(100_000, 0));
        assert_eq!(standing.percent_saved(), 0);
        assert_eq!(standing.remaining_cents(), 100_000);
        assert!(!standing.is_reached());
    }

    #[test]
    fn a_goal_part_way_reports_what_is_left() {
        let standing = goal_progress(&goal(100_000, 25_000));
        assert_eq!(standing.percent_saved(), 25);
        assert_eq!(standing.remaining_cents(), 75_000);
    }

    #[test]
    fn a_goal_exactly_met_is_reached_and_needs_nothing() {
        let standing = goal_progress(&goal(100_000, 100_000));
        assert!(standing.is_reached());
        assert_eq!(standing.remaining_cents(), 0);
        assert_eq!(standing.percent_saved(), 100);
    }

    #[test]
    fn saving_past_a_goal_never_reports_a_negative_remainder() {
        let standing = goal_progress(&goal(100_000, 150_000));
        assert!(standing.is_reached());
        assert_eq!(standing.remaining_cents(), 0);
        assert_eq!(standing.percent_saved(), 150);
        assert_eq!(standing.bar_percent(), 100);
    }

    #[test]
    fn a_goal_with_no_target_does_not_divide_by_zero() {
        let standing = goal_progress(&goal(0, 5_000));
        assert_eq!(standing.percent_saved(), 100);
        assert_eq!(standing.remaining_cents(), 0);
    }

    #[test]
    fn a_huge_goal_does_not_overflow_its_percentage() {
        let standing = goal_progress(&goal(1, i64::MAX));
        assert!(standing.percent_saved() > 0);
        assert_eq!(standing.bar_percent(), 100);
    }
}
