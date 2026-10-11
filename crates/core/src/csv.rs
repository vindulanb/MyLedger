//! Reading bank and card statements.
//!
//! One parser, in `core`, used by both sides: the web app parses the file
//! locally to show a column preview and ask about anything ambiguous, and the
//! server parses it again authoritatively when importing. The client's parse
//! is never trusted — it only decides what to *ask* — but because both run
//! the same code, the preview cannot promise something the import then reads
//! differently.
//!
//! The reader is hand-written rather than pulled from a crate so that `core`
//! stays dependency-light for the mobile app, and because the awkward parts of
//! a real statement are not the CSV grammar but the three places a file can be
//! genuinely ambiguous:
//!
//! 1. **Which column is which.** Banks name things differently, so headers are
//!    matched against a list of known spellings.
//! 2. **Date order.** `03/04/2026` is the 3rd of April or the 4th of March,
//!    and nothing in the file says which. The whole column is examined for a
//!    day above 12 to settle it; if every row is ambiguous, that is reported
//!    rather than guessed.
//! 3. **Sign convention.** Some statements use separate debit and credit
//!    columns, some put a sign on one amount column, some keep every amount
//!    positive and add a "type" column. A file with one all-positive amount
//!    column and no type column cannot be read at all without being told, and
//!    assuming "expense" would silently turn a salary into a cost.
//!
//! In all three cases the answer is a question to the user, never a guess.
//! Everything here is pure: no row is written, nothing is deduplicated, and
//! the caller decides what to do with what comes back.

use crate::period::split_date;

/// A parsed file: one header row and the data rows under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    /// Data rows, each padded or trimmed to the header count.
    pub rows: Vec<Vec<String>>,
    /// The delimiter that was detected, for showing back to the user.
    pub delimiter: char,
}

/// Why a file could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvError {
    Empty,
    /// The header row had no usable column names.
    NoHeaders,
    /// A quoted field ran to the end of the file without closing.
    UnclosedQuote { line: usize },
}

impl std::fmt::Display for CsvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsvError::Empty => write!(f, "the file is empty"),
            CsvError::NoHeaders => write!(f, "the first row has no column names"),
            CsvError::UnclosedQuote { line } => {
                write!(f, "a quoted value starting on line {line} is never closed")
            }
        }
    }
}

impl std::error::Error for CsvError {}

/// Which order a date column is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    /// `YYYY-MM-DD` or `YYYY/MM/DD`.
    Iso,
    /// `DD/MM/YYYY`.
    DayFirst,
    /// `MM/DD/YYYY`.
    MonthFirst,
    /// `15 Jan 2026`, `Jan 15 2026`, `15-Jan-26`.
    Named,
}

/// What examining a whole date column concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateGuess {
    /// Settled, either by the shape of the values or by a day above 12.
    Certain(DateFormat),
    /// Every value could be read either way round. The caller must ask.
    Ambiguous,
    /// Nothing in the column looked like a date.
    Unreadable,
}

/// Where a row's amount and its direction come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountSource {
    /// Separate columns, at most one of which is filled per row. Unambiguous,
    /// and the easiest kind of statement to read.
    DebitCredit { debit: usize, credit: usize },
    /// One column whose sign carries the direction: negative is money out.
    Signed { amount: usize },
    /// One column of positive amounts, plus a column saying which way.
    Labelled { amount: usize, kind: usize },
    /// One column of positive amounts, and the user has told us which way
    /// every row goes.
    ///
    /// This is the answer to [`AmountGuess::NeedsDirection`], and it is a
    /// real case rather than a fallback: a credit-card export is often all
    /// purchases, with nothing in the file saying so.
    Fixed { amount: usize, is_income: bool },
}

/// What examining the amount columns concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmountGuess {
    Certain(AmountSource),
    /// One amount column, every value positive, and no column saying which
    /// way the money went. Unreadable without being told.
    NeedsDirection { amount: usize },
    Unreadable,
}

/// Which column holds what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mapping {
    pub date: Option<usize>,
    pub merchant: Option<usize>,
    pub category: Option<usize>,
    pub account: Option<usize>,
}

/// Everything a caller needs to decide whether it can import without asking.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub mapping: Mapping,
    pub date: DateGuess,
    pub amount: AmountGuess,
}

impl Plan {
    /// The questions that have to be answered before this file can be read.
    ///
    /// An empty list means the import can go ahead as detected. Each entry is
    /// a sentence fit to show someone.
    pub fn questions(&self) -> Vec<String> {
        let mut asking = Vec::new();
        if self.mapping.date.is_none() {
            asking.push("Which column holds the date?".to_string());
        }
        if self.mapping.merchant.is_none() {
            asking.push("Which column holds the description or merchant?".to_string());
        }
        match &self.amount {
            AmountGuess::Certain(_) => {}
            AmountGuess::NeedsDirection { .. } => asking.push(
                "Every amount in this file is positive and nothing says which way the money \
                 went. Which column says debit or credit?"
                    .to_string(),
            ),
            AmountGuess::Unreadable => {
                asking.push("Which column holds the amount?".to_string())
            }
        }
        match &self.date {
            DateGuess::Certain(_) => {}
            DateGuess::Ambiguous => asking.push(
                "Dates in this file could be day-first or month-first. Which is it?".to_string(),
            ),
            DateGuess::Unreadable => {
                asking.push("The date column could not be read. Which column is it?".to_string())
            }
        }
        asking
    }

    /// Whether the file can be imported exactly as detected.
    pub fn is_settled(&self) -> bool {
        self.questions().is_empty()
    }
}

/// One row, read and ready to become a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRow {
    /// 1-based line in the file, for reporting.
    pub line: usize,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub merchant: String,
    /// Always positive.
    pub amount_cents: i64,
    /// `true` for income, `false` for an expense.
    pub is_income: bool,
    /// The category name as the statement wrote it, if it had one.
    pub category: Option<String>,
    /// The account name as the statement wrote it, if it had one.
    pub account: Option<String>,
}

/// A row that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    pub line: usize,
    pub reason: String,
}

/// The result of reading every row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadRows {
    pub rows: Vec<ImportRow>,
    /// Rows that could not be read. No placeholder transaction is ever
    /// invented for one — a line the parser did not understand is reported,
    /// not approximated.
    pub skipped: Vec<SkippedRow>,
}

// ---- reading the file -----------------------------------------------------

