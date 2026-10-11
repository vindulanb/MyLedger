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
