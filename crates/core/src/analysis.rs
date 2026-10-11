//! The arithmetic behind the dashboard.
//!
//! Every number a summary card, a chart or an insight line shows is computed
//! here, as a pure function over a slice of transactions. That placement is
//! the point: the web app compiles only for `wasm32`, so a `#[test]` inside it
//! never runs on this machine. Money arithmetic that nobody can test is money
//! arithmetic nobody should trust, so it lives in `core` where `cargo test`
//! reaches it.
//!
//! Three rules hold throughout:
//!
//! - Amounts stay `i64` cents. Nothing here produces a float except a
//!   percentage, and a percentage is never summed.
//! - Soft-deleted transactions are excluded. Callers are usually handing over
//!   a list the server already filtered, but a function that quietly counts
//!   deleted money if handed the wrong slice is a trap.
//! - Nothing is invented. A comparison against a previous period is returned
//!   only when that period genuinely has data, so the UI has no way to render
//!   a trend arrow out of one data point.

use std::collections::HashMap;

use uuid::Uuid;

use crate::model::{Period, Transaction, TransactionKind};
use crate::period::{DateRange, days_in_month, split_date};

/// Income, spending and what that implies, over one span of dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PeriodSummary {
    pub income_cents: i64,
    pub spending_cents: i64,
    /// How many transactions went into these totals.
    pub counted: usize,
}

impl PeriodSummary {
    /// Income less spending. Negative when more went out than came in.
    pub fn net_cents(&self) -> i64 {
        self.income_cents - self.spending_cents
    }

    /// The share of income that wasn't spent, as a percentage.
    ///
    /// Zero when there was no income, rather than an error or a blank: the
    /// spec asks for `0%` with no division to go wrong. A period that spent
    /// more than it earned gives a negative rate, which is the truth and
    /// better than clamping it to zero.
    pub fn savings_rate_percent(&self) -> i64 {
        if self.income_cents <= 0 {
            return 0;
        }
        // Rounded to the nearest whole percent, computed in i128 so a very
        // large ledger cannot overflow the multiply.
        let numerator = self.net_cents() as i128 * 100;
        let denominator = self.income_cents as i128;
        round_div(numerator, denominator) as i64
    }

    /// Whether anything at all landed in this period.
    pub fn is_empty(&self) -> bool {
        self.counted == 0
    }
}

/// One month's income and expense, for the cash-flow chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthFlow {
    /// `YYYY-MM`.
    pub month: String,
    /// Short label for an axis, e.g. `Oct`.
    pub label: &'static str,
    pub income_cents: i64,
    pub expense_cents: i64,
}

/// One category's share of spending, for the donut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategorySlice {
    /// `None` for transactions filed under no category at all.
    pub category_id: Option<Uuid>,
    pub name: String,
    pub cents: i64,
    /// Share of total spending, 0–100, rounded to one decimal place and held
    /// as tenths of a percent so it stays integer arithmetic.
    pub tenths_of_percent: i64,
}

impl CategorySlice {
    /// The share as a display string, e.g. `34.2%`.
    pub fn percent_label(&self) -> String {
        format!("{}.{}%", self.tenths_of_percent / 10, self.tenths_of_percent % 10)
    }
}

/// Resolves a category or account ID to a name.
///
/// The same shape the recurring detector takes, so a caller already holding a
/// [`StateResponse`](crate::StateResponse) has nothing new to build.
pub trait Names {
    fn category_name(&self, id: Uuid) -> Option<&str>;
}

/// The whole ledger already answers this, so a caller holding one needs
/// nothing else.
impl Names for crate::wire::StateResponse {
    fn category_name(&self, id: Uuid) -> Option<&str> {
        crate::wire::StateResponse::category_name(self, id)
    }
}

/// The label used for spending filed under no category.
pub const UNCATEGORISED: &str = "Uncategorised";

/// The starter category that import paths fall back to, and which the
/// dashboard's insight line counts.
pub const NEEDS_REVIEW: &str = "Needs review";