/// Parses a statement into headers and rows.
pub fn read_csv(text: &str) -> Result<Table, CsvError> {
    // A BOM would otherwise become part of the first header's name, which is
    // how "Date" quietly stops matching.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Err(CsvError::Empty);
    }

    let delimiter = detect_delimiter(text);
    let mut records = split_records(text, delimiter)?;
    if records.is_empty() {
        return Err(CsvError::Empty);
    }

    let headers: Vec<String> = records.remove(0).into_iter().map(|h| h.trim().to_string()).collect();
    if headers.iter().all(|h| h.is_empty()) {
        return Err(CsvError::NoHeaders);
    }

    // Every row is squared off to the header count, so a column index is
    // always safe to use and a short row reads as empty rather than panicking.
    let width = headers.len();
    let rows: Vec<Vec<String>> = records
        .into_iter()
        .filter(|row| !(row.len() == 1 && row[0].trim().is_empty()))
        .map(|mut row| {
            row.resize(width, String::new());
            row
        })
        .collect();

    Ok(Table { headers, rows, delimiter })
}

/// Picks the delimiter by counting candidates outside quotes on the first line.
///
/// Semicolons and tabs are as common as commas in statements exported in
/// Europe, and a file split on the wrong character reads as one giant column.
fn detect_delimiter(text: &str) -> char {
    let first_line = text.lines().next().unwrap_or_default();
    let mut best = (',', 0usize);
    for candidate in [',', ';', '\t', '|'] {
        let mut count = 0;
        let mut quoted = false;
        for ch in first_line.chars() {
            match ch {
                '"' => quoted = !quoted,
                c if c == candidate && !quoted => count += 1,
                _ => {}
            }
        }
        if count > best.1 {
            best = (candidate, count);
        }
    }
    best.0
}

/// Splits text into records of fields, honouring quotes.
fn split_records(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, CsvError> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1usize;
    let mut quote_started_on = 1usize;

    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' => {
                    // A doubled quote inside a quoted field is one quote.
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        quoted = false;
                    }
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                c => field.push(c),
            }
            continue;
        }

        match ch {
            '"' => {
                quoted = true;
                quote_started_on = line;
            }
            c if c == delimiter => {
                record.push(std::mem::take(&mut field));
            }
            '\r' => {
                // Swallowed: a CRLF ends the record on the \n.
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                line += 1;
            }
            c => field.push(c),
        }
    }

    if quoted {
        return Err(CsvError::UnclosedQuote { line: quote_started_on });
    }

    // Whatever is left is the last record, unless the file ended on a newline.
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }

    Ok(records)
}

// ---- recognising the columns ----------------------------------------------

/// Header spellings for each thing worth finding, lowercase.
///
/// Longer and more specific names come first so that "transaction date" wins
/// over "date" and "debit amount" is not mistaken for the amount column.
const DATE_NAMES: &[&str] = &[
    "transaction date",
    "posting date",
    "posted date",
    "value date",
    "booking date",
    "date posted",
    "trans date",
    "date",
];

const MERCHANT_NAMES: &[&str] = &[
    "merchant name",
    "merchant",
    "description",
    "narrative",
    "details",
    "particulars",
    "payee",
    "name",
    "reference",
    "memo",
    "transaction",
];

const AMOUNT_NAMES: &[&str] =
    &["transaction amount", "amount", "value", "sum", "total", "amount (gbp)", "amt"];

const DEBIT_NAMES: &[&str] =
    &["debit amount", "debit", "withdrawal", "withdrawals", "money out", "paid out", "out"];

const CREDIT_NAMES: &[&str] =
    &["credit amount", "credit", "deposit", "deposits", "money in", "paid in", "in"];

const CATEGORY_NAMES: &[&str] = &["category", "categories", "type of transaction", "class"];

const ACCOUNT_NAMES: &[&str] = &["account name", "account", "account number", "card"];

/// Column names that say which way the money went.
const KIND_NAMES: &[&str] =
    &["transaction type", "debit/credit", "dr/cr", "type", "direction", "indicator"];

/// Finds the column whose header matches one of `names`.
///
/// Exact matches are preferred over partial ones, so a file with both "Date"
/// and "Date Posted" does not resolve to whichever came first.
fn find_column(headers: &[String], names: &[&str], taken: &[usize]) -> Option<usize> {
    let lowered: Vec<String> = headers.iter().map(|h| h.trim().to_lowercase()).collect();

    for name in names {
        for (index, header) in lowered.iter().enumerate() {
            if header == name && !taken.contains(&index) {
                return Some(index);
            }
        }
    }
    for name in names {
        for (index, header) in lowered.iter().enumerate() {
            if header.contains(name) && !taken.contains(&index) {
                return Some(index);
            }
        }
    }
    None
}

/// Works out how to read a file, and what still has to be asked.
pub fn plan(table: &Table) -> Plan {
    let mut taken: Vec<usize> = Vec::new();

    let date = find_column(&table.headers, DATE_NAMES, &taken);
    taken.extend(date);

    // Debit and credit are looked for before a plain amount column, because a
    // file with both "Debit" and "Credit" should not have one of them
    // mistaken for the amount.
    let debit = find_column(&table.headers, DEBIT_NAMES, &taken);
    let credit = find_column(&table.headers, CREDIT_NAMES, &taken);
    if let (Some(d), Some(c)) = (debit, credit) {
        taken.push(d);
        taken.push(c);
    }

    let amount = find_column(&table.headers, AMOUNT_NAMES, &taken);
    taken.extend(amount);

    let kind = find_column(&table.headers, KIND_NAMES, &taken);
    taken.extend(kind);

    let category = find_column(&table.headers, CATEGORY_NAMES, &taken);
    taken.extend(category);

    let account = find_column(&table.headers, ACCOUNT_NAMES, &taken);
    taken.extend(account);

    // The merchant is looked for last, because its candidate names are the
    // loosest and would otherwise swallow a column that is really something
    // else.
    let merchant = find_column(&table.headers, MERCHANT_NAMES, &taken);

    let mapping = Mapping { date, merchant, category, account };

    Plan {
        mapping,
        date: guess_date_format(table, date),
        amount: guess_amount_source(table, debit, credit, amount, kind),
    }
}

/// Decides how the amount is carried, from the columns and their contents.
fn guess_amount_source(
    table: &Table,
    debit: Option<usize>,
    credit: Option<usize>,
    amount: Option<usize>,
    kind: Option<usize>,
) -> AmountGuess {
    if let (Some(debit), Some(credit)) = (debit, credit) {
        return AmountGuess::Certain(AmountSource::DebitCredit { debit, credit });
    }

    let Some(amount) = amount.or(debit).or(credit) else {
        return AmountGuess::Unreadable;
    };

    // A single column with any negative value in it is carrying its own
    // direction, whatever else the file has.
    let any_negative = table
        .rows
        .iter()
        .filter_map(|row| row.get(amount))
        .filter_map(|value| parse_signed_cents(value))
        .any(|(cents, _)| cents < 0);
    if any_negative {
        return AmountGuess::Certain(AmountSource::Signed { amount });
    }

    // Otherwise a column that says debit or credit is what settles it.
    if let Some(kind) = kind
        && table
            .rows
            .iter()
            .filter_map(|row| row.get(kind))
            .any(|value| read_direction(value).is_some())
    {
        return AmountGuess::Certain(AmountSource::Labelled { amount, kind });
    }

    // Nothing left to go on. Assuming "expense" here would turn a salary into
    // a cost without saying so.
    AmountGuess::NeedsDirection { amount }
}

