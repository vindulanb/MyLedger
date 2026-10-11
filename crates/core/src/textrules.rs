//! Pulling a transaction out of a block of text, by rule.
//!
//! One engine, three callers. A receipt's extracted text and a bank's SMS
//! alert are the same problem — a pattern with capture groups, mapped onto
//! merchant, date, amount and direction — so there is one implementation of
//! it rather than one per screen. The web app's document extraction (Master
//! Prompt C, step 5) and its SMS rule authoring (step 8) both come through
//! here, and Master Prompt B's `crates/sms_parser` is meant to as well, which
//! is why nothing in this module knows about SMS, documents, or a database.
//!
//! The behaviour that matters is what happens when the answer isn't clear:
//!
//! - **No enabled rule matches** → [`MatchOutcome::NoMatch`]. Not an error.
//!   Most text is not a receipt and most messages are not bank alerts.
//! - **Exactly one rule produces a valid reading** → [`MatchOutcome::Matched`].
//! - **More than one rule matches, disagreeing** → [`MatchOutcome::Ambiguous`],
//!   carrying every candidate. The caller must route these to a human. Picking
//!   the first, or the "best", would be inventing a transaction.
//!
//! Two rules that match and *agree* are not ambiguous — that is one answer
//! arrived at twice, and treating it as a conflict would make overlapping
//! rules unusable.
//!
//! A captured date in an ambiguous numeric form (`03/04/2026`) makes the whole
//! candidate unreadable rather than resolving to one of the two readings. See
//! [`crate::csv::normalize_date_unambiguous`].

use regex::Regex;
use uuid::Uuid;

use crate::csv::normalize_date_unambiguous;
use crate::model::{SmsKindHint, TransactionKind};

/// A rule, as the engine needs to see it.
///
/// Borrowed rather than owned, and separate from the stored record types, so
/// `SmsParseRule` and `DocumentExtractionRule` can keep their own tables and
/// their own names while sharing every line of matching logic. Each provides
/// an `as_view()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleView<'a> {
    pub id: Uuid,
    pub label: &'a str,
    /// Pattern the *source* must match — a sender for a message, a filename
    /// for a document. `None` accepts any source.
    pub source_pattern: Option<&'a str>,
    /// Regex run against the text itself.
    pub body_regex: &'a str,
    /// Which capture group holds the amount. Without one, a rule cannot
    /// produce a transaction.
    pub amount_group: Option<i64>,
    pub merchant_group: Option<i64>,
    pub date_group: Option<i64>,
    /// `Debit`, `Credit`, or `Auto` to read the direction out of the text.
    pub kind_hint: Option<SmsKindHint>,
    /// The account these belong to, by name.
    pub account_hint: Option<&'a str>,
    pub default_category_id: Option<Uuid>,
    pub enabled: bool,
}

/// Everything needed to build a transaction, extracted from one match.
///
/// Serialisable because it travels to the client: the review screen shows
/// every candidate a document produced, and the SMS "Test rule" box shows
/// what a pattern would extract.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    /// Which rule produced this, so the user can see what matched.
    pub rule_id: Uuid,
    pub rule_label: String,
    /// Always positive.
    pub amount_cents: i64,
    /// The captured merchant, or the source when the rule captured none.
    pub merchant: String,
    /// `YYYY-MM-DD`: captured, or the fallback the caller supplied.
    pub date: String,
    pub kind: TransactionKind,
    /// The account by name, for the caller to resolve.
    pub account_hint: Option<String>,
    pub category_id: Option<Uuid>,
    /// Whether the date was read from the text rather than assumed.
    pub date_was_captured: bool,
}

/// What matching a block of text produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// Nothing matched. Ignore the text; this is the common case.
    NoMatch,
    /// One reading, which the caller may act on.
    Matched(Candidate),
    /// Several readings that disagree. Must go to a human, never guessed.
    Ambiguous(Vec<Candidate>),
}

impl MatchOutcome {
    /// The single confident reading, if there is one.
    pub fn confident(&self) -> Option<&Candidate> {
        match self {
            MatchOutcome::Matched(candidate) => Some(candidate),
            _ => None,
        }
    }

    /// Every candidate, however many there are.
    pub fn candidates(&self) -> &[Candidate] {
        match self {
            MatchOutcome::NoMatch => &[],
            MatchOutcome::Matched(candidate) => std::slice::from_ref(candidate),
            MatchOutcome::Ambiguous(all) => all,
        }
    }
}

