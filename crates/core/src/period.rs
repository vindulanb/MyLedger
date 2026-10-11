//! Turning a [`Period`] into the span of dates it means.
//!
//! Every total the apps show is "income in the selected period", so the exact
//! boundaries of that period decide the numbers. Working them out lives here,
//! in `core`, for two reasons: the arithmetic is fiddly enough to deserve
//! tests that run on the host, and the web and mobile apps must agree about
//! what "last 3 months" covers or the same ledger reads differently on each.
//!
//! Two decisions are baked in, both deliberate:
//!
//! 1. Periods are **rolling and include the current month**. On 2026-10-11,
//!    "Last 3 months" starts at 2026-08-01 — August, September and October so
//!    far — rather than covering the three *completed* months before this one.
//! 2. A period that reaches the present has **no end bound** at all, rather
//!    than ending today. Ending it today would hide a transaction dated later
//!    this month from every view except "All time", which is a quiet way to
//!    lose money that has already been recorded. "From 2026-08-01 onward" has
//!    no such gap.
//!
//! Dates are compared as `YYYY-MM-DD` strings, which sort chronologically, so
//! a range is a pair of string bounds and filtering needs no date parsing.

use crate::model::Period;

/// An inclusive span of dates, as `YYYY-MM-DD` bounds.
///
/// `None` on either side means unbounded: no start is "since records began",
/// and no end is "and anything after".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DateRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

impl DateRange {
    /// The range that excludes nothing.
    pub fn everything() -> Self {
        Self { start: None, end: None }
    }

    /// Whether a `YYYY-MM-DD` date falls inside this range.
    ///
    /// Comparison is lexicographic, which is chronological for this format —
    /// the reason [`validate_date`](crate::validate_date) insists on
    /// zero-padding.
    pub fn contains(&self, date: &str) -> bool {
        if let Some(start) = &self.start
            && date < start.as_str()
        {
            return false;
        }
        if let Some(end) = &self.end
            && date > end.as_str()
        {
            return false;
        }
        true
    }

    /// Whether this range admits every date.
    pub fn is_everything(&self) -> bool {
        self.start.is_none() && self.end.is_none()
    }
}

impl Period {
    /// The human label for a period selector.
    pub fn label(self) -> &'static str {
        match self {
            Period::AllTime => "All time",
            Period::ThisMonth => "This month",
            Period::LastMonth => "Last month",
            Period::LastThreeMonths => "Last 3 months",
            Period::LastSixMonths => "Last 6 months",
            Period::ThisYear => "This year",
        }
    }

    /// Every period, in the order a selector should offer them.
    pub fn all() -> [Period; 6] {
        [
            Period::AllTime,
            Period::ThisMonth,
            Period::LastMonth,
            Period::LastThreeMonths,
            Period::LastSixMonths,
            Period::ThisYear,
        ]
    }

    /// The dates this period covers, relative to `today` (`YYYY-MM-DD`).
    ///
    /// A malformed `today` yields [`DateRange::everything`] rather than an
    /// error: showing the whole ledger is the safe failure, where showing an
    /// arbitrary window would silently understate every total.
    pub fn range(self, today: &str) -> DateRange {
        let Some((year, month, _)) = split_date(today) else {
            return DateRange::everything();
        };

        match self {
            Period::AllTime => DateRange::everything(),

            // Open-ended on purpose: see the module comment.
            Period::ThisMonth => DateRange { start: Some(first_of(year, month)), end: None },
            Period::LastThreeMonths => {
                let (y, m) = months_before(year, month, 2);
                DateRange { start: Some(first_of(y, m)), end: None }
            }
            Period::LastSixMonths => {
                let (y, m) = months_before(year, month, 5);
                DateRange { start: Some(first_of(y, m)), end: None }
            }
            Period::ThisYear => {
                DateRange { start: Some(first_of(year, 1)), end: None }
            }

            // The one closed period: last month is over, so it has both ends.
            Period::LastMonth => {
                let (y, m) = months_before(year, month, 1);
                DateRange { start: Some(first_of(y, m)), end: Some(last_of(y, m)) }
            }
        }
    }
}

/// Splits a `YYYY-MM-DD` date, rejecting anything else.
///
/// Strict about width because the rest of the app relies on these strings
/// sorting chronologically.
pub(crate) fn split_date(date: &str) -> Option<(i32, u32, u32)> {
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = date[0..4].parse().ok()?;
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// `count` whole months earlier than the given year and month.
fn months_before(year: i32, month: u32, count: u32) -> (i32, u32) {
    // Work in months-since-year-zero so the borrow across January is plain
    // arithmetic rather than a branch.
    let absolute = year as i64 * 12 + (month as i64 - 1) - count as i64;
    ((absolute.div_euclid(12)) as i32, (absolute.rem_euclid(12)) as u32 + 1)
}

/// The first day of a month, as `YYYY-MM-DD`.
fn first_of(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}-01")
}

