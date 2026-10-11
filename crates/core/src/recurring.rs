//! Recurring bill and subscription detection.
//!
//! Given the transaction history, this module works out which merchants are
//! charging on a schedule. It is deliberately conservative: a false suggestion
//! costs the user attention every time they open the app, so a pattern has to
//! look genuinely periodic and genuinely stable in price before it is proposed.
//!
//! Everything here is pure. Detection *proposes*; it never writes, never
//! mutates, and never promotes a pattern into a confirmed
//! [`RecurringItem`](crate::RecurringItem). Only the user does that, later, in
//! the UI. Even "today" arrives as a parameter rather than being read from the
//! clock, so the same input always produces the same output.
//!
//! The pipeline, per merchant:
//!
//! 1. [`normalize_merchant`] collapses the many spellings a bank uses for one
//!    merchant into a single grouping key.
//! 2. The gaps between charges are reduced to one [`dominant_interval`] and
//!    classified by [`classify_interval`] into a [`Cadence`].
//! 3. [`amount_variation`] measures how steady the price is, against a
//!    threshold that depends on what kind of charge this looks like.
//! 4. Merchants that match no hint list at all face a much higher bar, so that
//!    ordinary weekly grocery runs are never mistaken for subscriptions.

use std::cmp::Reverse;
use std::collections::HashMap;

use chrono::{Days, Months, NaiveDate};
use uuid::Uuid;

use crate::model::{Cadence, Category, RecurringKind, Tag, Transaction, TransactionKind, token_enum};
use crate::validate::validate_date;

token_enum! {
    /// How much trust to place in a suggestion.
    Confidence as "confidence" {
        /// Enough occurrences, steady price, steady schedule.
        High => "high",
        /// Consistent with a repeating charge, but on thinner evidence.
        Likely => "likely",
    }
}

/// Merchants whose name alone marks a charge as a subscription.
///
/// Matched as whole words against the normalised merchant.
pub const SUBSCRIPTION_MERCHANT_HINTS: &[&str] = &[
    "netflix",
    "spotify",
    "hulu",
    "disney",
    "youtube",
    "icloud",
    "dropbox",
    "adobe",
    "microsoft",
    "amazon prime",
    "patreon",
    "membership",
    "studio",
    "gym",
    "openai",
    "chatgpt",
    "canva",
    "notion",
    "zoom",
    "slack",
    "github",
];

/// Merchants whose name alone marks a charge as a recurring bill.
pub const RECURRING_MERCHANT_HINTS: &[&str] = &[
    "mortgage",
    "rent",
    "loan",
    "insurance",
    "utility",
    "utilities",
    "electric",
    "water",
    "internet",
    "phone",
    "mobile",
    "daycare",
    "tuition",
    "lease",
    "car payment",
    "auto payment",
    "hoa",
    "property tax",
];

/// A subscription's price may swing this far before the candidate is dropped.
const SUBSCRIPTION_MAX_VARIATION: f64 = 0.20;
/// Bills are allowed to move more — a utility bill legitimately varies by season.
const RECURRING_MAX_VARIATION: f64 = 0.35;
/// A merchant matching no hint list must be near-identical every single time.
const NO_HINT_MAX_VARIATION: f64 = 0.03;
/// …and must have done it at least this many times.
const NO_HINT_MIN_OCCURRENCES: usize = 3;

/// Thresholds separating [`Confidence::High`] from [`Confidence::Likely`].
const HIGH_CONFIDENCE_MIN_OCCURRENCES: usize = 3;
const HIGH_CONFIDENCE_MAX_VARIATION: f64 = 0.12;
const HIGH_CONFIDENCE_MAX_JITTER_DAYS: i64 = 5;

/// A pattern is treated as ended once this many periods have passed since the
/// last charge — one expected, two overdue. A monthly charge unseen for three
/// months has almost certainly been cancelled, and suggesting it is noise.
const STALE_AFTER_PERIODS: u32 = 3;

/// A repeating charge that detection believes exists, offered to the user for
/// confirmation.
///
/// This is *not* a [`RecurringItem`](crate::RecurringItem). Nothing is stored
/// until the user accepts it.
#[derive(Debug, Clone, PartialEq, Eq, ::serde::Serialize, ::serde::Deserialize)]
pub struct RecurringSuggestion {
    /// Stable identity of the pattern: the normalised merchant.
    ///
    /// Dismissing a suggestion records this key, so the merchant stays hidden
    /// even if its cadence or price later changes.
    pub pattern_key: String,
    /// The merchant as the user last saw it, original casing intact.
    pub merchant: String,
    /// The category most of these transactions were filed under, if any.
    pub category_id: Option<Uuid>,
    /// That category's name, resolved for display.
    pub category_name: Option<String>,
    /// The account most of these charges came out of, if any. Carried so that
    /// accepting a suggestion produces a complete bill rather than one the user
    /// has to finish filling in.
    pub account_id: Option<Uuid>,
    pub cadence: Cadence,
    /// Which hint list matched, and therefore how the charge is presented.
    pub kind: RecurringKind,
    /// Number of distinct days this merchant charged on.
    pub occurrences: usize,
    pub confidence: Confidence,
    /// Mean charge, in cents.
    pub average_amount_cents: i64,
    /// What this costs per month once the cadence is levelled out, in cents.
    pub monthly_equivalent_cents: i64,
    /// Most recent charge, as `YYYY-MM-DD`.
    pub last_seen: String,
    /// One cadence period after [`last_seen`](Self::last_seen), as `YYYY-MM-DD`.
    pub next_expected_date: String,
}

