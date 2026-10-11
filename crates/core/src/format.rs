//! Turning stored values into display strings.
//!
//! Money is an `i64` count of cents everywhere in MyLedger. This module is the
//! only place it becomes text, and the text is never parsed back — which is
//! what keeps rounding out of the arithmetic. It lives in `core` so the web and
//! mobile apps render an amount the same way rather than each inventing a
//! formatter.

/// Formats a count of cents as currency.
///
/// Thousands are grouped, the fraction is always two digits, and a negative
/// amount puts its sign outside the symbol (`-$5.45`) the way a statement
/// does.
///
/// ```
/// # use myledger_core::format::money;
/// assert_eq!(money(545), "$5.45");
/// assert_eq!(money(123_456), "$1,234.56");
/// assert_eq!(money(-545), "-$5.45");
/// ```
pub fn money(cents: i64) -> String {
    let negative = cents < 0;
    // `unsigned_abs`, not `abs`: `i64::MIN.abs()` would overflow.
    let absolute = cents.unsigned_abs();

    let mut out = String::with_capacity(12);
    if negative {
        out.push('-');
    }
    out.push('$');
    push_grouped(&mut out, absolute / 100);
    out.push('.');
    let fraction = absolute % 100;
    out.push((b'0' + (fraction / 10) as u8) as char);
    out.push((b'0' + (fraction % 10) as u8) as char);
    out
}

/// Formats an amount with an explicit direction.
///
/// Amounts are stored positive, with
/// [`TransactionKind`](crate::TransactionKind) carrying the direction, so a
/// shared amount column needs to be told which way the money went.
pub fn signed_money(cents: i64, is_income: bool) -> String {
    let mut out = String::with_capacity(13);
    out.push(if is_income { '+' } else { '-' });
    out.push_str(&money(cents.unsigned_abs().min(i64::MAX as u64) as i64));
    out
}