/// The last day of a month, as `YYYY-MM-DD`.
fn last_of(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}-{:02}", days_in_month(year, month))
}

/// How many days a month has, leap years included.
pub(crate) fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// The proleptic Gregorian leap-year rule.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "2026-10-11";

    #[test]
    fn all_time_excludes_nothing() {
        let range = Period::AllTime.range(TODAY);
        assert!(range.is_everything());
        assert!(range.contains("1999-01-01"));
        assert!(range.contains("2099-12-31"));
    }

    #[test]
    fn this_month_starts_at_the_first() {
        let range = Period::ThisMonth.range(TODAY);
        assert_eq!(range.start.as_deref(), Some("2026-10-01"));
        assert_eq!(range.end, None);
        assert!(range.contains("2026-10-01"));
        assert!(range.contains(TODAY));
        assert!(!range.contains("2026-09-30"));
    }

    #[test]
    fn a_date_later_this_month_is_still_in_this_month() {
        // The reason the range is open-ended: a transaction dated later this
        // month must not vanish from every view but "All time".
        let range = Period::ThisMonth.range(TODAY);
        assert!(range.contains("2026-10-31"));
    }

    #[test]
    fn last_month_is_closed_at_both_ends() {
        let range = Period::LastMonth.range(TODAY);
        assert_eq!(range.start.as_deref(), Some("2026-09-01"));
        assert_eq!(range.end.as_deref(), Some("2026-09-30"));
        assert!(range.contains("2026-09-30"));
        assert!(!range.contains("2026-10-01"));
        assert!(!range.contains("2026-08-31"));
    }

    #[test]
    fn last_three_months_includes_the_current_one() {
        // The decision the user made: rolling, inclusive of this month. Three
        // months on 2026-10-11 means August, September and October so far.
        let range = Period::LastThreeMonths.range(TODAY);
        assert_eq!(range.start.as_deref(), Some("2026-08-01"));
        assert!(range.contains("2026-08-01"));
        assert!(range.contains(TODAY));
        assert!(!range.contains("2026-07-31"));
    }

    #[test]
    fn last_six_months_reaches_back_five_whole_months() {
        let range = Period::LastSixMonths.range(TODAY);
        assert_eq!(range.start.as_deref(), Some("2026-05-01"));
        assert!(!range.contains("2026-04-30"));
    }

    #[test]
    fn this_year_starts_in_january() {
        let range = Period::ThisYear.range(TODAY);
        assert_eq!(range.start.as_deref(), Some("2026-01-01"));
        assert!(!range.contains("2025-12-31"));
    }

    #[test]
    fn a_period_spanning_new_year_borrows_correctly() {
        // February: three months back crosses into the previous year.
        let range = Period::LastThreeMonths.range("2026-02-05");
        assert_eq!(range.start.as_deref(), Some("2025-12-01"));

        let range = Period::LastSixMonths.range("2026-01-20");
        assert_eq!(range.start.as_deref(), Some("2025-08-01"));

        let range = Period::LastMonth.range("2026-01-20");
        assert_eq!(range.start.as_deref(), Some("2025-12-01"));
        assert_eq!(range.end.as_deref(), Some("2025-12-31"));
    }

    #[test]
    fn february_gets_the_right_number_of_days() {
        // Last month, viewed from March, has to know about leap years.
        assert_eq!(Period::LastMonth.range("2024-03-10").end.as_deref(), Some("2024-02-29"));
        assert_eq!(Period::LastMonth.range("2026-03-10").end.as_deref(), Some("2026-02-28"));
        assert_eq!(Period::LastMonth.range("2100-03-10").end.as_deref(), Some("2100-02-28"));
        assert_eq!(Period::LastMonth.range("2000-03-10").end.as_deref(), Some("2000-02-29"));
    }

    #[test]
    fn a_month_with_thirty_days_ends_on_the_thirtieth() {
        assert_eq!(Period::LastMonth.range("2026-05-02").end.as_deref(), Some("2026-04-30"));
    }

    #[test]
    fn an_unparseable_today_falls_back_to_everything() {
        // Showing the whole ledger is the safe failure; a guessed window would
        // understate every total without saying so.
        for bad in ["", "2026-3-4", "not-a-date", "2026-13-01", "20261011"] {
            assert!(
                Period::LastThreeMonths.range(bad).is_everything(),
                "{bad} should have fallen back"
            );
        }
    }

    #[test]
    fn every_period_has_a_label_and_they_are_distinct() {
        let labels: Vec<&str> = Period::all().iter().map(|p| p.label()).collect();
        assert_eq!(labels.len(), 6);
        for (index, label) in labels.iter().enumerate() {
            assert!(!label.is_empty());
            assert!(!labels[index + 1..].contains(label), "duplicate label {label}");
        }
    }

    #[test]
    fn the_selector_offers_all_time_first() {
        // The spec's default and first-ever value.
        assert_eq!(Period::all()[0], Period::AllTime);
    }
}