/// Resolves the category and tag IDs on a transaction to their names.
///
/// Detection needs the names — a category called "Subscriptions" is itself a
/// hint — but a [`Transaction`] only carries IDs, so the caller supplies the
/// mapping.
#[derive(Debug, Clone, Default)]
pub struct NameLookup {
    categories: HashMap<Uuid, String>,
    tags: HashMap<Uuid, String>,
}

impl NameLookup {
    /// Builds a lookup from the category and tag records already loaded.
    pub fn from_records(categories: &[Category], tags: &[Tag]) -> Self {
        Self {
            categories: categories.iter().map(|c| (c.id, c.name.clone())).collect(),
            tags: tags.iter().map(|t| (t.id, t.name.clone())).collect(),
        }
    }

    /// Builds a lookup from bare `(id, name)` pairs.
    pub fn from_pairs(
        categories: impl IntoIterator<Item = (Uuid, String)>,
        tags: impl IntoIterator<Item = (Uuid, String)>,
    ) -> Self {
        Self {
            categories: categories.into_iter().collect(),
            tags: tags.into_iter().collect(),
        }
    }

    /// The name of a category, if it is known.
    pub fn category(&self, id: Uuid) -> Option<&str> {
        self.categories.get(&id).map(String::as_str)
    }

    /// The name of a tag, if it is known.
    pub fn tag(&self, id: Uuid) -> Option<&str> {
        self.tags.get(&id).map(String::as_str)
    }
}

/// Reduces a merchant string to a stable grouping key.
///
/// Banks spell the same merchant a dozen ways — `STARBUCKS #1234`,
/// `Starbucks  Store 4471`, `STARBUCKS.COM`. All of them need to land in one
/// group, so the noise that varies per transaction is stripped out:
///
/// 1. lowercase and trim;
/// 2. drop a trailing `#<digits>` store number — done first, while the `#` is
///    still there to identify it;
/// 3. drop apostrophes outright, so `McDonald's` becomes one word;
/// 4. turn every other punctuation mark into a space;
/// 5. drop runs of four or more digits, which are reference numbers rather
///    than part of a name — short numbers like `7-Eleven` or `Store 24`
///    survive, because they identify the merchant;
/// 6. collapse the remaining whitespace.
///
/// The result is only ever used for grouping and hint matching. The merchant
/// the user sees is kept separately and untouched.
///
/// ```
/// # use myledger_core::recurring::normalize_merchant;
/// assert_eq!(normalize_merchant("STARBUCKS #1234"), "starbucks");
/// assert_eq!(normalize_merchant("SQ *Coffee Bar 998877"), "sq coffee bar");
/// assert_eq!(normalize_merchant("McDonald's"), "mcdonalds");
/// ```
pub fn normalize_merchant(merchant: &str) -> String {
    let mut working = merchant.trim().to_lowercase();

    // Strip trailing store numbers, repeatedly: "cafe #12 #34" has two.
    while let Some(hash) = working.rfind('#') {
        let digits = working[hash + 1..].trim_end();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        working.truncate(hash);
        working.truncate(working.trim_end().len());
    }

    let depunctuated: String = working
        .chars()
        .filter(|c| !matches!(c, '\'' | '\u{2019}'))
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();

    let mut without_reference_numbers = String::with_capacity(depunctuated.len());
    let mut digit_run = String::new();
    for c in depunctuated.chars() {
        if c.is_ascii_digit() {
            digit_run.push(c);
            continue;
        }
        flush_digit_run(&mut digit_run, &mut without_reference_numbers);
        without_reference_numbers.push(c);
    }
    flush_digit_run(&mut digit_run, &mut without_reference_numbers);

    without_reference_numbers
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Appends a buffered run of digits unless it is long enough to be a reference
/// number, in which case it is dropped.
fn flush_digit_run(run: &mut String, out: &mut String) {
    if run.is_empty() {
        return;
    }
    if run.len() < 4 {
        out.push_str(run);
    } else {
        out.push(' ');
    }
    run.clear();
}

/// Reports whether `phrase` appears in `normalized` as a run of whole words.
///
/// Word matching rather than plain substring matching, because several hints
/// are short enough to be dangerous otherwise: `rent` hides inside "parent",
/// and `hoa` inside "whoa".
fn contains_phrase(normalized: &str, phrase: &str) -> bool {
    let haystack: Vec<&str> = normalized.split(' ').collect();
    let needle: Vec<&str> = phrase.split(' ').collect();
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| window == needle.as_slice())
}