/// Formats a whole number with thousands separators.
fn push_grouped(out: &mut String, value: u64) {
    let digits = value.to_string();
    let len = digits.len();
    for (index, ch) in digits.char_indices() {
        if index > 0 && (len - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
}

#[cfg(test)]
mod tests {
    use super::{money, signed_money};

    #[test]
    fn cents_render_with_two_decimal_places() {
        assert_eq!(money(0), "$0.00");
        assert_eq!(money(5), "$0.05");
        assert_eq!(money(50), "$0.50");
        assert_eq!(money(100), "$1.00");
        assert_eq!(money(545), "$5.45");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(money(99_999), "$999.99");
        assert_eq!(money(100_000), "$1,000.00");
        assert_eq!(money(123_456), "$1,234.56");
        assert_eq!(money(100_000_000), "$1,000,000.00");
        assert_eq!(money(1_234_567_890), "$12,345,678.90");
    }

    #[test]
    fn negatives_put_the_sign_outside_the_symbol() {
        assert_eq!(money(-5), "-$0.05");
        assert_eq!(money(-545), "-$5.45");
        assert_eq!(money(-123_456), "-$1,234.56");
    }

    #[test]
    fn the_extremes_do_not_overflow() {
        // `i64::MIN.abs()` panics, so the implementation uses `unsigned_abs`.
        assert_eq!(money(i64::MIN), "-$92,233,720,368,547,758.08");
        assert_eq!(money(i64::MAX), "$92,233,720,368,547,758.07");
    }

    #[test]
    fn a_signed_amount_shows_direction_not_storage() {
        // Amounts are stored positive; the direction comes from the kind.
        assert_eq!(signed_money(545, false), "-$5.45");
        assert_eq!(signed_money(545, true), "+$5.45");
        assert_eq!(signed_money(0, true), "+$0.00");
    }
}

/// Reads typed money into cents.
///
/// Accepts `12`, `12.5`, `12.50`, `1,234.56`, `5.` and `.5`, plus a leading
/// currency symbol and surrounding whitespace, because those are all things a
/// person types into an amount field. Anything else is `None` rather than a
/// guess: a silently misread amount is the worst bug an entry form can have.
///
/// Whitespace *inside* the number is refused, which is why it is stripped from
/// the ends only. Accepting it would read `1 2` as `$12.00` and `1 234` as
/// `$1,234.00` — the second is a real convention elsewhere, the first is a
/// typo, and nothing in the string says which one this is.
///
/// More than two decimal places is a rejection, not a rounding. If the user
/// meant `1.239` they need to say which cent they want, and quietly choosing
/// one for them is how a ledger starts disagreeing with a bank statement.
///
/// ```
/// # use myledger_core::format::parse_money;
/// assert_eq!(parse_money("12.50"), Some(1250));
/// assert_eq!(parse_money("$1,234.56"), Some(123_456));
/// assert_eq!(parse_money("1.239"), None);
/// ```
pub fn parse_money(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let cleaned: String = trimmed.chars().filter(|c| *c != ',' && *c != '$').collect();
    if cleaned.is_empty() {
        return None;
    }

    let (whole, fraction) = match cleaned.split_once('.') {
        None => (cleaned.as_str(), ""),
        Some((whole, fraction)) => (whole, fraction),
    };

    // A second separator means this isn't a number at all.
    if fraction.contains('.') || fraction.len() > 2 {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // "." alone is not an amount, though "5." and ".5" are.
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }

    let units: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let cents: i64 = match fraction.len() {
        0 => 0,
        // One decimal place is tenths: "12.5" is 1250 cents, not 125.
        1 => fraction.parse::<i64>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    units.checked_mul(100)?.checked_add(cents)
}

#[cfg(test)]
mod parse_tests {
    use super::{money, parse_money};

    #[test]
    fn whole_amounts_become_cents() {
        assert_eq!(parse_money("0"), Some(0));
        assert_eq!(parse_money("5"), Some(500));
        assert_eq!(parse_money("1234"), Some(123_400));
    }

    #[test]
    fn two_decimal_places_are_taken_as_written() {
        assert_eq!(parse_money("12.50"), Some(1250));
        assert_eq!(parse_money("0.05"), Some(5));
        assert_eq!(parse_money("0.99"), Some(99));
    }

    #[test]
    fn one_decimal_place_means_tenths() {
        // The trap this test exists for: "12.5" is twelve fifty, not twelve
        // dollars and five cents.
        assert_eq!(parse_money("12.5"), Some(1250));
        assert_eq!(parse_money("0.5"), Some(50));
    }

    #[test]
    fn the_things_people_actually_type_are_accepted() {
        assert_eq!(parse_money("  12.50  "), Some(1250));
        // Whitespace is stripped from the ends only; see the doc comment.
        assert_eq!(parse_money("1 234.56"), None);
        assert_eq!(parse_money("$12.50"), Some(1250));
        assert_eq!(parse_money("1,234.56"), Some(123_456));
        assert_eq!(parse_money("$1,000"), Some(100_000));
        // Mid-typing states, so the live preview doesn't flicker to an error.
        assert_eq!(parse_money("5."), Some(500));
        assert_eq!(parse_money(".5"), Some(50));
    }

    #[test]
    fn more_than_two_decimal_places_is_refused_not_rounded() {
        assert_eq!(parse_money("1.239"), None);
        assert_eq!(parse_money("1.000"), None);
    }

    #[test]
    fn nonsense_is_refused() {
        for bad in ["", "   ", ".", "abc", "1.2.3", "12a", "-5", "1e3", "--", "$", "1 2"] {
            assert_eq!(parse_money(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_negative_is_refused_because_direction_is_the_kind() {
        // Amounts are stored positive; expense or income carries the sign.
        assert_eq!(parse_money("-12.50"), None);
    }

    #[test]
    fn an_amount_too_large_for_the_type_is_refused_not_wrapped() {
        assert_eq!(parse_money("99999999999999999999"), None);
        assert_eq!(parse_money("92233720368547758.08"), None);
    }

    #[test]
    fn parsing_then_formatting_round_trips() {
        for text in ["0.00", "5.45", "1,234.56", "999,999.99"] {
            let cents = parse_money(text).expect(text);
            assert_eq!(money(cents), format!("${text}"));
        }
    }
}