/// Reads a "Debit"/"Credit" style value.
///
/// Returns `Some(true)` for income. Deliberately narrow: a value this does not
/// recognise leaves the direction unknown rather than defaulting.
pub fn read_direction(value: &str) -> Option<bool> {
    let cleaned = value.trim().to_lowercase();
    match cleaned.as_str() {
        "" => None,
        "c" | "cr" | "credit" | "deposit" | "income" | "in" | "payment in" | "received" => {
            Some(true)
        }
        "d" | "dr" | "debit" | "withdrawal" | "expense" | "out" | "payment" | "payment out"
        | "purchase" | "paid" => Some(false),
        other => {
            // Some exports write "DEBIT CARD PURCHASE" or "Credit Transfer".
            if other.starts_with("credit") || other.starts_with("deposit") {
                Some(true)
            } else if other.starts_with("debit") || other.starts_with("withdraw") {
                Some(false)
            } else {
                None
            }
        }
    }
}

/// Examines a whole date column to decide its order.
fn guess_date_format(table: &Table, date: Option<usize>) -> DateGuess {
    let Some(index) = date else {
        return DateGuess::Unreadable;
    };

    let values: Vec<&str> = table
        .rows
        .iter()
        .filter_map(|row| row.get(index))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .collect();

    if values.is_empty() {
        return DateGuess::Unreadable;
    }

    let mut saw_numeric = false;
    let mut day_first_only = false;
    let mut month_first_only = false;
    let mut readable = 0usize;

    for value in &values {
        if let Some((a, b, _)) = numeric_parts(value) {
            saw_numeric = true;
            readable += 1;
            // A first part above 12 cannot be a month, and vice versa. One
            // such row settles the whole column.
            if a > 12 {
                day_first_only = true;
            }
            if b > 12 {
                month_first_only = true;
            }
        } else if iso_date(value).is_some() || named_date(value).is_some() {
            // Readable, and in a shape that cannot be ambiguous.
            readable += 1;
        }
    }

    if readable == 0 {
        return DateGuess::Unreadable;
    }

    if !saw_numeric {
        // Every readable value was ISO or had a month name, so there is
        // nothing to be ambiguous about.
        return if values.iter().any(|v| iso_date(v).is_some()) {
            DateGuess::Certain(DateFormat::Iso)
        } else {
            DateGuess::Certain(DateFormat::Named)
        };
    }

    match (day_first_only, month_first_only) {
        // A file claiming both is contradictory; treat it as needing an answer
        // rather than silently picking one.
        (true, true) => DateGuess::Ambiguous,
        (true, false) => DateGuess::Certain(DateFormat::DayFirst),
        (false, true) => DateGuess::Certain(DateFormat::MonthFirst),
        (false, false) => DateGuess::Ambiguous,
    }
}

// ---- reading values -------------------------------------------------------

/// Splits `03/04/2026` or `3-4-26` into its two leading numbers and the year.
///
/// Returns `None` for an ISO date, which is handled separately and is never
/// ambiguous.
fn numeric_parts(value: &str) -> Option<(u32, u32, i32)> {
    let separator = value.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let parts: Vec<&str> = value.split(separator).map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    // A four-digit first part is a year: that is an ISO date, not this shape.
    if parts[0].len() == 4 {
        return None;
    }
    if !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }

    let a: u32 = parts[0].parse().ok()?;
    let b: u32 = parts[1].parse().ok()?;
    let year = expand_year(parts[2])?;
    if a == 0 || b == 0 || a > 31 || b > 31 {
        return None;
    }
    Some((a, b, year))
}

/// `26` becomes 2026; `1998` stays 1998.
///
/// A two-digit year is read as this century, which for a bank statement is
/// right for anything a user is likely to import.
fn expand_year(text: &str) -> Option<i32> {
    let year: i32 = text.parse().ok()?;
    match text.len() {
        4 => Some(year),
        2 => Some(2000 + year),
        _ => None,
    }
}

/// Reads `YYYY-MM-DD` or `YYYY/MM/DD`.
fn iso_date(value: &str) -> Option<String> {
    let normalised = value.trim().replace('/', "-");
    let parts: Vec<&str> = normalised.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    build_date(year, month, day)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Reads `15 Jan 2026`, `Jan 15, 2026` or `15-Jan-26`.
fn named_date(value: &str) -> Option<String> {
    let cleaned = value.trim().replace(',', " ").replace(['-', '/'], " ");
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }

    let month_at = parts.iter().position(|p| month_number(p).is_some())?;
    let month = month_number(parts[month_at])?;
    let others: Vec<&str> = parts
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != month_at)
        .map(|(_, value)| *value)
        .collect();

    // A four-digit number is the year. Otherwise the day comes first, which
    // covers both `15-Jan-26` and `Jan 15 26`: in the second the month has
    // already been removed, leaving the day ahead of the year either way.
    let (first, second) = (others[0], others[1]);
    let (day_text, year_text) =
        if first.len() == 4 { (second, first) } else { (first, second) };

    let day: u32 = day_text.parse().ok()?;
    let year = expand_year(year_text)?;
    build_date(year, month, day)
}

fn month_number(text: &str) -> Option<u32> {
    let lowered = text.trim().to_lowercase();
    if lowered.len() < 3 {
        return None;
    }
    MONTHS.iter().position(|m| lowered.starts_with(m)).map(|index| index as u32 + 1)
}

/// Builds `YYYY-MM-DD`, rejecting a date that doesn't exist.
fn build_date(year: i32, month: u32, day: u32) -> Option<String> {
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    if day > crate::period::days_in_month(year, month) {
        return None;
    }
    if !(1900..=2200).contains(&year) {
        return None;
    }
    let formatted = format!("{year:04}-{month:02}-{day:02}");
    // One last check through the same splitter the rest of the app uses.
    split_date(&formatted).map(|_| formatted)
}