/// The transactions in a range, newest first, excluding deleted ones.
pub fn in_range<'a>(
    transactions: &'a [Transaction],
    range: &'a DateRange,
) -> impl Iterator<Item = &'a Transaction> {
    transactions
        .iter()
        .filter(|t| t.deleted_at.is_none())
        .filter(move |t| range.contains(&t.date))
}

/// Totals income and spending over a range.
pub fn summarise(transactions: &[Transaction], range: &DateRange) -> PeriodSummary {
    let mut summary = PeriodSummary::default();
    for transaction in in_range(transactions, range) {
        // Saturating, so a ledger large enough to overflow renders a huge
        // number rather than panicking in a browser where nobody sees why.
        match transaction.kind {
            TransactionKind::Income => {
                summary.income_cents = summary.income_cents.saturating_add(transaction.amount_cents)
            }
            TransactionKind::Expense => {
                summary.spending_cents =
                    summary.spending_cents.saturating_add(transaction.amount_cents)
            }
        }
        summary.counted += 1;
    }
    summary
}

/// The period immediately before this one, for an honest comparison.
///
/// Returns `None` for [`Period::AllTime`], which has nothing before it, and
/// for an unparseable `today`. The caller must also check that the previous
/// period actually contains transactions before rendering a trend — this
/// function hands back a span, not a promise that it holds data.
pub fn previous_range(period: Period, today: &str) -> Option<DateRange> {
    let current = period.range(today);
    let start = current.start.as_deref()?;
    let (year, month, _) = split_date(start)?;

    let absolute = year as i64 * 12 + (month as i64 - 1);

    let (previous_start, previous_end) = match period {
        Period::AllTime => return None,

        // A rolling period compares with the same number of months
        // immediately before it: "last 3 months" against the 3 before those.
        Period::ThisMonth | Period::LastMonth => (absolute - 1, absolute - 1),
        Period::LastThreeMonths => (absolute - 3, absolute - 1),
        Period::LastSixMonths => (absolute - 6, absolute - 1),

        // Year to date is the exception. Shifting back by its own length
        // would compare January–October 2026 with March–December 2025, which
        // is neither the same months nor the same year. The honest comparison
        // is the same months one year earlier, so a part-year doesn't read as
        // a slump against a full one.
        Period::ThisYear => {
            let elapsed = month_of(today)? as i64;
            (absolute - 12, absolute - 12 + elapsed - 1)
        }
    };

    Some(DateRange {
        start: Some(first_day(previous_start)),
        end: Some(last_day(previous_end)),
    })
}

/// Monthly income and expense, oldest first, capped at `max_points`.
///
/// The months are taken from the range when it has a start, and from the data
/// when it does not — "All time" on a two-month-old ledger should show two
/// bars, not seven empty ones. Months with no activity inside the window are
/// kept, so a gap reads as a gap rather than closing up and misrepresenting
/// the shape of the year.
pub fn monthly_flow(
    transactions: &[Transaction],
    range: &DateRange,
    today: &str,
    max_points: usize,
) -> Vec<MonthFlow> {
    if max_points == 0 {
        return Vec::new();
    }

    let mut totals: HashMap<(i32, u32), (i64, i64)> = HashMap::new();
    let mut earliest: Option<i64> = None;
    let mut latest: Option<i64> = None;

    for transaction in in_range(transactions, range) {
        let Some((year, month, _)) = split_date(&transaction.date) else {
            continue;
        };
        let slot = totals.entry((year, month)).or_insert((0, 0));
        match transaction.kind {
            TransactionKind::Income => slot.0 = slot.0.saturating_add(transaction.amount_cents),
            TransactionKind::Expense => slot.1 = slot.1.saturating_add(transaction.amount_cents),
        }
        let absolute = year as i64 * 12 + (month as i64 - 1);
        earliest = Some(earliest.map_or(absolute, |e: i64| e.min(absolute)));
        latest = Some(latest.map_or(absolute, |l: i64| l.max(absolute)));
    }

    let (Some(first_seen), Some(last_seen)) = (earliest, latest) else {
        return Vec::new();
    };

    // The window ends at the later of the newest transaction and the current
    // month, so a month with no spending still appears as an empty bar instead
    // of the chart pretending the period ended early.
    let current = split_date(today).map(|(y, m, _)| y as i64 * 12 + (m as i64 - 1));
    let window_end = match current {
        Some(now) if range.end.is_none() => last_seen.max(now),
        _ => last_seen,
    };
    let window_start = first_seen.max(window_end - (max_points as i64 - 1));

    (window_start..=window_end)
        .map(|absolute| {
            let (year, month) = year_month(absolute);
            let (income, expense) = totals.get(&(year, month)).copied().unwrap_or((0, 0));
            MonthFlow {
                month: format!("{year:04}-{month:02}"),
                label: month_label(month),
                income_cents: income,
                expense_cents: expense,
            }
        })
        .collect()
}