/// Maps a gap in days onto a cadence, or `None` if it falls between the
/// recognised rhythms.
///
/// The windows are wide enough to absorb weekends and month lengths, and the
/// deliberate gaps between them are what rejects a merchant that is simply
/// visited often rather than billed on a schedule.
pub fn classify_interval(days: i64) -> Option<Cadence> {
    match days {
        5..=9 => Some(Cadence::Weekly),
        12..=17 => Some(Cadence::Biweekly),
        24..=40 => Some(Cadence::Monthly),
        75..=110 => Some(Cadence::Quarterly),
        330..=400 => Some(Cadence::Annual),
        _ => None,
    }
}

/// The gaps in days between consecutive dates. Expects `dates` sorted ascending.
fn intervals(dates: &[NaiveDate]) -> Vec<i64> {
    dates.windows(2).map(|pair| (pair[1] - pair[0]).num_days()).collect()
}

/// The representative gap between charges: the median.
///
/// The median rather than the mean, so that one skipped or doubled-up month
/// can't drag an otherwise regular pattern out of its cadence window. Returns
/// `None` for fewer than two dates, where there is no gap to measure.
pub fn dominant_interval(dates: &[NaiveDate]) -> Option<i64> {
    let mut gaps = intervals(dates);
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();

    let middle = gaps.len() / 2;
    Some(if gaps.len() % 2 == 1 {
        gaps[middle]
    } else {
        // Round the two-sided median up, so 28 and 31 days give 30 rather
        // than 29 — the answer a person would give for "about a month".
        (gaps[middle - 1] + gaps[middle] + 1) / 2
    })
}

/// How much the charge amount moves, as a fraction of the average.
///
/// Measured as full spread — `(max - min) / mean` — so a single unusual charge
/// is enough to disqualify a candidate. Returns `0.0` for an empty slice or a
/// zero mean, both of which mean there is nothing to judge.
pub fn amount_variation(amounts: &[i64]) -> f64 {
    if amounts.is_empty() {
        return 0.0;
    }
    let (mut min, mut max, mut sum) = (i64::MAX, i64::MIN, 0i128);
    for &amount in amounts {
        min = min.min(amount);
        max = max.max(amount);
        sum += i128::from(amount);
    }
    let mean = sum as f64 / amounts.len() as f64;
    if mean == 0.0 {
        return 0.0;
    }
    (max - min) as f64 / mean.abs()
}

/// Moves a date forward by whole cadence periods, respecting the calendar.
///
/// Month-based cadences keep the day of the month where the target month has
/// one: the 15th stays the 15th. Where it doesn't — the 31st of a 30-day month
/// — the date clamps to the last day rather than spilling into the next month.
///
/// Returns `None` only if the result would fall outside the representable date
/// range.
pub fn advance_by_cadence(date: NaiveDate, cadence: Cadence, periods: u32) -> Option<NaiveDate> {
    match cadence {
        Cadence::Weekly => date.checked_add_days(Days::new(u64::from(periods) * 7)),
        Cadence::Biweekly => date.checked_add_days(Days::new(u64::from(periods) * 14)),
        Cadence::Monthly => date.checked_add_months(Months::new(periods)),
        Cadence::Quarterly => date.checked_add_months(Months::new(periods.checked_mul(3)?)),
        Cadence::Annual => date.checked_add_months(Months::new(periods.checked_mul(12)?)),
    }
}

/// When the charge after `last` is expected.
pub fn next_expected_date(last: NaiveDate, cadence: Cadence) -> Option<NaiveDate> {
    advance_by_cadence(last, cadence, 1)
}

/// What a charge at this cadence costs per month, in cents.
///
/// Weekly and biweekly charges are annualised before being divided down, since
/// there are not exactly four weeks in a month. Results round to the nearest
/// cent.
pub fn monthly_equivalent_cents(amount_cents: i64, cadence: Cadence) -> i64 {
    match cadence {
        Cadence::Weekly => scale_rounded(amount_cents, 52, 12),
        Cadence::Biweekly => scale_rounded(amount_cents, 26, 12),
        Cadence::Monthly => amount_cents,
        Cadence::Quarterly => scale_rounded(amount_cents, 1, 3),
        Cadence::Annual => scale_rounded(amount_cents, 1, 12),
    }
}

/// `amount * numerator / denominator`, rounded to nearest, computed wide enough
/// that the intermediate product can't overflow.
fn scale_rounded(amount_cents: i64, numerator: i64, denominator: i64) -> i64 {
    let scaled = i128::from(amount_cents) * i128::from(numerator);
    let denominator = i128::from(denominator);
    let rounded = if scaled >= 0 {
        (scaled * 2 + denominator) / (denominator * 2)
    } else {
        (scaled * 2 - denominator) / (denominator * 2)
    };
    rounded.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// Which hint list, if any, a candidate matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hint {
    Subscription,
    RecurringBill,
    None,
}

impl Hint {
    /// How a matched candidate should be filed. An unhinted pattern that
    /// survives the stricter bar is recorded as an ordinary recurring charge.
    fn kind(self) -> RecurringKind {
        match self {
            Hint::Subscription => RecurringKind::Subscription,
            Hint::RecurringBill | Hint::None => RecurringKind::Recurring,
        }
    }
}