/// What the text came with, and what to assume when a rule captures nothing.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The sender of a message, or a document's filename. Matched against
    /// `source_pattern`, and used as the merchant when no merchant is
    /// captured.
    pub source: &'a str,
    /// `YYYY-MM-DD` to use when the rule captures no date — a message's
    /// arrival date, or a document's upload date.
    pub fallback_date: &'a str,
}

/// Why one rule could not be used.
///
/// Returned by [`check_rule`] so a rule can be refused when it is written,
/// rather than silently never matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleProblem {
    EmptyPattern,
    BadRegex { detail: String },
    BadSourcePattern { detail: String },
    NoAmountGroup,
    /// A group number the regex does not have.
    GroupOutOfRange { field: &'static str, group: i64, groups: usize },
}

impl std::fmt::Display for RuleProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleProblem::EmptyPattern => write!(f, "the pattern is empty"),
            RuleProblem::BadRegex { detail } => write!(f, "the pattern is not a valid regex: {detail}"),
            RuleProblem::BadSourcePattern { detail } => {
                write!(f, "the sender pattern is not a valid regex: {detail}")
            }
            RuleProblem::NoAmountGroup => write!(
                f,
                "no amount group, so this rule could never produce a transaction"
            ),
            RuleProblem::GroupOutOfRange { field, group, groups } => write!(
                f,
                "the {field} group is {group}, but the pattern has {groups} capture group(s)"
            ),
        }
    }
}

impl std::error::Error for RuleProblem {}

/// Checks a rule is usable before it is saved.
///
/// Group `0` is refused deliberately: in a regex it is the whole match, so a
/// rule pointing the amount at group 0 would try to read the entire message as
/// a number and silently never work.
pub fn check_rule(rule: &RuleView<'_>) -> Result<(), RuleProblem> {
    if rule.body_regex.trim().is_empty() {
        return Err(RuleProblem::EmptyPattern);
    }
    let compiled = Regex::new(rule.body_regex)
        .map_err(|e| RuleProblem::BadRegex { detail: first_line(&e.to_string()) })?;
    if let Some(pattern) = rule.source_pattern.filter(|p| !p.trim().is_empty()) {
        Regex::new(pattern)
            .map_err(|e| RuleProblem::BadSourcePattern { detail: first_line(&e.to_string()) })?;
    }

    let groups = compiled.captures_len().saturating_sub(1);
    let Some(amount) = rule.amount_group else {
        return Err(RuleProblem::NoAmountGroup);
    };

    for (field, group) in [
        ("amount", Some(amount)),
        ("merchant", rule.merchant_group),
        ("date", rule.date_group),
    ] {
        let Some(group) = group else { continue };
        if group < 1 || group as usize > groups {
            return Err(RuleProblem::GroupOutOfRange { field, group, groups });
        }
    }

    Ok(())
}