/// Normalises one date value, given the column's established order.
pub fn normalize_date(value: &str, format: DateFormat) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    // An ISO value is read as ISO whatever the column's order, because
    // `2026-03-04` cannot mean anything else. Statements do mix formats.
    if let Some(date) = iso_date(value) {
        return Some(date);
    }
    if let Some(date) = named_date(value) {
        return Some(date);
    }

    let (a, b, year) = numeric_parts(value)?;
    match format {
        DateFormat::Iso | DateFormat::Named | DateFormat::DayFirst => build_date(year, b, a),
        DateFormat::MonthFirst => build_date(year, a, b),
    }
}

/// Reads an amount, keeping its sign.
///
/// Handles currency symbols, thousands separators, a trailing or leading
/// minus, accounting parentheses, and a trailing `CR`/`DR`. Returns cents and
/// whether the value was explicitly marked as a credit.
pub fn parse_signed_cents(value: &str) -> Option<(i64, bool)> {
    let mut text = value.trim().to_string();
    if text.is_empty() {
        return None;
    }

    let mut negative = false;
    let mut marked_credit = false;

    // A trailing CR or DR overrides any sign on the number.
    let lowered = text.to_lowercase();
    if let Some(stripped) = lowered.strip_suffix("cr") {
        marked_credit = true;
        text = stripped.trim().to_string();
    } else if let Some(stripped) = lowered.strip_suffix("dr") {
        negative = true;
        text = stripped.trim().to_string();
    }

    // Accounting style: (1,234.56) is negative.
    let trimmed = text.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        negative = true;
        text = trimmed[1..trimmed.len() - 1].to_string();
    }

    let mut digits = String::new();
    for ch in text.chars() {
        match ch {
            '-' | '\u{2212}' => negative = true,
            '+' => {}
            '0'..='9' | '.' => digits.push(ch),
            ',' | ' ' | '\u{a0}' | '\'' => {}
            // A currency symbol or code is ignored; anything else means this
            // is not a number.
            '$' | '£' | '€' | '¥' | '₹' => {}
            c if c.is_alphabetic() => return None,
            _ => return None,
        }
    }

    if digits.is_empty() || digits == "." {
        return None;
    }
    // More than one decimal point is not a number.
    if digits.matches('.').count() > 1 {
        return None;
    }

    let (whole, fraction) = match digits.split_once('.') {
        None => (digits.as_str(), ""),
        Some((whole, fraction)) => (whole, fraction),
    };
    // A statement with three decimal places is not something to round
    // silently.
    if fraction.len() > 2 {
        return None;
    }

    let units: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let cents: i64 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<i64>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    let total = units.checked_mul(100)?.checked_add(cents)?;
    let signed = if negative { -total } else { total };
    Some((signed, marked_credit))
}

/// Reads a date that can only mean one thing.
///
/// For a lone date with no column of others to compare it against — a captured
/// group from a receipt, a date typed into a form. ISO and month-name forms
/// are read; a numeric form is read only when one of its parts is above 12 and
/// so cannot be a month.
///
/// `03/04/2026` therefore returns `None`. It is the 3rd of April or the 4th of
/// March, nothing in the string says which, and resolving it to one of the two
/// would put a transaction a month out without ever mentioning it.
///
/// ```
/// # use myledger_core::csv::normalize_date_unambiguous;
/// assert_eq!(normalize_date_unambiguous("2026-04-03").as_deref(), Some("2026-04-03"));
/// assert_eq!(normalize_date_unambiguous("25/09/2026").as_deref(), Some("2026-09-25"));
/// assert_eq!(normalize_date_unambiguous("03/04/2026"), None);
/// ```
pub fn normalize_date_unambiguous(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(date) = iso_date(value) {
        return Some(date);
    }
    if let Some(date) = named_date(value) {
        return Some(date);
    }

    let (a, b, year) = numeric_parts(value)?;
    match (a > 12, b > 12) {
        // A part above 12 cannot be a month, so the other one is.
        (true, false) => build_date(year, b, a),
        (false, true) => build_date(year, a, b),
        // Both above 12, or neither: unreadable either way.
        _ => None,
    }
}

// ---- turning the table into rows ------------------------------------------