/// Classifies a candidate from its merchant and the labels attached to it.
///
/// A subscription hint wins over a bill hint: "gym membership insurance" is a
/// subscription that happens to mention insurance, not the other way round.
fn classify_hint(normalized_merchant: &str, labels: &[&str]) -> Hint {
    let labelled_subscription = labels
        .iter()
        .any(|label| label.to_lowercase().contains("subscription"));
    let named_subscription = SUBSCRIPTION_MERCHANT_HINTS
        .iter()
        .any(|hint| contains_phrase(normalized_merchant, hint));
    if labelled_subscription || named_subscription {
        return Hint::Subscription;
    }

    if RECURRING_MERCHANT_HINTS
        .iter()
        .any(|hint| contains_phrase(normalized_merchant, hint))
    {
        return Hint::RecurringBill;
    }

    Hint::None
}

/// Finds the repeating charges hiding in a transaction history.
///
/// Only live expenses are considered — income, soft-deleted rows and anything
/// that fails to parse are skipped. Results come back with the most expensive
/// per month first, since that is the order a user cares about.
///
/// `today` anchors the staleness check and is a parameter so that detection
/// stays deterministic.
pub fn detect_recurring(
    transactions: &[Transaction],
    names: &NameLookup,
    today: NaiveDate,
) -> Vec<RecurringSuggestion> {
    let mut groups: HashMap<String, Vec<(NaiveDate, &Transaction)>> = HashMap::new();

    for transaction in transactions {
        if transaction.kind != TransactionKind::Expense
            || transaction.deleted_at.is_some()
            || transaction.amount_cents <= 0
        {
            continue;
        }
        let Ok(date) = validate_date(&transaction.date) else { continue };

        let key = normalize_merchant(&transaction.merchant);
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push((date, transaction));
    }

    let mut suggestions: Vec<RecurringSuggestion> = groups
        .into_iter()
        .filter_map(|(key, occurrences)| suggest(key, occurrences, names, today))
        .collect();

    suggestions.sort_by(|a, b| {
        b.monthly_equivalent_cents
            .cmp(&a.monthly_equivalent_cents)
            .then_with(|| a.merchant.cmp(&b.merchant))
    });
    suggestions
}

/// Decides whether one merchant's charges amount to a suggestion.
fn suggest(
    pattern_key: String,
    mut occurrences: Vec<(NaiveDate, &Transaction)>,
    names: &NameLookup,
    today: NaiveDate,
) -> Option<RecurringSuggestion> {
    occurrences.sort_by_key(|(date, _)| *date);

    // Two charges on one day are one occurrence: the schedule is what's being
    // measured, and a same-day pair says nothing about it.
    let mut distinct_dates: Vec<NaiveDate> = occurrences.iter().map(|(date, _)| *date).collect();
    distinct_dates.dedup();
    if distinct_dates.len() < 2 {
        return None;
    }

    let dominant = dominant_interval(&distinct_dates)?;
    let cadence = classify_interval(dominant)?;

    let amounts: Vec<i64> = occurrences.iter().map(|(_, t)| t.amount_cents).collect();
    let variation = amount_variation(&amounts);

    let labels = collect_labels(&occurrences, names);
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let hint = classify_hint(&pattern_key, &label_refs);

    let variation_ceiling = match hint {
        Hint::Subscription => SUBSCRIPTION_MAX_VARIATION,
        Hint::RecurringBill | Hint::None => RECURRING_MAX_VARIATION,
    };
    if variation > variation_ceiling {
        return None;
    }

    let occurrence_count = distinct_dates.len();

    // A merchant nobody named is guilty until proven innocent. Weekly grocery
    // runs repeat perfectly well; that is not a reason to call them bills.
    if hint == Hint::None {
        let plausible_cadence =
            matches!(cadence, Cadence::Monthly | Cadence::Quarterly | Cadence::Annual);
        if !plausible_cadence
            || occurrence_count < NO_HINT_MIN_OCCURRENCES
            || variation > NO_HINT_MAX_VARIATION
        {
            return None;
        }
    }

    let last_seen = *distinct_dates.last()?;
    let next = next_expected_date(last_seen, cadence)?;

    // Once the charge is this overdue it has stopped, and suggesting a
    // cancelled service is just noise.
    if let Some(gave_up_at) = advance_by_cadence(last_seen, cadence, STALE_AFTER_PERIODS)
        && today >= gave_up_at
    {
        return None;
    }

    let jitter = intervals(&distinct_dates)
        .into_iter()
        .map(|gap| (gap - dominant).abs())
        .max()
        .unwrap_or(0);
    let confidence = if occurrence_count >= HIGH_CONFIDENCE_MIN_OCCURRENCES
        && variation <= HIGH_CONFIDENCE_MAX_VARIATION
        && jitter <= HIGH_CONFIDENCE_MAX_JITTER_DAYS
    {
        Confidence::High
    } else {
        Confidence::Likely
    };

    let average_amount_cents = mean_cents(&amounts);
    let category_id = dominant_value(&occurrences, |transaction| transaction.category_id);

    Some(RecurringSuggestion {
        pattern_key,
        merchant: occurrences.last()?.1.merchant.trim().to_string(),
        category_name: category_id
            .and_then(|id| names.category(id))
            .map(str::to_string),
        category_id,
        account_id: dominant_value(&occurrences, |transaction| transaction.account_id),
        cadence,
        kind: hint.kind(),
        occurrences: occurrence_count,
        confidence,
        average_amount_cents,
        monthly_equivalent_cents: monthly_equivalent_cents(average_amount_cents, cadence),
        last_seen: last_seen.format("%Y-%m-%d").to_string(),
        next_expected_date: next.format("%Y-%m-%d").to_string(),
    })
}