/// Spending per category, largest first.
///
/// Only expenses count — a donut of "spending by category" that included
/// salary would be meaningless. Shares are computed against the total of the
/// slices returned, so they add to 100% (give or take one tenth of rounding).
pub fn spending_by_category(
    transactions: &[Transaction],
    range: &DateRange,
    names: &impl Names,
) -> Vec<CategorySlice> {
    let mut totals: HashMap<Option<Uuid>, i64> = HashMap::new();
    let mut total = 0i64;

    for transaction in in_range(transactions, range) {
        if transaction.kind != TransactionKind::Expense {
            continue;
        }
        let slot = totals.entry(transaction.category_id).or_insert(0);
        *slot = slot.saturating_add(transaction.amount_cents);
        total = total.saturating_add(transaction.amount_cents);
    }

    let mut slices: Vec<CategorySlice> = totals
        .into_iter()
        .map(|(category_id, cents)| CategorySlice {
            category_id,
            name: category_id
                .and_then(|id| names.category_name(id))
                .unwrap_or(UNCATEGORISED)
                .to_string(),
            cents,
            tenths_of_percent: if total > 0 {
                round_div(cents as i128 * 1000, total as i128) as i64
            } else {
                0
            },
        })
        .collect();

    // Largest first, then by name so that two equal slices keep a stable
    // order instead of shuffling between renders.
    slices.sort_by(|a, b| b.cents.cmp(&a.cents).then_with(|| a.name.cmp(&b.name)));
    slices
}

/// How many transactions in the range still need a human to look at them.
///
/// That means filed under no category, or under the `Needs review` category
/// the import paths fall back to. This is the only "insight" the dashboard
/// states, because it is a count of real rows rather than an opinion.
pub fn needs_review_count(
    transactions: &[Transaction],
    range: &DateRange,
    names: &impl Names,
) -> usize {
    in_range(transactions, range)
        .filter(|t| match t.category_id {
            None => true,
            Some(id) => names
                .category_name(id)
                .is_some_and(|name| name.eq_ignore_ascii_case(NEEDS_REVIEW)),
        })
        .count()
}

/// The `n` most recent transactions in the range, newest first.
///
/// Sorts rather than trusting the input order: the server returns newest
/// first, but a caller that has filtered or concatenated lists should still
/// get the right five.
pub fn most_recent<'a>(
    transactions: &'a [Transaction],
    range: &'a DateRange,
    n: usize,
) -> Vec<&'a Transaction> {
    let mut found: Vec<&Transaction> = in_range(transactions, range).collect();
    found.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.created_at.cmp(&a.created_at)));
    found.truncate(n);
    found
}

/// Divides, rounding half away from zero.
fn round_div(numerator: i128, denominator: i128) -> i128 {
    if denominator == 0 {
        return 0;
    }
    let half = denominator.abs() / 2;
    if (numerator >= 0) == (denominator >= 0) {
        (numerator + half) / denominator
    } else {
        (numerator - half) / denominator
    }
}

/// Splits a months-since-year-zero count back into a year and month.
fn year_month(absolute: i64) -> (i32, u32) {
    ((absolute.div_euclid(12)) as i32, (absolute.rem_euclid(12)) as u32 + 1)
}