/// Reads every data row, given a settled plan.
///
/// Rows that cannot be read are collected in `skipped` with a reason. No
/// placeholder transaction is ever created for one.
pub fn read_rows(
    table: &Table,
    mapping: Mapping,
    date_format: DateFormat,
    amount: AmountSource,
) -> ReadRows {
    let mut out = ReadRows::default();

    for (offset, row) in table.rows.iter().enumerate() {
        // +2: one for the header row, one to count from 1.
        let line = offset + 2;
        let field = |index: Option<usize>| -> Option<String> {
            index
                .and_then(|i| row.get(i))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        let Some(raw_date) = field(mapping.date) else {
            out.skipped.push(SkippedRow { line, reason: "no date".to_string() });
            continue;
        };
        let Some(date) = normalize_date(&raw_date, date_format) else {
            out.skipped
                .push(SkippedRow { line, reason: format!("unreadable date {raw_date:?}") });
            continue;
        };

        let Some((amount_cents, is_income)) = read_amount(row, amount) else {
            out.skipped.push(SkippedRow { line, reason: "no readable amount".to_string() });
            continue;
        };
        if amount_cents == 0 {
            // A zero-value line is a fee reversal or a balance marker, not a
            // transaction. Importing it would add noise with no money in it.
            out.skipped.push(SkippedRow { line, reason: "amount is zero".to_string() });
            continue;
        }

        // A statement row with no description still happened, so it is
        // imported under a name that says so rather than being dropped.
        let merchant = field(mapping.merchant).unwrap_or_else(|| "Unknown".to_string());

        out.rows.push(ImportRow {
            line,
            date,
            merchant,
            amount_cents,
            is_income,
            category: field(mapping.category),
            account: field(mapping.account),
        });
    }

    out
}

/// Reads one row's amount and direction.
fn read_amount(row: &[String], source: AmountSource) -> Option<(i64, bool)> {
    match source {
        AmountSource::DebitCredit { debit, credit } => {
            let out = row.get(debit).and_then(|v| parse_signed_cents(v));
            let into = row.get(credit).and_then(|v| parse_signed_cents(v));
            match (out, into) {
                // Both filled: the larger wins, since one is usually a zero
                // placeholder.
                (Some((d, _)), Some((c, _))) if d.abs() > 0 && c.abs() > 0 => {
                    if d.abs() >= c.abs() {
                        Some((d.abs(), false))
                    } else {
                        Some((c.abs(), true))
                    }
                }
                (Some((d, _)), _) if d != 0 => Some((d.abs(), false)),
                (_, Some((c, _))) if c != 0 => Some((c.abs(), true)),
                _ => None,
            }
        }
        AmountSource::Signed { amount } => {
            let (cents, marked_credit) = row.get(amount).and_then(|v| parse_signed_cents(v))?;
            // A `CR` suffix beats the sign, because that is what it is for.
            let is_income = if marked_credit { true } else { cents > 0 };
            Some((cents.abs(), is_income))
        }
        AmountSource::Labelled { amount, kind } => {
            let (cents, marked_credit) = row.get(amount).and_then(|v| parse_signed_cents(v))?;
            let direction = row.get(kind).and_then(|v| read_direction(v));
            let is_income = direction.unwrap_or(marked_credit || cents > 0);
            Some((cents.abs(), is_income))
        }
        AmountSource::Fixed { amount, is_income } => {
            // The user's answer governs, except where the row contradicts it
            // outright: an explicit minus or a `CR` marker in the file is
            // better evidence than a blanket choice made about the file.
            let (cents, marked_credit) = row.get(amount).and_then(|v| parse_signed_cents(v))?;
            let direction = if marked_credit {
                true
            } else if cents < 0 {
                false
            } else {
                is_income
            };
            Some((cents.abs(), direction))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the reader ----------------------------------------------------

    #[test]
    fn a_plain_file_reads_into_headers_and_rows() {
        let table = read_csv("Date,Description,Amount\n2026-10-01,Corner Grocer,-12.50\n").unwrap();
        assert_eq!(table.headers, ["Date", "Description", "Amount"]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0], ["2026-10-01", "Corner Grocer", "-12.50"]);
        assert_eq!(table.delimiter, ',');
    }

    #[test]
    fn quoted_fields_may_contain_the_delimiter() {
        let table =
            read_csv("Date,Description\n2026-10-01,\"Grocer, Corner\"\n").unwrap();
        assert_eq!(table.rows[0][1], "Grocer, Corner");
    }

    #[test]
    fn a_doubled_quote_inside_a_quoted_field_is_one_quote() {
        let table = read_csv("A\n\"say \"\"hello\"\"\"\n").unwrap();
        assert_eq!(table.rows[0][0], r#"say "hello""#);
    }

    #[test]
    fn a_quoted_field_may_span_lines() {
        let table = read_csv("A,B\n\"two\nlines\",x\n").unwrap();
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][0], "two\nlines");
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        let table = read_csv("A,B\r\n1,2\r\n3,4\r\n").unwrap();
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[1], ["3", "4"]);
    }

    #[test]
    fn a_byte_order_mark_does_not_become_part_of_the_first_header() {
        // Excel writes one, and it is how "Date" quietly stops matching.
        let table = read_csv("\u{feff}Date,Amount\n2026-10-01,5\n").unwrap();
        assert_eq!(table.headers[0], "Date");
        assert!(plan(&table).mapping.date.is_some());
    }

    #[test]
    fn semicolons_and_tabs_are_recognised_as_delimiters() {
        let table = read_csv("Date;Description;Amount\n2026-10-01;Shop;-5,00\n").unwrap();
        assert_eq!(table.delimiter, ';');
        assert_eq!(table.headers.len(), 3);

        let table = read_csv("Date\tDescription\n2026-10-01\tShop\n").unwrap();
        assert_eq!(table.delimiter, '\t');
    }

    #[test]
    fn short_and_long_rows_are_squared_off_to_the_header_count() {
        let table = read_csv("A,B,C\n1\n1,2,3,4\n").unwrap();
        assert_eq!(table.rows[0], ["1", "", ""]);
        assert_eq!(table.rows[1].len(), 3);
    }

    #[test]
    fn blank_lines_are_not_rows() {
        let table = read_csv("A,B\n1,2\n\n3,4\n\n").unwrap();
        assert_eq!(table.rows.len(), 2);
    }

    #[test]
    fn a_file_with_no_final_newline_still_has_its_last_row() {
        let table = read_csv("A,B\n1,2").unwrap();
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn an_empty_file_is_an_error_rather_than_an_empty_import() {
        assert_eq!(read_csv(""), Err(CsvError::Empty));
        assert_eq!(read_csv("   \n  \n"), Err(CsvError::Empty));
    }

    #[test]
    fn an_unclosed_quote_is_reported_with_its_line() {
        let error = read_csv("A,B\n1,\"never closed\n2,3\n").unwrap_err();
        assert_eq!(error, CsvError::UnclosedQuote { line: 2 });
        assert!(error.to_string().contains("line 2"));
    }

    // ---- amounts -------------------------------------------------------

    #[test]
    fn amounts_are_read_with_their_sign() {
        assert_eq!(parse_signed_cents("12.50"), Some((1250, false)));
        assert_eq!(parse_signed_cents("-12.50"), Some((-1250, false)));
        assert_eq!(parse_signed_cents("+12.50"), Some((1250, false)));
    }

    #[test]
    fn the_decorations_banks_add_are_ignored() {
        assert_eq!(parse_signed_cents("$1,234.56"), Some((123_456, false)));
        assert_eq!(parse_signed_cents("£1 234.56"), Some((123_456, false)));
        assert_eq!(parse_signed_cents("  -£12.00  "), Some((-1200, false)));
    }

    #[test]
    fn accounting_parentheses_mean_negative() {
        assert_eq!(parse_signed_cents("(1,234.56)"), Some((-123_456, false)));
    }

    #[test]
    fn a_cr_suffix_marks_a_credit_and_dr_a_debit() {
        assert_eq!(parse_signed_cents("12.50 CR"), Some((1250, true)));
        assert_eq!(parse_signed_cents("12.50 DR"), Some((-1250, false)));
    }

    #[test]
    fn a_value_with_three_decimal_places_is_refused_not_rounded() {
        assert_eq!(parse_signed_cents("1.239"), None);
    }

    #[test]
    fn non_numbers_are_refused() {
        for bad in ["", "   ", "n/a", "pending", "-", "..", "1.2.3", "abc"] {
            assert_eq!(parse_signed_cents(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn one_decimal_place_is_tenths_here_too() {
        assert_eq!(parse_signed_cents("12.5"), Some((1250, false)));
    }

    // ---- directions ----------------------------------------------------

    #[test]
    fn the_usual_debit_and_credit_spellings_are_understood() {
        for credit in ["C", "CR", "credit", "Deposit", "in", "Received"] {
            assert_eq!(read_direction(credit), Some(true), "{credit}");
        }
        for debit in ["D", "DR", "debit", "Withdrawal", "out", "purchase"] {
            assert_eq!(read_direction(debit), Some(false), "{debit}");
        }
    }

    #[test]
    fn a_longer_phrase_is_read_from_its_first_word() {
        assert_eq!(read_direction("DEBIT CARD PURCHASE"), Some(false));
        assert_eq!(read_direction("Credit Transfer"), Some(true));
    }

    #[test]
    fn an_unrecognised_direction_stays_unknown_rather_than_defaulting() {
        for unknown in ["", "transfer", "misc", "?"] {
            assert_eq!(read_direction(unknown), None, "{unknown:?}");
        }
    }

    // ---- dates ---------------------------------------------------------

    #[test]
    fn iso_dates_are_read_as_themselves() {
        assert_eq!(normalize_date("2026-10-01", DateFormat::Iso), Some("2026-10-01".into()));
        assert_eq!(normalize_date("2026/10/01", DateFormat::Iso), Some("2026-10-01".into()));
    }

    #[test]
    fn an_iso_date_is_read_as_iso_whatever_the_column_order() {
        // Statements mix formats, and `2026-03-04` cannot mean anything else.
        assert_eq!(
            normalize_date("2026-03-04", DateFormat::MonthFirst),
            Some("2026-03-04".into())
        );
    }

    #[test]
    fn day_first_and_month_first_read_the_same_text_differently() {
        assert_eq!(normalize_date("03/04/2026", DateFormat::DayFirst), Some("2026-04-03".into()));
        assert_eq!(normalize_date("03/04/2026", DateFormat::MonthFirst), Some("2026-03-04".into()));
    }

    #[test]
    fn two_digit_years_become_this_century() {
        assert_eq!(normalize_date("01/10/26", DateFormat::DayFirst), Some("2026-10-01".into()));
    }

    #[test]
    fn month_names_are_read_in_either_order() {
        assert_eq!(normalize_date("15 Jan 2026", DateFormat::Named), Some("2026-01-15".into()));
        assert_eq!(normalize_date("Jan 15, 2026", DateFormat::Named), Some("2026-01-15".into()));
        assert_eq!(normalize_date("15-Jan-26", DateFormat::Named), Some("2026-01-15".into()));
    }

    #[test]
    fn a_date_that_does_not_exist_is_refused() {
        assert_eq!(normalize_date("31/02/2026", DateFormat::DayFirst), None);
        assert_eq!(normalize_date("2026-02-30", DateFormat::Iso), None);
        assert_eq!(normalize_date("00/01/2026", DateFormat::DayFirst), None);
        // 2026 is not a leap year.
        assert_eq!(normalize_date("29/02/2026", DateFormat::DayFirst), None);
        assert_eq!(normalize_date("29/02/2024", DateFormat::DayFirst), Some("2024-02-29".into()));
    }

    #[test]
    fn nonsense_in_a_date_column_is_refused() {
        for bad in ["", "pending", "13/13/2026", "2026", "--", "1/2"] {
            assert_eq!(normalize_date(bad, DateFormat::DayFirst), None, "{bad:?}");
        }
    }

    // ---- planning ------------------------------------------------------

    fn planned(csv: &str) -> (Table, Plan) {
        let table = read_csv(csv).unwrap();
        let plan = plan(&table);
        (table, plan)
    }

    #[test]
    fn a_tidy_statement_needs_no_questions() {
        let (_, plan) =
            planned("Date,Description,Amount\n2026-10-01,Grocer,-12.50\n2026-10-02,Pay,2000.00\n");
        assert!(plan.is_settled(), "{:?}", plan.questions());
        assert_eq!(plan.date, DateGuess::Certain(DateFormat::Iso));
        assert_eq!(plan.amount, AmountGuess::Certain(AmountSource::Signed { amount: 2 }));
    }

    #[test]
    fn separate_debit_and_credit_columns_are_recognised() {
        let (_, plan) = planned(
            "Date,Narrative,Debit,Credit\n2026-10-01,Grocer,12.50,\n2026-10-02,Pay,,2000.00\n",
        );
        assert!(plan.is_settled(), "{:?}", plan.questions());
        assert_eq!(
            plan.amount,
            AmountGuess::Certain(AmountSource::DebitCredit { debit: 2, credit: 3 })
        );
    }

    #[test]
    fn a_type_column_settles_an_all_positive_amount_column() {
        let (_, plan) = planned(
            "Date,Description,Amount,Transaction Type\n2026-10-01,Grocer,12.50,Debit\n\
             2026-10-02,Pay,2000.00,Credit\n",
        );
        assert!(plan.is_settled(), "{:?}", plan.questions());
        assert_eq!(
            plan.amount,
            AmountGuess::Certain(AmountSource::Labelled { amount: 2, kind: 3 })
        );
    }

    #[test]
    fn all_positive_amounts_with_no_type_column_must_be_asked_about() {
        // The case that matters: guessing "expense" here would turn a salary
        // into a cost without saying so.
        let (_, plan) =
            planned("Date,Description,Amount\n2026-10-01,Grocer,12.50\n2026-10-02,Pay,2000.00\n");
        assert_eq!(plan.amount, AmountGuess::NeedsDirection { amount: 2 });
        assert!(!plan.is_settled());
        assert!(plan.questions().iter().any(|q| q.contains("which way")));
    }

    #[test]
    fn an_ambiguous_date_column_must_be_asked_about() {
        // Every row could be read either way round.
        let (_, plan) =
            planned("Date,Description,Amount\n03/04/2026,Grocer,-12.50\n05/06/2026,Shop,-3.00\n");
        assert_eq!(plan.date, DateGuess::Ambiguous);
        assert!(!plan.is_settled());
        assert!(plan.questions().iter().any(|q| q.contains("day-first")));
    }

    #[test]
    fn one_row_with_a_day_above_twelve_settles_the_whole_column() {
        let (_, plan) =
            planned("Date,Description,Amount\n03/04/2026,Grocer,-12.50\n25/04/2026,Shop,-3.00\n");
        assert_eq!(plan.date, DateGuess::Certain(DateFormat::DayFirst));
        assert!(plan.is_settled());

        let (_, plan) =
            planned("Date,Description,Amount\n04/03/2026,Grocer,-12.50\n04/25/2026,Shop,-3.00\n");
        assert_eq!(plan.date, DateGuess::Certain(DateFormat::MonthFirst));
    }

    #[test]
    fn a_column_claiming_both_orders_is_treated_as_ambiguous() {
        // 25/04 says day-first, 04/25 says month-first. The file contradicts
        // itself, so the user decides.
        let (_, plan) =
            planned("Date,Description,Amount\n25/04/2026,A,-1.00\n04/25/2026,B,-2.00\n");
        assert_eq!(plan.date, DateGuess::Ambiguous);
    }

    #[test]
    fn the_many_names_banks_give_their_columns_are_recognised() {
        for header in ["Transaction Date", "Posting Date", "Value Date", "Date Posted"] {
            let csv = format!("{header},Narrative,Amount\n2026-10-01,Shop,-1.00\n");
            let (_, plan) = planned(&csv);
            assert_eq!(plan.mapping.date, Some(0), "{header}");
        }
        for header in ["Description", "Narrative", "Payee", "Details", "Particulars", "Memo"] {
            let csv = format!("Date,{header},Amount\n2026-10-01,Shop,-1.00\n");
            let (_, plan) = planned(&csv);
            assert_eq!(plan.mapping.merchant, Some(1), "{header}");
        }
    }

    #[test]
    fn an_exact_header_match_beats_a_merely_similar_one() {
        // A column literally named "Date" must not lose to one that only
        // happens to contain the word, whichever came first in the file.
        let (_, plan) = planned(
            "Date Range Covered,Date,Description,Amount\n2026-01-01,2026-10-01,S,-1.00\n",
        );
        assert_eq!(plan.mapping.date, Some(1));
    }

    #[test]
    fn when_several_known_date_columns_are_present_the_more_specific_wins() {
        // Statements often carry both. The order of `DATE_NAMES` is the
        // preference, so this is a stated choice rather than an accident of
        // column order: a named transaction or posting date beats a bare
        // "Date", which on such a file is usually the day it cleared.
        let (_, plan) =
            planned("Date,Transaction Date,Description,Amount\n2026-01-01,2026-10-01,S,-1.00\n");
        assert_eq!(plan.mapping.date, Some(1));

        let (_, plan) =
            planned("Posting Date,Date,Description,Amount\n2026-01-01,2026-10-01,S,-1.00\n");
        assert_eq!(plan.mapping.date, Some(0));
    }

    #[test]
    fn a_debit_column_is_not_mistaken_for_the_amount_column() {
        let (_, plan) = planned("Date,Description,Debit,Credit\n2026-10-01,S,1.00,\n");
        assert_eq!(
            plan.amount,
            AmountGuess::Certain(AmountSource::DebitCredit { debit: 2, credit: 3 })
        );
        assert_eq!(plan.mapping.merchant, Some(1));
    }

    #[test]
    fn category_and_account_columns_are_found_when_present() {
        let (_, plan) = planned(
            "Date,Description,Amount,Category,Account\n2026-10-01,S,-1.00,Groceries,Main\n",
        );
        assert_eq!(plan.mapping.category, Some(3));
        assert_eq!(plan.mapping.account, Some(4));
    }

    #[test]
    fn a_file_with_no_recognisable_columns_asks_about_all_of_them() {
        let (_, plan) = planned("Col1,Col2,Col3\na,b,c\n");
        assert!(!plan.is_settled());
        assert!(plan.questions().len() >= 3, "{:?}", plan.questions());
    }

    // ---- reading rows --------------------------------------------------

    #[test]
    fn a_signed_column_maps_negatives_to_expenses_and_positives_to_income() {
        let table =
            read_csv("Date,Description,Amount\n2026-10-01,Grocer,-12.50\n2026-10-02,Pay,2000.00\n")
                .unwrap();
        let plan = plan(&table);
        let AmountGuess::Certain(amount) = plan.amount else { panic!("{:?}", plan.amount) };
        let read = read_rows(&table, plan.mapping, DateFormat::Iso, amount);

        assert!(read.skipped.is_empty(), "{:?}", read.skipped);
        assert_eq!(read.rows.len(), 2);
        // Stored positive, with the direction carried separately.
        assert_eq!(read.rows[0].amount_cents, 1250);
        assert!(!read.rows[0].is_income);
        assert_eq!(read.rows[1].amount_cents, 200_000);
        assert!(read.rows[1].is_income);
    }

    #[test]
    fn a_statement_using_positive_debits_is_read_the_other_way_round() {
        // The sign convention is the file's, not an assumption: here positive
        // means money out, said so by the type column.
        let table = read_csv(
            "Date,Description,Amount,Type\n2026-10-01,Grocer,12.50,Debit\n\
             2026-10-02,Pay,2000.00,Credit\n",
        )
        .unwrap();
        let plan = plan(&table);
        let AmountGuess::Certain(amount) = plan.amount else { panic!("{:?}", plan.amount) };
        let read = read_rows(&table, plan.mapping, DateFormat::Iso, amount);

        assert!(!read.rows[0].is_income);
        assert_eq!(read.rows[0].amount_cents, 1250);
        assert!(read.rows[1].is_income);
    }

    #[test]
    fn debit_and_credit_columns_choose_whichever_is_filled() {
        let table = read_csv(
            "Date,Narrative,Debit,Credit\n2026-10-01,Grocer,12.50,\n2026-10-02,Pay,,2000.00\n\
             2026-10-03,Fee,3.00,0.00\n",
        )
        .unwrap();
        let plan = plan(&table);
        let AmountGuess::Certain(amount) = plan.amount else { panic!() };
        let read = read_rows(&table, plan.mapping, DateFormat::Iso, amount);

        assert_eq!(read.rows.len(), 3);
        assert!(!read.rows[0].is_income);
        assert!(read.rows[1].is_income);
        // A zero in the other column is a placeholder, not a credit.
        assert!(!read.rows[2].is_income);
        assert_eq!(read.rows[2].amount_cents, 300);
    }

    #[test]
    fn an_unreadable_row_is_skipped_with_a_reason_and_never_invented() {
        let table = read_csv(
            "Date,Description,Amount\n2026-10-01,Grocer,-12.50\nnot-a-date,Shop,-1.00\n\
             2026-10-03,Shop,pending\n,Shop,-1.00\n",
        )
        .unwrap();
        let read = read_rows(
            &table,
            Mapping { date: Some(0), merchant: Some(1), category: None, account: None },
            DateFormat::Iso,
            AmountSource::Signed { amount: 2 },
        );

        assert_eq!(read.rows.len(), 1);
        assert_eq!(read.skipped.len(), 3);
        assert_eq!(read.skipped[0].line, 3);
        assert!(read.skipped[0].reason.contains("unreadable date"));
        assert!(read.skipped[1].reason.contains("amount"));
        assert!(read.skipped[2].reason.contains("no date"));
    }

    #[test]
    fn being_told_the_direction_reads_an_all_positive_file() {
        // The answer to the question the planner asks.
        let table = read_csv("Date,Description,Amount
2026-10-01,Grocer,12.50
").unwrap();
        let mapping = Mapping { date: Some(0), merchant: Some(1), category: None, account: None };

        let expenses =
            read_rows(&table, mapping, DateFormat::Iso, AmountSource::Fixed { amount: 2, is_income: false });
        assert_eq!(expenses.rows[0].amount_cents, 1250);
        assert!(!expenses.rows[0].is_income);

        let income =
            read_rows(&table, mapping, DateFormat::Iso, AmountSource::Fixed { amount: 2, is_income: true });
        assert!(income.rows[0].is_income);
    }

    #[test]
    fn a_row_that_contradicts_the_chosen_direction_wins() {
        // The user said "all expenses", but this row is explicitly a credit.
        // What the file says about a specific row beats a blanket choice.
        let table = read_csv(
            "Date,Description,Amount
2026-10-01,Grocer,12.50
2026-10-02,Refund,8.00 CR
             2026-10-03,Pay,-5.00
",
        )
        .unwrap();
        let read = read_rows(
            &table,
            Mapping { date: Some(0), merchant: Some(1), category: None, account: None },
            DateFormat::Iso,
            AmountSource::Fixed { amount: 2, is_income: false },
        );
        assert!(!read.rows[0].is_income);
        assert!(read.rows[1].is_income, "a CR marker beats the blanket choice");
        assert!(!read.rows[2].is_income, "an explicit minus is still an expense");
    }

    #[test]
    fn a_zero_amount_row_is_skipped_rather_than_imported() {
        let table = read_csv("Date,Description,Amount\n2026-10-01,Balance carried,0.00\n").unwrap();
        let read = read_rows(
            &table,
            Mapping { date: Some(0), merchant: Some(1), category: None, account: None },
            DateFormat::Iso,
            AmountSource::Signed { amount: 2 },
        );
        assert!(read.rows.is_empty());
        assert!(read.skipped[0].reason.contains("zero"));
    }

    #[test]
    fn a_row_with_no_description_is_imported_under_a_name_that_says_so() {
        // It still happened, so dropping it would lose money from the ledger.
        let table = read_csv("Date,Description,Amount\n2026-10-01,,-12.50\n").unwrap();
        let read = read_rows(
            &table,
            Mapping { date: Some(0), merchant: Some(1), category: None, account: None },
            DateFormat::Iso,
            AmountSource::Signed { amount: 2 },
        );
        assert_eq!(read.rows.len(), 1);
        assert_eq!(read.rows[0].merchant, "Unknown");
    }

    #[test]
    fn the_line_number_reported_is_the_line_in_the_file() {
        let table = read_csv("Date,Description,Amount\n2026-10-01,A,-1.00\n2026-10-02,B,-2.00\n")
            .unwrap();
        let read = read_rows(
            &table,
            Mapping { date: Some(0), merchant: Some(1), category: None, account: None },
            DateFormat::Iso,
            AmountSource::Signed { amount: 2 },
        );
        // Line 1 is the header, so the first data row is line 2.
        assert_eq!(read.rows[0].line, 2);
        assert_eq!(read.rows[1].line, 3);
    }

    #[test]
    fn category_and_account_names_come_through_as_written() {
        let table = read_csv(
            "Date,Description,Amount,Category,Account\n2026-10-01,S,-1.00,Groceries,Main Account\n",
        )
        .unwrap();
        let plan = plan(&table);
        let AmountGuess::Certain(amount) = plan.amount else { panic!() };
        let read = read_rows(&table, plan.mapping, DateFormat::Iso, amount);
        assert_eq!(read.rows[0].category.as_deref(), Some("Groceries"));
        assert_eq!(read.rows[0].account.as_deref(), Some("Main Account"));
    }

    #[test]
    fn a_realistic_statement_reads_end_to_end() {
        // Quoted descriptions with commas, mixed signs, a trailing blank
        // line, CRLF endings, and a currency symbol.
        let csv = "Transaction Date,Description,Amount,Category\r\n\
                   01/10/2026,\"GROCER, CORNER #4411\",-£62.14,Groceries\r\n\
                   03/10/2026,TRAM PASS,-£20.00,Transport\r\n\
                   25/10/2026,ACME PAYROLL,£2000.00,Salary\r\n\
                   \r\n";
        let table = read_csv(csv).unwrap();
        let plan = plan(&table);

        assert!(plan.is_settled(), "{:?}", plan.questions());
        assert_eq!(plan.date, DateGuess::Certain(DateFormat::DayFirst));

        let AmountGuess::Certain(amount) = plan.amount else { panic!() };
        let DateGuess::Certain(format) = plan.date else { panic!() };
        let read = read_rows(&table, plan.mapping, format, amount);

        assert!(read.skipped.is_empty(), "{:?}", read.skipped);
        assert_eq!(read.rows.len(), 3);
        assert_eq!(read.rows[0].date, "2026-10-01");
        assert_eq!(read.rows[0].merchant, "GROCER, CORNER #4411");
        assert_eq!(read.rows[0].amount_cents, 6214);
        assert!(!read.rows[0].is_income);
        assert_eq!(read.rows[0].category.as_deref(), Some("Groceries"));
        assert_eq!(read.rows[2].date, "2026-10-25");
        assert!(read.rows[2].is_income);
        assert_eq!(read.rows[2].amount_cents, 200_000);
    }

    // ---- a date with nothing to compare it against ---------------------

    #[test]
    fn an_iso_or_named_date_alone_is_never_ambiguous() {
        assert_eq!(normalize_date_unambiguous("2026-04-03").as_deref(), Some("2026-04-03"));
        assert_eq!(normalize_date_unambiguous("2026/04/03").as_deref(), Some("2026-04-03"));
        assert_eq!(normalize_date_unambiguous("4 September 2026").as_deref(), Some("2026-09-04"));
        assert_eq!(normalize_date_unambiguous("Sep 4, 2026").as_deref(), Some("2026-09-04"));
    }

    #[test]
    fn a_numeric_date_alone_is_read_only_when_it_can_mean_one_thing() {
        // A part above 12 cannot be a month.
        assert_eq!(normalize_date_unambiguous("25/09/2026").as_deref(), Some("2026-09-25"));
        assert_eq!(normalize_date_unambiguous("09/25/2026").as_deref(), Some("2026-09-25"));
        assert_eq!(normalize_date_unambiguous("31-01-2026").as_deref(), Some("2026-01-31"));
    }

    #[test]
    fn a_date_that_reads_either_way_is_refused() {
        // The whole point: 03/04/2026 is two different days and nothing here
        // says which.
        for ambiguous in ["03/04/2026", "01/02/2026", "12/11/2026", "1/2/26"] {
            assert_eq!(normalize_date_unambiguous(ambiguous), None, "{ambiguous}");
        }
    }

    #[test]
    fn nonsense_alone_is_refused() {
        for bad in ["", "   ", "pending", "99/99/2026", "2026", "--"] {
            assert_eq!(normalize_date_unambiguous(bad), None, "{bad:?}");
        }
    }
}