/// Runs every enabled rule over a block of text.
pub fn match_text(rules: &[RuleView<'_>], text: &str, context: &Context<'_>) -> MatchOutcome {
    let mut found: Vec<Candidate> = Vec::new();

    for rule in rules.iter().filter(|rule| rule.enabled) {
        // A rule that doesn't compile cannot match. It is skipped rather than
        // failing the whole run, because one bad rule should not stop the
        // others — `check_rule` is what keeps one from being saved.
        if check_rule(rule).is_err() {
            continue;
        }
        if !source_matches(rule, context.source) {
            continue;
        }
        let Ok(pattern) = Regex::new(rule.body_regex) else { continue };
        let Some(captures) = pattern.captures(text) else { continue };

        let Some(candidate) = build(rule, &captures, text, context) else { continue };
        found.push(candidate);
    }

    match found.len() {
        0 => MatchOutcome::NoMatch,
        1 => MatchOutcome::Matched(found.remove(0)),
        _ => {
            // Several rules matching the same way is one answer reached
            // twice, not a conflict. Only a genuine disagreement about what
            // the transaction *is* needs a human.
            if found.iter().skip(1).all(|other| agrees(&found[0], other)) {
                MatchOutcome::Matched(found.remove(0))
            } else {
                MatchOutcome::Ambiguous(found)
            }
        }
    }
}

/// Whether two candidates describe the same transaction.
///
/// The rule that produced one is not part of the comparison: two differently
/// named rules reading the same amount, date, merchant and direction have
/// agreed.
fn agrees(a: &Candidate, b: &Candidate) -> bool {
    a.amount_cents == b.amount_cents
        && a.date == b.date
        && a.kind == b.kind
        && a.merchant.eq_ignore_ascii_case(&b.merchant)
}

fn source_matches(rule: &RuleView<'_>, source: &str) -> bool {
    match rule.source_pattern.map(str::trim).filter(|p| !p.is_empty()) {
        // No pattern accepts any source.
        None => true,
        Some(pattern) => match Regex::new(pattern) {
            Ok(compiled) => compiled.is_match(source),
            Err(_) => false,
        },
    }
}

/// Turns one set of captures into a candidate, or nothing if the amount
/// cannot be read.
fn build(
    rule: &RuleView<'_>,
    captures: &regex::Captures<'_>,
    text: &str,
    context: &Context<'_>,
) -> Option<Candidate> {
    let group = |number: Option<i64>| -> Option<&str> {
        let number = number?;
        if number < 1 {
            return None;
        }
        captures.get(number as usize).map(|m| m.as_str().trim()).filter(|s| !s.is_empty())
    };

    // Without a readable amount there is no transaction, whatever else
    // matched.
    let raw_amount = group(rule.amount_group)?;
    let (cents, marked_credit) = crate::csv::parse_signed_cents(raw_amount)?;
    if cents == 0 {
        return None;
    }

    // The merchant falls back to the source — a sender name or a filename is
    // a worse label than a captured one, but better than nothing, and the
    // user can see where it came from.
    let merchant = group(rule.merchant_group)
        .map(str::to_string)
        .unwrap_or_else(|| context.source.trim().to_string());
    if merchant.is_empty() {
        return None;
    }

    // A captured date that reads either way round makes the whole candidate
    // unusable, rather than resolving to one of the two.
    let captured_date = group(rule.date_group);
    let date = match captured_date {
        Some(raw) => normalize_date_unambiguous(raw)?,
        None => context.fallback_date.to_string(),
    };

    let kind = direction(rule, captures, text, marked_credit, cents);

    Some(Candidate {
        rule_id: rule.id,
        rule_label: rule.label.to_string(),
        amount_cents: cents.abs(),
        merchant,
        date,
        kind,
        account_hint: rule
            .account_hint
            .map(str::trim)
            .filter(|hint| !hint.is_empty())
            .map(str::to_string),
        category_id: rule.default_category_id,
        date_was_captured: captured_date.is_some(),
    })
}

/// Which way the money went.
///
/// An explicit hint on the rule wins. `Auto`, or no hint at all, reads the
/// text for a keyword, then falls back to the sign on the amount — and
/// finally to an expense, because that is what the overwhelming majority of
/// receipts and card alerts are.
///
/// The keyword search covers the **whole text**, not the span the pattern
/// matched. "Work it out from the message" means the message: a rule whose
/// pattern is just `([0-9.]+)` matches four characters, and looking only
/// there would find no keyword in any message ever.
fn direction(
    rule: &RuleView<'_>,
    captures: &regex::Captures<'_>,
    text: &str,
    marked_credit: bool,
    cents: i64,
) -> TransactionKind {
    match rule.kind_hint {
        Some(SmsKindHint::Debit) => return TransactionKind::Expense,
        Some(SmsKindHint::Credit) => return TransactionKind::Income,
        Some(SmsKindHint::Auto) | None => {}
    }

    // A named group called `kind` is where a rule says to look; without one,
    // the whole text is searched.
    let haystack = captures.name("kind").map(|m| m.as_str()).unwrap_or(text).to_lowercase();

    const INCOME_WORDS: &[&str] =
        &["credited", "credit", "received", "deposited", "deposit", "refund", "salary", "paid in"];
    const EXPENSE_WORDS: &[&str] = &[
        "debited", "debit", "spent", "withdrawn", "withdrawal", "purchase", "paid to", "charged",
    ];

    // Expense words are checked first: "debited" contains no income word, but
    // "refund credited" contains both, and a refund really is income.
    let income_at = INCOME_WORDS.iter().filter_map(|word| haystack.find(word)).min();
    let expense_at = EXPENSE_WORDS.iter().filter_map(|word| haystack.find(word)).min();

    match (income_at, expense_at) {
        // Whichever appears first in the text is the one describing this
        // transaction.
        (Some(income), Some(expense)) => {
            if income <= expense {
                TransactionKind::Income
            } else {
                TransactionKind::Expense
            }
        }
        (Some(_), None) => TransactionKind::Income,
        (None, Some(_)) => TransactionKind::Expense,
        // No keyword at all. A `CR` marker on the amount is the last real
        // evidence; failing that, an expense, which is what the overwhelming
        // majority of receipts and card alerts are. Stated here so the
        // choice is visible rather than incidental.
        (None, None) if marked_credit => TransactionKind::Income,
        (None, None) if cents < 0 => TransactionKind::Expense,
        (None, None) => TransactionKind::Expense,
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule<'a>(label: &'a str, body_regex: &'a str) -> RuleView<'a> {
        RuleView {
            id: Uuid::new_v4(),
            label,
            source_pattern: None,
            body_regex,
            amount_group: Some(1),
            merchant_group: None,
            date_group: None,
            kind_hint: None,
            account_hint: None,
            default_category_id: None,
            enabled: true,
        }
    }

    fn context<'a>(source: &'a str) -> Context<'a> {
        Context { source, fallback_date: "2026-10-11" }
    }

    // ---- checking a rule -----------------------------------------------

    #[test]
    fn a_workable_rule_passes() {
        assert!(check_rule(&rule("card", r"spent ([0-9.]+)")).is_ok());
    }

    #[test]
    fn an_empty_pattern_is_refused() {
        assert_eq!(check_rule(&rule("x", "   ")), Err(RuleProblem::EmptyPattern));
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_refused() {
        let problem = check_rule(&rule("x", r"spent ([0-9.+")).unwrap_err();
        assert!(matches!(problem, RuleProblem::BadRegex { .. }), "{problem:?}");
    }

    #[test]
    fn a_rule_with_no_amount_group_could_never_work_and_is_refused() {
        let mut one = rule("x", r"spent ([0-9.]+)");
        one.amount_group = None;
        assert_eq!(check_rule(&one), Err(RuleProblem::NoAmountGroup));
    }

    #[test]
    fn a_group_number_beyond_the_pattern_is_refused() {
        let mut one = rule("x", r"spent ([0-9.]+)");
        one.merchant_group = Some(4);
        let problem = check_rule(&one).unwrap_err();
        assert!(
            matches!(problem, RuleProblem::GroupOutOfRange { field: "merchant", group: 4, groups: 1 }),
            "{problem:?}"
        );
    }

    #[test]
    fn group_zero_is_refused_because_it_is_the_whole_match() {
        let mut one = rule("x", r"spent ([0-9.]+)");
        one.amount_group = Some(0);
        assert!(check_rule(&one).is_err());
    }

    #[test]
    fn a_bad_source_pattern_is_refused_too() {
        let mut one = rule("x", r"spent ([0-9.]+)");
        one.source_pattern = Some("(unclosed");
        let problem = check_rule(&one).unwrap_err();
        assert!(matches!(problem, RuleProblem::BadSourcePattern { .. }), "{problem:?}");
    }

    // ---- matching ------------------------------------------------------

    #[test]
    fn nothing_matching_is_not_an_error() {
        let rules = [rule("card", r"spent ([0-9.]+)")];
        assert_eq!(
            match_text(&rules, "your parcel has shipped", &context("DHL")),
            MatchOutcome::NoMatch
        );
    }

    #[test]
    fn no_rules_at_all_matches_nothing() {
        assert_eq!(match_text(&[], "spent 10.00", &context("BANK")), MatchOutcome::NoMatch);
    }

    #[test]
    fn a_single_rule_match_produces_one_candidate() {
        let rules = [rule("card", r"spent ([0-9.]+)")];
        let outcome = match_text(&rules, "You spent 12.50 at the shop", &context("MYBANK"));
        let candidate = outcome.confident().expect("a match");
        assert_eq!(candidate.amount_cents, 1250);
        assert_eq!(candidate.rule_label, "card");
    }

    #[test]
    fn a_disabled_rule_never_matches() {
        let mut one = rule("card", r"spent ([0-9.]+)");
        one.enabled = false;
        assert_eq!(match_text(&[one], "spent 12.50", &context("B")), MatchOutcome::NoMatch);
    }

    #[test]
    fn a_rule_that_does_not_compile_is_skipped_not_fatal() {
        // One bad rule must not stop the good ones.
        let bad = rule("broken", r"([0-9.+");
        let good = rule("card", r"spent ([0-9.]+)");
        let outcome = match_text(&[bad, good], "spent 12.50", &context("B"));
        assert_eq!(outcome.confident().unwrap().amount_cents, 1250);
    }

    #[test]
    fn the_merchant_falls_back_to_the_source() {
        let rules = [rule("card", r"spent ([0-9.]+)")];
        let outcome = match_text(&rules, "spent 12.50", &context("MYBANK"));
        assert_eq!(outcome.confident().unwrap().merchant, "MYBANK");
    }

    #[test]
    fn a_captured_merchant_is_used_when_there_is_one() {
        let mut one = rule("card", r"spent ([0-9.]+) at (.+)");
        one.merchant_group = Some(2);
        let outcome = match_text(&[one], "spent 12.50 at Corner Grocer", &context("MYBANK"));
        assert_eq!(outcome.confident().unwrap().merchant, "Corner Grocer");
    }

    #[test]
    fn the_date_falls_back_to_the_context_when_none_is_captured() {
        let rules = [rule("card", r"spent ([0-9.]+)")];
        let outcome = match_text(&rules, "spent 12.50", &context("B"));
        let candidate = outcome.confident().unwrap();
        assert_eq!(candidate.date, "2026-10-11");
        assert!(!candidate.date_was_captured, "nothing was captured, so this must say so");
    }

    #[test]
    fn a_captured_iso_date_is_used() {
        let mut one = rule("card", r"on (\d{4}-\d{2}-\d{2}) spent ([0-9.]+)");
        one.date_group = Some(1);
        one.amount_group = Some(2);
        let outcome = match_text(&[one], "on 2026-09-04 spent 12.50", &context("B"));
        let candidate = outcome.confident().unwrap();
        assert_eq!(candidate.date, "2026-09-04");
        assert!(candidate.date_was_captured);
    }

    #[test]
    fn a_captured_date_with_a_month_name_is_used() {
        let mut one = rule("card", r"on (\d+ \w+ \d{4}) spent ([0-9.]+)");
        one.date_group = Some(1);
        one.amount_group = Some(2);
        let outcome = match_text(&[one], "on 4 September 2026 spent 12.50", &context("B"));
        assert_eq!(outcome.confident().unwrap().date, "2026-09-04");
    }

    #[test]
    fn an_ambiguous_captured_date_makes_the_whole_candidate_unusable() {
        // 03/04/2026 is the 3rd of April or the 4th of March, and a single
        // receipt has no column of other dates to settle it. Guessing would
        // put the transaction a month out without saying so.
        let mut one = rule("card", r"on (\S+) spent ([0-9.]+)");
        one.date_group = Some(1);
        one.amount_group = Some(2);
        assert_eq!(
            match_text(&[one], "on 03/04/2026 spent 12.50", &context("B")),
            MatchOutcome::NoMatch
        );
    }

    #[test]
    fn a_numeric_date_that_can_only_be_read_one_way_is_accepted() {
        let mut one = rule("card", r"on (\S+) spent ([0-9.]+)");
        one.date_group = Some(1);
        one.amount_group = Some(2);
        let outcome = match_text(&[one], "on 25/09/2026 spent 12.50", &context("B"));
        assert_eq!(outcome.confident().unwrap().date, "2026-09-25");
    }

    // ---- the source pattern ---------------------------------------------

    #[test]
    fn a_source_pattern_restricts_which_sources_match() {
        let mut one = rule("card", r"spent ([0-9.]+)");
        one.source_pattern = Some("MYBANK");
        assert!(match_text(&[one], "spent 12.50", &context("MYBANK-ALERTS")).confident().is_some());
        assert_eq!(
            match_text(&[one], "spent 12.50", &context("SOMEONE-ELSE")),
            MatchOutcome::NoMatch
        );
    }

    #[test]
    fn an_empty_source_pattern_accepts_any_source() {
        let mut one = rule("card", r"spent ([0-9.]+)");
        one.source_pattern = Some("   ");
        assert!(match_text(&[one], "spent 12.50", &context("ANYONE")).confident().is_some());
    }

    // ---- ambiguity -------------------------------------------------------

    #[test]
    fn two_rules_reading_the_same_transaction_differently_are_ambiguous() {
        // This is the case that must never be guessed.
        let first = rule("one", r"spent ([0-9.]+)");
        let mut second = rule("two", r"of ([0-9.]+)");
        second.amount_group = Some(1);

        let outcome = match_text(&[first, second], "spent 12.50 of 99.00", &context("B"));
        match outcome {
            MatchOutcome::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
                let amounts: Vec<i64> = candidates.iter().map(|c| c.amount_cents).collect();
                assert!(amounts.contains(&1250) && amounts.contains(&9900), "{amounts:?}");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn two_rules_agreeing_are_one_answer_not_a_conflict() {
        // Otherwise overlapping rules would be unusable: a general rule and a
        // specific one reading the same receipt the same way is fine.
        let first = rule("general", r"spent ([0-9.]+)");
        let second = rule("specific", r"You spent ([0-9.]+)");
        let outcome = match_text(&[first, second], "You spent 12.50", &context("B"));
        assert_eq!(outcome.confident().map(|c| c.amount_cents), Some(1250));
    }

    #[test]
    fn candidates_are_all_available_whichever_the_outcome() {
        let first = rule("one", r"spent ([0-9.]+)");
        let mut second = rule("two", r"of ([0-9.]+)");
        second.amount_group = Some(1);
        let outcome = match_text(&[first, second], "spent 1.00 of 2.00", &context("B"));
        assert_eq!(outcome.candidates().len(), 2);
        assert_eq!(match_text(&[], "x", &context("B")).candidates().len(), 0);
    }

    // ---- direction -------------------------------------------------------

    #[test]
    fn an_explicit_hint_decides_the_direction() {
        let mut debit = rule("d", r"([0-9.]+)");
        debit.kind_hint = Some(SmsKindHint::Debit);
        assert_eq!(
            match_text(&[debit], "credited 12.50", &context("B")).confident().unwrap().kind,
            TransactionKind::Expense,
            "an explicit hint beats a keyword in the text"
        );

        let mut credit = rule("c", r"([0-9.]+)");
        credit.kind_hint = Some(SmsKindHint::Credit);
        assert_eq!(
            match_text(&[credit], "debited 12.50", &context("B")).confident().unwrap().kind,
            TransactionKind::Income
        );
    }

    #[test]
    fn auto_reads_the_direction_from_a_keyword() {
        let mut one = rule("a", r"([0-9.]+)");
        one.kind_hint = Some(SmsKindHint::Auto);

        for (text, expected) in [
            ("debited 12.50 from your account", TransactionKind::Expense),
            ("credited 12.50 to your account", TransactionKind::Income),
            ("you spent 12.50", TransactionKind::Expense),
            ("payment received 12.50", TransactionKind::Income),
            ("12.50 withdrawn", TransactionKind::Expense),
            ("salary 12.50", TransactionKind::Income),
        ] {
            let outcome = match_text(&[one], text, &context("B"));
            assert_eq!(outcome.confident().unwrap().kind, expected, "{text}");
        }
    }

    #[test]
    fn with_no_hint_and_no_keyword_it_reads_as_an_expense() {
        // Which is what the overwhelming majority of receipts and card alerts
        // are. Stated here so the choice is visible rather than incidental.
        let rules = [rule("a", r"total ([0-9.]+)")];
        let outcome = match_text(&rules, "total 12.50", &context("B"));
        assert_eq!(outcome.confident().unwrap().kind, TransactionKind::Expense);
    }

    #[test]
    fn a_named_kind_group_is_where_auto_looks_first() {
        let mut one = rule("a", r"(?<kind>credited|debited) ([0-9.]+)");
        one.kind_hint = Some(SmsKindHint::Auto);
        one.amount_group = Some(2);
        let outcome = match_text(&[one], "credited 12.50 after a debited fee", &context("B"));
        assert_eq!(outcome.confident().unwrap().kind, TransactionKind::Income);
    }

    #[test]
    fn whichever_keyword_comes_first_describes_the_transaction() {
        let mut one = rule("a", r"([0-9.]+)");
        one.kind_hint = Some(SmsKindHint::Auto);
        // A refund credited, with a later mention of the original purchase.
        let outcome = match_text(&[one], "refund 12.50 for your purchase", &context("B"));
        assert_eq!(outcome.confident().unwrap().kind, TransactionKind::Income);
    }

    // ---- amounts ---------------------------------------------------------

    #[test]
    fn the_amount_is_stored_positive_whatever_the_text_said() {
        let rules = [rule("a", r"([-0-9.]+)")];
        let outcome = match_text(&rules, "-12.50", &context("B"));
        let candidate = outcome.confident().unwrap();
        assert_eq!(candidate.amount_cents, 1250);
        assert_eq!(candidate.kind, TransactionKind::Expense);
    }

    #[test]
    fn a_decorated_amount_is_still_read() {
        let rules = [rule("a", r"(?i)total:?\s*(\S+)")];
        let outcome = match_text(&rules, "Total: $1,234.56", &context("B"));
        assert_eq!(outcome.confident().unwrap().amount_cents, 123_456);
    }

    #[test]
    fn an_unreadable_amount_means_no_candidate() {
        let rules = [rule("a", r"total (\w+)")];
        assert_eq!(match_text(&rules, "total pending", &context("B")), MatchOutcome::NoMatch);
    }

    #[test]
    fn a_zero_amount_is_not_a_transaction() {
        let rules = [rule("a", r"total ([0-9.]+)")];
        assert_eq!(match_text(&rules, "total 0.00", &context("B")), MatchOutcome::NoMatch);
    }

    #[test]
    fn a_group_that_matched_nothing_means_no_candidate() {
        // An optional group the text didn't fill.
        let rules = [rule("a", r"total(?: ([0-9.]+))?")];
        assert_eq!(match_text(&rules, "total", &context("B")), MatchOutcome::NoMatch);
    }

    // ---- the rest of the candidate --------------------------------------

    #[test]
    fn the_account_and_category_hints_come_through() {
        let category = Uuid::new_v4();
        let mut one = rule("a", r"spent ([0-9.]+)");
        one.account_hint = Some("Credit Card");
        one.default_category_id = Some(category);

        let outcome = match_text(&[one], "spent 12.50", &context("B"));
        let candidate = outcome.confident().unwrap();
        assert_eq!(candidate.account_hint.as_deref(), Some("Credit Card"));
        assert_eq!(candidate.category_id, Some(category));
    }

    #[test]
    fn a_blank_account_hint_is_treated_as_absent() {
        let mut one = rule("a", r"spent ([0-9.]+)");
        one.account_hint = Some("   ");
        let outcome = match_text(&[one], "spent 12.50", &context("B"));
        assert_eq!(outcome.confident().unwrap().account_hint, None);
    }

    #[test]
    fn a_realistic_receipt_reads_end_to_end() {
        let category = Uuid::new_v4();
        let mut one = RuleView {
            id: Uuid::new_v4(),
            label: "Corner Grocer receipt",
            source_pattern: None,
            body_regex: r"(?is)grocer.*?date:\s*(\S+).*?total\s*:?\s*\$?([0-9.,]+)",
            amount_group: Some(2),
            merchant_group: None,
            date_group: Some(1),
            kind_hint: Some(SmsKindHint::Debit),
            account_hint: Some("Main Account"),
            default_category_id: Some(category),
            enabled: true,
        };
        one.enabled = true;

        let text = "CORNER GROCER #4411\n\
                    123 Example Street\n\
                    Date: 2026-10-04\n\
                    Bananas          2.40\n\
                    Milk             1.80\n\
                    TOTAL:         $62.14\n\
                    Thank you";

        let outcome = match_text(&[one], text, &context("receipt-oct.pdf"));
        let candidate = outcome.confident().expect("a match");
        assert_eq!(candidate.amount_cents, 6214);
        assert_eq!(candidate.date, "2026-10-04");
        assert!(candidate.date_was_captured);
        assert_eq!(candidate.kind, TransactionKind::Expense);
        // No merchant group, so the filename stands in and the user can see
        // where the name came from.
        assert_eq!(candidate.merchant, "receipt-oct.pdf");
        assert_eq!(candidate.account_hint.as_deref(), Some("Main Account"));
        assert_eq!(candidate.category_id, Some(category));
    }
}