/// The category and tag names attached to any transaction in the group.
fn collect_labels(occurrences: &[(NaiveDate, &Transaction)], names: &NameLookup) -> Vec<String> {
    let mut labels = Vec::new();
    for (_, transaction) in occurrences {
        if let Some(name) = transaction.category_id.and_then(|id| names.category(id)) {
            labels.push(name.to_string());
        }
        labels.extend(
            transaction
                .tags
                .iter()
                .filter_map(|id| names.tag(*id))
                .map(str::to_string),
        );
    }
    labels
}

/// The value most of the group's transactions carry, breaking ties towards the
/// most recent.
///
/// Used for both the category and the account: a merchant filed under
/// "Subscriptions" eleven times and miscategorised once is a subscription.
fn dominant_value<F>(occurrences: &[(NaiveDate, &Transaction)], pick: F) -> Option<Uuid>
where
    F: Fn(&Transaction) -> Option<Uuid>,
{
    let mut counts: HashMap<Uuid, usize> = HashMap::new();
    let mut recency: HashMap<Uuid, usize> = HashMap::new();

    for (rank, (_, transaction)) in occurrences.iter().rev().enumerate() {
        if let Some(id) = pick(transaction) {
            *counts.entry(id).or_default() += 1;
            recency.entry(id).or_insert(rank);
        }
    }

    counts
        .iter()
        .max_by_key(|(id, count)| (**count, Reverse(recency[*id])))
        .map(|(id, _)| *id)
}

