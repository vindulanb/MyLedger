//! Shared domain logic for MyLedger.
//!
//! This crate holds the types and pure functions that the backend and both
//! apps agree on. It performs no I/O and knows nothing about the database or
//! the HTTP layer, which keeps the rules about what a valid transaction is —
//! and what counts as a duplicate — in one testable place.
//!
//! - [`model`] — the domain records and their wire/database tokens.
//! - [`fingerprint`] — duplicate detection.
//! - [`format`] — rendering money for display.
//! - [`validate`] — server-side validation of untrusted input.
//! - [`period`] — what a selected period means in dates.
//! - [`filter`] — search and filters for the transactions list.
//! - [`csv`] — reading bank and card statements.
//! - [`textrules`] — pulling a transaction out of text, by rule.
//! - [`analysis`] — the totals, charts and counts the dashboard shows.
//! - [`budgets`] — budget and goal progress.
//! - [`recurring`] — finding the repeating charges in a transaction history.
//! - [`wire`] — the request and response shapes the API speaks in.

pub mod analysis;
pub mod budgets;
pub mod csv;
pub mod filter;
pub mod fingerprint;
pub mod format;
pub mod model;
pub mod period;
pub mod recurring;
pub mod textrules;
pub mod validate;
pub mod wire;

pub use fingerprint::transaction_fingerprint;
pub use format::{money, parse_money, signed_money};
pub use model::{
    Account, Budget, Cadence, Category, Document, DocumentExtractionRule, DocumentStatus, Goal,
    ParseEnumError, Period,
    RecurringItem, RecurringKind, Rule, Settings, SmsKindHint, SmsParseRule, Tag, Transaction,
    TransactionKind, TransactionSource, now_timestamp, timestamp_after,
};
pub use recurring::{
    Confidence, NameLookup, RecurringSuggestion, detect_recurring, normalize_merchant,
};
pub use analysis::{
    CategorySlice, MonthFlow, Names, PeriodSummary, monthly_flow, most_recent, needs_review_count,
    previous_range, spending_by_category, summarise,
};
pub use csv::{
    AmountGuess, AmountSource, CsvError, DateFormat, DateGuess, ImportRow, Mapping, Plan,
    ReadRows, SkippedRow, Table, read_csv, read_rows,
};
pub use filter::{AccountChoice, CategoryChoice, TransactionFilter};
pub use textrules::{
    Candidate, Context, MatchOutcome, RuleProblem, RuleView, check_rule, match_text,
};
pub use budgets::{BudgetHealth, BudgetProgress, GoalProgress};
pub use period::DateRange;
pub use wire::{
    BudgetInput, ErrorResponse, ExtractionResponse, GoalInput, ImportColumns, ImportCsvReport,
    ImportOptions, LoginRequest, LoginResponse, MeResponse, NamedInput, PreferencesRequest,
    KeepSuggestionRequest, PreferencesResponse, RecurringItemInput, RuleInput, StateResponse,
    StoredExtractionResponse, SuggestionsResponse, TagUsage,
};
pub use validate::{
    ValidatedTransaction, ValidationError, validate_amount_cents, validate_date, validate_kind,
    validate_merchant, validate_new_transaction, validate_transaction,
};

/// Display name of the application.
pub const APP_NAME: &str = "MyLedger";

/// Version of the JSON API the backend exposes, used to build route prefixes.
pub const API_VERSION: &str = "v1";