fn first_day(absolute: i64) -> String {
    let (year, month) = year_month(absolute);
    format!("{year:04}-{month:02}-01")
}

fn last_day(absolute: i64) -> String {
    let (year, month) = year_month(absolute);
    format!("{year:04}-{month:02}-{:02}", days_in_month(year, month))
}

fn month_of(date: &str) -> Option<u32> {
    split_date(date).map(|(_, month, _)| month)
}

/// Three-letter month name for a chart axis.
fn month_label(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    NAMES.get((month as usize).saturating_sub(1)).copied().unwrap_or("—")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TransactionSource, now_timestamp};

    /// Name lookup backed by a plain list.
    struct Lookup(Vec<(Uuid, &'static str)>);

    impl Names for Lookup {
        fn category_name(&self, id: Uuid) -> Option<&str> {
            self.0.iter().find(|(known, _)| *known == id).map(|(_, name)| *name)
        }
    }

    fn transaction(date: &str, cents: i64, kind: TransactionKind) -> Transaction {
        Transaction {
            id: Uuid::new_v4(),
            date: date.to_string(),
            merchant: "Somewhere".to_string(),
            category_id: None,
            amount_cents: cents,
            kind,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Manual,
            fingerprint: format!("{date}|somewhere|{cents}|"),
            sms_hash: None,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
            deleted_at: None,
        }
    }

    fn spend(date: &str, cents: i64) -> Transaction {
        transaction(date, cents, TransactionKind::Expense)
    }

    fn earn(date: &str, cents: i64) -> Transaction {
        transaction(date, cents, TransactionKind::Income)
    }

    // ---- summaries ----------------------------------------------------

    #[test]
    fn an_empty_ledger_summarises_to_zero() {
        let summary = summarise(&[], &DateRange::everything());
        assert_eq!(summary, PeriodSummary::default());
        assert_eq!(summary.net_cents(), 0);
        assert!(summary.is_empty());
    }

    #[test]
    fn income_and_spending_are_totalled_separately() {
        let ledger = vec![
            earn("2026-10-01", 500_000),
            spend("2026-10-02", 12_550),
            spend("2026-10-03", 7_450),
        ];
        let summary = summarise(&ledger, &DateRange::everything());
        assert_eq!(summary.income_cents, 500_000);
        assert_eq!(summary.spending_cents, 20_000);
        assert_eq!(summary.net_cents(), 480_000);
        assert_eq!(summary.counted, 3);
    }

    #[test]
    fn the_range_decides_what_counts() {
        let ledger = vec![spend("2026-09-30", 1_000), spend("2026-10-01", 2_000)];
        let october = Period::ThisMonth.range("2026-10-11");
        assert_eq!(summarise(&ledger, &october).spending_cents, 2_000);
    }

    #[test]
    fn deleted_transactions_are_not_counted() {
        let mut ledger = vec![spend("2026-10-01", 5_000), spend("2026-10-02", 3_000)];
        ledger[1].deleted_at = Some(now_timestamp());
        let summary = summarise(&ledger, &DateRange::everything());
        assert_eq!(summary.spending_cents, 5_000);
        assert_eq!(summary.counted, 1);
    }

    // ---- savings rate -------------------------------------------------

    #[test]
    fn no_income_gives_a_zero_savings_rate_and_no_division_error() {
        // The spec is explicit about this case.
        let summary = summarise(&[spend("2026-10-01", 50_000)], &DateRange::everything());
        assert_eq!(summary.income_cents, 0);
        assert_eq!(summary.savings_rate_percent(), 0);
    }

    #[test]
    fn the_savings_rate_is_the_share_of_income_kept() {
        let ledger = vec![earn("2026-10-01", 100_000), spend("2026-10-02", 25_000)];
        let summary = summarise(&ledger, &DateRange::everything());
        assert_eq!(summary.savings_rate_percent(), 75);
    }

    #[test]
    fn spending_more_than_you_earn_gives_a_negative_rate() {
        // Reported honestly rather than clamped to zero.
        let ledger = vec![earn("2026-10-01", 100_000), spend("2026-10-02", 150_000)];
        assert_eq!(summarise(&ledger, &DateRange::everything()).savings_rate_percent(), -50);
    }

    #[test]
    fn the_savings_rate_rounds_to_the_nearest_whole_percent() {
        // 1000 earned, 333 spent: 66.7% kept.
        let ledger = vec![earn("2026-10-01", 1_000), spend("2026-10-02", 333)];
        assert_eq!(summarise(&ledger, &DateRange::everything()).savings_rate_percent(), 67);
    }

    #[test]
    fn enormous_amounts_do_not_overflow_the_rate() {
        let ledger = vec![earn("2026-10-01", i64::MAX), spend("2026-10-02", i64::MAX / 2)];
        let rate = summarise(&ledger, &DateRange::everything()).savings_rate_percent();
        assert_eq!(rate, 50);
    }

    // ---- monthly flow -------------------------------------------------

    #[test]
    fn an_empty_ledger_charts_nothing() {
        assert!(monthly_flow(&[], &DateRange::everything(), "2026-10-11", 7).is_empty());
    }

    #[test]
    fn each_month_gets_its_own_point() {
        let ledger = vec![
            earn("2026-08-05", 300_000),
            spend("2026-08-06", 100_000),
            spend("2026-09-10", 150_000),
        ];
        let flow = monthly_flow(&ledger, &DateRange::everything(), "2026-09-30", 7);
        assert_eq!(flow.len(), 2);
        assert_eq!(flow[0].month, "2026-08");
        assert_eq!(flow[0].label, "Aug");
        assert_eq!(flow[0].income_cents, 300_000);
        assert_eq!(flow[0].expense_cents, 100_000);
        assert_eq!(flow[1].month, "2026-09");
        assert_eq!(flow[1].income_cents, 0);
        assert_eq!(flow[1].expense_cents, 150_000);
    }

    #[test]
    fn the_chart_never_exceeds_the_point_cap() {
        // Two years of data, seven points asked for: the newest seven months.
        let ledger: Vec<Transaction> = (1..=12)
            .map(|month| spend(&format!("2026-{month:02}-05"), month as i64 * 1_000))
            .collect();
        let flow = monthly_flow(&ledger, &DateRange::everything(), "2026-12-20", 7);
        assert_eq!(flow.len(), 7);
        assert_eq!(flow.first().unwrap().month, "2026-06");
        assert_eq!(flow.last().unwrap().month, "2026-12");
    }

    #[test]
    fn a_quiet_month_inside_the_window_is_kept_as_a_gap() {
        // Closing the gap would make an eight-month span look like two
        // consecutive months.
        let ledger = vec![spend("2026-05-01", 1_000), spend("2026-07-01", 2_000)];
        let flow = monthly_flow(&ledger, &DateRange::everything(), "2026-07-15", 7);
        assert_eq!(flow.len(), 3);
        assert_eq!(flow[1].month, "2026-06");
        assert_eq!(flow[1].income_cents, 0);
        assert_eq!(flow[1].expense_cents, 0);
    }

    #[test]
    fn an_open_ended_period_runs_up_to_the_current_month() {
        // Nothing spent this month, but the chart should still show it rather
        // than ending in September and implying the period is over.
        let ledger = vec![spend("2026-09-10", 5_000)];
        let flow = monthly_flow(&ledger, &DateRange::everything(), "2026-10-11", 7);
        assert_eq!(flow.last().unwrap().month, "2026-10");
        assert_eq!(flow.last().unwrap().expense_cents, 0);
    }

    #[test]
    fn a_closed_period_stops_where_it_ends() {
        let range = Period::LastMonth.range("2026-10-11");
        let ledger = vec![spend("2026-09-10", 5_000), spend("2026-10-02", 9_000)];
        let flow = monthly_flow(&ledger, &range, "2026-10-11", 7);
        assert_eq!(flow.len(), 1);
        assert_eq!(flow[0].month, "2026-09");
    }

    #[test]
    fn a_chart_with_no_room_for_points_is_empty_rather_than_panicking() {
        let ledger = vec![spend("2026-09-10", 5_000)];
        assert!(monthly_flow(&ledger, &DateRange::everything(), "2026-10-11", 0).is_empty());
    }

    #[test]
    fn a_window_spanning_a_year_boundary_labels_months_correctly() {
        let ledger = vec![spend("2025-12-01", 1_000), spend("2026-01-01", 2_000)];
        let flow = monthly_flow(&ledger, &DateRange::everything(), "2026-01-15", 7);
        assert_eq!(flow[0].month, "2025-12");
        assert_eq!(flow[0].label, "Dec");
        assert_eq!(flow[1].month, "2026-01");
        assert_eq!(flow[1].label, "Jan");
    }

    // ---- category breakdown -------------------------------------------

    #[test]
    fn spending_splits_by_category_largest_first() {
        let groceries = Uuid::new_v4();
        let dining = Uuid::new_v4();
        let names = Lookup(vec![(groceries, "Groceries"), (dining, "Dining")]);

        let mut ledger = vec![spend("2026-10-01", 3_000), spend("2026-10-02", 7_000)];
        ledger[0].category_id = Some(dining);
        ledger[1].category_id = Some(groceries);

        let slices = spending_by_category(&ledger, &DateRange::everything(), &names);
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].name, "Groceries");
        assert_eq!(slices[0].cents, 7_000);
        assert_eq!(slices[0].tenths_of_percent, 700);
        assert_eq!(slices[0].percent_label(), "70.0%");
        assert_eq!(slices[1].name, "Dining");
        assert_eq!(slices[1].percent_label(), "30.0%");
    }

    #[test]
    fn income_is_left_out_of_spending_by_category() {
        let salary = Uuid::new_v4();
        let names = Lookup(vec![(salary, "Income")]);
        let mut ledger = vec![earn("2026-10-01", 500_000), spend("2026-10-02", 1_000)];
        ledger[0].category_id = Some(salary);

        let slices = spending_by_category(&ledger, &DateRange::everything(), &names);
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].name, UNCATEGORISED);
        assert_eq!(slices[0].cents, 1_000);
    }

    #[test]
    fn transactions_with_no_category_are_grouped_and_named() {
        let slices = spending_by_category(
            &[spend("2026-10-01", 1_000), spend("2026-10-02", 2_000)],
            &DateRange::everything(),
            &Lookup(vec![]),
        );
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].name, UNCATEGORISED);
        assert_eq!(slices[0].cents, 3_000);
        assert_eq!(slices[0].percent_label(), "100.0%");
    }

    #[test]
    fn a_category_deleted_since_still_resolves_to_its_name() {
        // `StateResponse::category_name` looks through deleted rows for
        // exactly this reason.
        let gone = Uuid::new_v4();
        let names = Lookup(vec![(gone, "Old category")]);
        let mut ledger = vec![spend("2026-10-01", 1_000)];
        ledger[0].category_id = Some(gone);
        let slices = spending_by_category(&ledger, &DateRange::everything(), &names);
        assert_eq!(slices[0].name, "Old category");
    }

    #[test]
    fn shares_add_up_to_about_a_hundred_percent() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let names = Lookup(vec![(a, "A"), (b, "B"), (c, "C")]);
        let mut ledger =
            vec![spend("2026-10-01", 333), spend("2026-10-02", 333), spend("2026-10-03", 334)];
        ledger[0].category_id = Some(a);
        ledger[1].category_id = Some(b);
        ledger[2].category_id = Some(c);

        let total: i64 = spending_by_category(&ledger, &DateRange::everything(), &names)
            .iter()
            .map(|s| s.tenths_of_percent)
            .sum();
        assert!((999..=1001).contains(&total), "shares summed to {total} tenths");
    }

    #[test]
    fn nothing_spent_means_no_slices() {
        assert!(
            spending_by_category(
                &[earn("2026-10-01", 1_000)],
                &DateRange::everything(),
                &Lookup(vec![])
            )
            .is_empty()
        );
    }

    // ---- needs review --------------------------------------------------

    #[test]
    fn uncategorised_and_needs_review_both_count_as_needing_attention() {
        let review = Uuid::new_v4();
        let groceries = Uuid::new_v4();
        let names = Lookup(vec![(review, "Needs review"), (groceries, "Groceries")]);

        let mut ledger = vec![
            spend("2026-10-01", 1_000),
            spend("2026-10-02", 1_000),
            spend("2026-10-03", 1_000),
        ];
        ledger[1].category_id = Some(review);
        ledger[2].category_id = Some(groceries);

        assert_eq!(needs_review_count(&ledger, &DateRange::everything(), &names), 2);
    }

    #[test]
    fn a_fully_categorised_ledger_needs_no_review() {
        let groceries = Uuid::new_v4();
        let names = Lookup(vec![(groceries, "Groceries")]);
        let mut ledger = vec![spend("2026-10-01", 1_000)];
        ledger[0].category_id = Some(groceries);
        assert_eq!(needs_review_count(&ledger, &DateRange::everything(), &names), 0);
    }

    // ---- recent --------------------------------------------------------

    #[test]
    fn the_most_recent_are_returned_newest_first_and_capped() {
        let ledger = vec![
            spend("2026-10-01", 100),
            spend("2026-10-05", 200),
            spend("2026-10-03", 300),
            spend("2026-09-28", 400),
        ];
        let everything = DateRange::everything();
        let recent = most_recent(&ledger, &everything, 2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].date, "2026-10-05");
        assert_eq!(recent[1].date, "2026-10-03");
    }

    #[test]
    fn asking_for_more_than_exists_returns_what_there_is() {
        let ledger = [spend("2026-10-01", 100)];
        let everything = DateRange::everything();
        assert_eq!(most_recent(&ledger, &everything, 5).len(), 1);
    }

    #[test]
    fn recent_respects_the_period() {
        let ledger = vec![spend("2026-10-05", 100), spend("2026-08-05", 200)];
        let this_month = Period::ThisMonth.range("2026-10-11");
        let recent = most_recent(&ledger, &this_month, 5);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].date, "2026-10-05");
    }

    // ---- previous period -----------------------------------------------

    #[test]
    fn all_time_has_no_previous_period() {
        // Nothing precedes everything, so there is no comparison to draw and
        // the dashboard must not draw one.
        assert!(previous_range(Period::AllTime, "2026-10-11").is_none());
    }

    #[test]
    fn the_month_before_this_one() {
        let range = previous_range(Period::ThisMonth, "2026-10-11").unwrap();
        assert_eq!(range.start.as_deref(), Some("2026-09-01"));
        assert_eq!(range.end.as_deref(), Some("2026-09-30"));
    }

    #[test]
    fn the_three_months_before_the_last_three() {
        let range = previous_range(Period::LastThreeMonths, "2026-10-11").unwrap();
        assert_eq!(range.start.as_deref(), Some("2026-05-01"));
        assert_eq!(range.end.as_deref(), Some("2026-07-31"));
    }

    #[test]
    fn the_period_before_last_month_is_the_month_before_it() {
        let range = previous_range(Period::LastMonth, "2026-10-11").unwrap();
        assert_eq!(range.start.as_deref(), Some("2026-08-01"));
        assert_eq!(range.end.as_deref(), Some("2026-08-31"));
    }

    #[test]
    fn this_year_compares_with_the_same_months_last_year() {
        // January to October 2026 compares with January to October 2025, not
        // with all of 2025 — otherwise a part-year always looks like a slump.
        let range = previous_range(Period::ThisYear, "2026-10-11").unwrap();
        assert_eq!(range.start.as_deref(), Some("2025-01-01"));
        assert_eq!(range.end.as_deref(), Some("2025-10-31"));
    }

    #[test]
    fn a_previous_period_can_be_empty_and_that_is_the_callers_signal() {
        // The function hands back a span; whether it holds anything is what
        // decides if a trend may be shown.
        let range = previous_range(Period::ThisMonth, "2026-10-11").unwrap();
        let summary = summarise(&[spend("2026-10-05", 1_000)], &range);
        assert!(summary.is_empty());
    }
}