/// The mean of a set of amounts, rounded to the nearest cent.
fn mean_cents(amounts: &[i64]) -> i64 {
    if amounts.is_empty() {
        return 0;
    }
    let sum: i128 = amounts.iter().map(|a| i128::from(*a)).sum();
    let count = amounts.len() as i128;
    let rounded = if sum >= 0 {
        (sum * 2 + count) / (count * 2)
    } else {
        (sum * 2 - count) / (count * 2)
    };
    rounded.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TransactionSource, now_timestamp};
    use crate::transaction_fingerprint;

    fn on(date: &str) -> NaiveDate {
        validate_date(date).expect("test date should be valid")
    }

    /// An expense, with everything not under test held constant.
    fn expense(date: &str, merchant: &str, amount_cents: i64) -> Transaction {
        let now = now_timestamp();
        Transaction {
            id: Uuid::new_v4(),
            date: date.to_string(),
            merchant: merchant.to_string(),
            category_id: None,
            amount_cents,
            kind: TransactionKind::Expense,
            account_id: None,
            tags: Vec::new(),
            receipt: false,
            source: TransactionSource::Manual,
            fingerprint: transaction_fingerprint(date, merchant, amount_cents, "Main Account"),
            sms_hash: None,
            created_at: now.clone(),
            updated_at: now,
            deleted_at: None,
        }
    }

    fn no_names() -> NameLookup {
        NameLookup::default()
    }

    // ---- merchant normalisation -------------------------------------------

    #[test]
    fn normalisation_strips_trailing_store_numbers() {
        assert_eq!(normalize_merchant("STARBUCKS #1234"), "starbucks");
        assert_eq!(normalize_merchant("Cafe Nero #12 #34"), "cafe nero");
    }

    #[test]
    fn normalisation_strips_long_reference_numbers_but_keeps_short_ones() {
        assert_eq!(normalize_merchant("SQ *Coffee Bar 998877"), "sq coffee bar");
        assert_eq!(normalize_merchant("AMZN123456 Mktp"), "amzn mktp");
        // Short numbers are part of the name, not noise.
        assert_eq!(normalize_merchant("7-Eleven 24"), "7 eleven 24");
    }

    #[test]
    fn normalisation_folds_case_punctuation_and_whitespace() {
        assert_eq!(normalize_merchant("  Whole   Foods  "), "whole foods");
        assert_eq!(normalize_merchant("McDonald's"), "mcdonalds");
        assert_eq!(normalize_merchant("NETFLIX.COM"), "netflix com");
        assert_eq!(normalize_merchant("Spotify  USA"), "spotify usa");
    }

    #[test]
    fn normalisation_can_leave_nothing_behind() {
        assert_eq!(normalize_merchant("  "), "");
        assert_eq!(normalize_merchant("#4471"), "");
    }

    #[test]
    fn hint_matching_needs_whole_words() {
        // "rent" must not match inside "parent", nor "hoa" inside "whoa".
        assert_eq!(classify_hint("parent teacher store", &[]), Hint::None);
        assert_eq!(classify_hint("whoa burgers", &[]), Hint::None);
        assert_eq!(classify_hint("acme rent", &[]), Hint::RecurringBill);
        assert_eq!(classify_hint("amazon prime", &[]), Hint::Subscription);
    }

    #[test]
    fn a_subscription_category_is_a_hint_on_its_own() {
        assert_eq!(classify_hint("obscure vendor", &["Subscriptions"]), Hint::Subscription);
        assert_eq!(classify_hint("obscure vendor", &["Groceries"]), Hint::None);
    }

    #[test]
    fn a_subscription_hint_outranks_a_bill_hint() {
        assert_eq!(classify_hint("gym insurance", &[]), Hint::Subscription);
    }

    // ---- cadence -----------------------------------------------------------

    #[test]
    fn intervals_map_onto_the_documented_windows() {
        assert_eq!(classify_interval(7), Some(Cadence::Weekly));
        assert_eq!(classify_interval(14), Some(Cadence::Biweekly));
        assert_eq!(classify_interval(30), Some(Cadence::Monthly));
        assert_eq!(classify_interval(91), Some(Cadence::Quarterly));
        assert_eq!(classify_interval(365), Some(Cadence::Annual));
    }

    #[test]
    fn intervals_between_the_windows_are_rejected() {
        for gap in [0, 4, 10, 11, 18, 23, 41, 74, 111, 329, 401] {
            assert_eq!(classify_interval(gap), None, "{gap} days should not classify");
        }
    }

    #[test]
    fn the_dominant_interval_is_the_median_gap() {
        // Gaps of 30, 31, 90: the mean would be 50 and classify as nothing,
        // but the median correctly reads this as monthly.
        let dates = [on("2026-01-01"), on("2026-01-31"), on("2026-03-03"), on("2026-06-01")];
        assert_eq!(dominant_interval(&dates), Some(31));
        assert_eq!(classify_interval(31), Some(Cadence::Monthly));
    }

    #[test]
    fn an_even_number_of_gaps_rounds_the_median_up() {
        let dates = [on("2026-01-05"), on("2026-02-05"), on("2026-03-05")];
        // Gaps of 31 and 28 — "about a month".
        assert_eq!(dominant_interval(&dates), Some(30));
    }

    #[test]
    fn a_single_date_has_no_interval() {
        assert_eq!(dominant_interval(&[on("2026-01-05")]), None);
        assert_eq!(dominant_interval(&[]), None);
    }

    // ---- amount variation --------------------------------------------------

    #[test]
    fn variation_is_the_full_spread_over_the_mean() {
        assert_eq!(amount_variation(&[1000, 1000, 1000]), 0.0);
        // (1100 - 900) / 1000
        assert!((amount_variation(&[900, 1000, 1100]) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn variation_of_nothing_is_zero() {
        assert_eq!(amount_variation(&[]), 0.0);
        assert_eq!(amount_variation(&[1599]), 0.0);
    }

    // ---- monthly equivalent ------------------------------------------------

    #[test]
    fn monthly_equivalent_covers_every_cadence() {
        assert_eq!(monthly_equivalent_cents(1200, Cadence::Weekly), 5200);
        assert_eq!(monthly_equivalent_cents(1200, Cadence::Biweekly), 2600);
        assert_eq!(monthly_equivalent_cents(1200, Cadence::Monthly), 1200);
        assert_eq!(monthly_equivalent_cents(1200, Cadence::Quarterly), 400);
        assert_eq!(monthly_equivalent_cents(1200, Cadence::Annual), 100);
    }

    #[test]
    fn monthly_equivalent_rounds_to_the_nearest_cent() {
        assert_eq!(monthly_equivalent_cents(100, Cadence::Annual), 8); // 8.33
        assert_eq!(monthly_equivalent_cents(150, Cadence::Annual), 13); // 12.5, rounded up
        assert_eq!(monthly_equivalent_cents(1000, Cadence::Weekly), 4333); // 4333.33
    }

    #[test]
    fn monthly_equivalent_does_not_overflow_on_absurd_amounts() {
        assert_eq!(monthly_equivalent_cents(i64::MAX, Cadence::Weekly), i64::MAX);
    }

    // ---- next expected date ------------------------------------------------

    #[test]
    fn next_date_keeps_the_day_of_the_month() {
        assert_eq!(
            next_expected_date(on("2026-01-15"), Cadence::Monthly),
            Some(on("2026-02-15"))
        );
        assert_eq!(
            next_expected_date(on("2026-01-15"), Cadence::Quarterly),
            Some(on("2026-04-15"))
        );
        assert_eq!(
            next_expected_date(on("2026-01-15"), Cadence::Annual),
            Some(on("2027-01-15"))
        );
    }

    #[test]
    fn next_date_clamps_rather_than_spilling_into_the_next_month() {
        assert_eq!(
            next_expected_date(on("2026-01-31"), Cadence::Monthly),
            Some(on("2026-02-28"))
        );
        assert_eq!(
            next_expected_date(on("2024-01-31"), Cadence::Monthly),
            Some(on("2024-02-29")),
            "2024 is a leap year"
        );
    }

    #[test]
    fn next_date_for_day_based_cadences_is_plain_arithmetic() {
        assert_eq!(
            next_expected_date(on("2026-01-05"), Cadence::Weekly),
            Some(on("2026-01-12"))
        );
        assert_eq!(
            next_expected_date(on("2026-01-05"), Cadence::Biweekly),
            Some(on("2026-01-19"))
        );
    }

    // ---- end-to-end detection ----------------------------------------------

    #[test]
    fn a_clean_monthly_subscription_is_suggested() {
        let transactions = [
            expense("2026-01-05", "NETFLIX.COM", 1599),
            expense("2026-02-05", "NETFLIX.COM", 1599),
            expense("2026-03-05", "Netflix.com", 1599),
        ];

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1);
        let suggestion = &found[0];
        assert_eq!(suggestion.pattern_key, "netflix com");
        assert_eq!(suggestion.merchant, "Netflix.com", "display casing is preserved");
        assert_eq!(suggestion.kind, RecurringKind::Subscription);
        assert_eq!(suggestion.cadence, Cadence::Monthly);
        assert_eq!(suggestion.occurrences, 3);
        assert_eq!(suggestion.confidence, Confidence::High);
        assert_eq!(suggestion.average_amount_cents, 1599);
        assert_eq!(suggestion.monthly_equivalent_cents, 1599);
        assert_eq!(suggestion.last_seen, "2026-03-05");
        assert_eq!(suggestion.next_expected_date, "2026-04-05");
    }

    #[test]
    fn detection_never_mutates_its_input() {
        let transactions = [
            expense("2026-01-05", "NETFLIX.COM", 1599),
            expense("2026-02-05", "NETFLIX.COM", 1599),
        ];
        let before = transactions.clone();

        let _ = detect_recurring(&transactions, &no_names(), on("2026-02-20"));

        assert_eq!(transactions, before);
    }

    #[test]
    fn inconsistent_intervals_are_rejected() {
        // Gaps of 3, 20 and 45 days. The median of 20 belongs to no cadence
        // window, so this is somebody shopping, not a schedule.
        let transactions = [
            expense("2026-01-01", "Netflix", 1599),
            expense("2026-01-04", "Netflix", 1599),
            expense("2026-01-24", "Netflix", 1599),
            expense("2026-03-10", "Netflix", 1599),
        ];

        assert!(detect_recurring(&transactions, &no_names(), on("2026-03-20")).is_empty());
    }

    #[test]
    fn an_unstable_price_is_rejected_for_a_subscription() {
        // Spread of 500 on a mean of ~1200 is 42%, well past the 20% a
        // subscription is allowed.
        let transactions = [
            expense("2026-01-05", "Netflix", 1000),
            expense("2026-02-05", "Netflix", 1100),
            expense("2026-03-05", "Netflix", 1500),
        ];

        assert!(detect_recurring(&transactions, &no_names(), on("2026-03-20")).is_empty());
    }

    #[test]
    fn a_bill_is_allowed_the_price_swing_a_subscription_is_not() {
        // A spread of 2500 on a mean of ~10167 is 25%: past the 20% a
        // subscription is allowed, but within the 35% a seasonal utility bill
        // gets. The merchant matching a bill hint is what buys the extra room.
        let transactions = [
            expense("2026-01-10", "City Water Utility", 9000),
            expense("2026-02-10", "City Water Utility", 10000),
            expense("2026-03-10", "City Water Utility", 11500),
        ];

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, RecurringKind::Recurring);
        assert_eq!(found[0].confidence, Confidence::Likely, "25% spread is not high confidence");
    }

    #[test]
    fn an_unhinted_merchant_needs_three_stable_occurrences() {
        let twice = [
            expense("2026-01-08", "Kwik Lawn Care", 6000),
            expense("2026-02-08", "Kwik Lawn Care", 6000),
        ];
        assert!(
            detect_recurring(&twice, &no_names(), on("2026-02-20")).is_empty(),
            "two occurrences is not enough without a hint"
        );

        let thrice = [
            expense("2026-01-08", "Kwik Lawn Care", 6000),
            expense("2026-02-08", "Kwik Lawn Care", 6000),
            expense("2026-03-08", "Kwik Lawn Care", 6000),
        ];
        let found = detect_recurring(&thrice, &no_names(), on("2026-03-20"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, RecurringKind::Recurring);
    }

    #[test]
    fn an_unhinted_merchant_needs_a_near_identical_price() {
        // 5% spread — fine for a named bill, too loose for an unknown merchant.
        let transactions = [
            expense("2026-01-08", "Kwik Lawn Care", 5900),
            expense("2026-02-08", "Kwik Lawn Care", 6000),
            expense("2026-03-08", "Kwik Lawn Care", 6200),
        ];

        assert!(detect_recurring(&transactions, &no_names(), on("2026-03-20")).is_empty());
    }

    #[test]
    fn a_weekly_grocery_run_is_never_suggested() {
        // Perfectly regular, perfectly stable, and absolutely not a bill.
        let transactions: Vec<Transaction> = ["2026-01-02", "2026-01-09", "2026-01-16", "2026-01-23", "2026-01-30"]
            .iter()
            .map(|date| expense(date, "Trader Joes", 8500))
            .collect();

        assert!(
            detect_recurring(&transactions, &no_names(), on("2026-02-02")).is_empty(),
            "weekly spending at an unhinted merchant is shopping, not a subscription"
        );
    }

    #[test]
    fn a_cancelled_subscription_stops_being_suggested() {
        let transactions = [
            expense("2026-01-05", "Netflix", 1599),
            expense("2026-02-05", "Netflix", 1599),
            expense("2026-03-05", "Netflix", 1599),
        ];

        assert_eq!(
            detect_recurring(&transactions, &no_names(), on("2026-05-01")).len(),
            1,
            "one period overdue is just a late charge"
        );
        assert!(
            detect_recurring(&transactions, &no_names(), on("2026-06-05")).is_empty(),
            "three months without a charge means it is gone"
        );
    }

    #[test]
    fn income_deleted_and_same_day_rows_are_handled() {
        let mut income = expense("2026-02-05", "Netflix", 1599);
        income.kind = TransactionKind::Income;
        let mut deleted = expense("2026-03-05", "Netflix", 1599);
        deleted.deleted_at = Some(now_timestamp());

        let transactions = [expense("2026-01-05", "Netflix", 1599), income, deleted];

        assert!(
            detect_recurring(&transactions, &no_names(), on("2026-03-20")).is_empty(),
            "only one live expense remains, which is not a pattern"
        );

        // Two charges on the same day count once, so this is still one date.
        let same_day = [
            expense("2026-01-05", "Netflix", 1599),
            expense("2026-01-05", "Netflix", 1599),
        ];
        assert!(detect_recurring(&same_day, &no_names(), on("2026-01-20")).is_empty());
    }

    #[test]
    fn a_subscription_category_promotes_an_unnamed_merchant() {
        let category_id = Uuid::new_v4();
        let names = NameLookup::from_pairs(
            [(category_id, "Subscriptions".to_string())],
            std::iter::empty(),
        );

        let transactions: Vec<Transaction> = ["2026-01-11", "2026-02-11", "2026-03-11"]
            .iter()
            .map(|date| {
                let mut t = expense(date, "Obscure Vendor Ltd", 2500);
                t.category_id = Some(category_id);
                t
            })
            .collect();

        let found = detect_recurring(&transactions, &names, on("2026-03-20"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, RecurringKind::Subscription);
        assert_eq!(found[0].category_id, Some(category_id));
        assert_eq!(found[0].category_name.as_deref(), Some("Subscriptions"));
    }

    #[test]
    fn the_account_most_charges_came_from_is_carried_through() {
        let main = Uuid::new_v4();
        let other = Uuid::new_v4();

        let mut transactions: Vec<Transaction> = ["2026-01-05", "2026-02-05", "2026-03-05"]
            .iter()
            .map(|date| {
                let mut t = expense(date, "Netflix", 1599);
                t.account_id = Some(main);
                t
            })
            .collect();
        // One charge landed on the wrong card; the majority still decides.
        transactions[1].account_id = Some(other);

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].account_id, Some(main));
    }

    #[test]
    fn suggestions_are_ordered_by_what_they_cost_per_month() {
        let mut transactions: Vec<Transaction> = ["2026-01-05", "2026-02-05", "2026-03-05"]
            .iter()
            .flat_map(|date| {
                [
                    expense(date, "Netflix", 1599),
                    expense(date, "Acme Mortgage", 180_000),
                    expense(date, "Spotify", 1099),
                ]
            })
            .collect();
        transactions.sort_by(|a, b| a.date.cmp(&b.date));

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        let order: Vec<&str> = found.iter().map(|s| s.merchant.as_str()).collect();
        assert_eq!(order, ["Acme Mortgage", "Netflix", "Spotify"]);
        assert_eq!(found[0].kind, RecurringKind::Recurring);
        assert_eq!(found[1].kind, RecurringKind::Subscription);
    }

    #[test]
    fn confidence_drops_when_the_schedule_wobbles() {
        // Gaps of 24 and 40 days: still monthly, but jitter of 8 days either
        // side of the 32-day median is more than a high-confidence call allows.
        let transactions = [
            expense("2026-01-05", "Netflix", 1599),
            expense("2026-01-29", "Netflix", 1599),
            expense("2026-03-10", "Netflix", 1599),
        ];

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].cadence, Cadence::Monthly);
        assert_eq!(found[0].confidence, Confidence::Likely);
    }

    #[test]
    fn two_occurrences_of_a_hinted_merchant_are_only_likely() {
        let transactions = [
            expense("2026-02-05", "Spotify", 1099),
            expense("2026-03-05", "Spotify", 1099),
        ];

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].occurrences, 2);
        assert_eq!(found[0].confidence, Confidence::Likely);
    }

    #[test]
    fn the_pattern_key_survives_the_noise_a_bank_adds() {
        let transactions = [
            expense("2026-01-05", "SPOTIFY #1234", 1099),
            expense("2026-02-05", "spotify  #5678", 1099),
            expense("2026-03-05", "Spotify", 1099),
        ];

        let found = detect_recurring(&transactions, &no_names(), on("2026-03-20"));

        assert_eq!(found.len(), 1, "all three spellings are one merchant");
        assert_eq!(found[0].pattern_key, "spotify");
        assert_eq!(found[0].occurrences, 3);
    }
}
