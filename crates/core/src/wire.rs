//! The shapes that travel between the server and its clients.
//!
//! These live in `core` rather than in the server because they are a contract,
//! not an implementation detail: the server serialises them and the web and
//! mobile apps deserialise them. One definition means a field renamed on one
//! side cannot silently become `None` on the other — it stops compiling
//! instead.
//!
//! Only shapes with more than one reader belong here. A response read by
//! nothing but its own endpoint stays with that endpoint.

use serde::{Deserialize, Serialize};

use crate::model::{
    Account, Budget, Category, Document, Goal, Period, RecurringItem, Rule, Settings, SmsParseRule,
    Tag, Transaction,
};

/// Everything a client needs, as returned by `GET /api/v1/state`.
///
/// Clients are offline-tolerant and keep a local copy of the ledger, so they
/// reconcile against one large read rather than a dozen small ones. That also
/// means a client can never hold a half-refreshed view where transactions have
/// been updated but the categories they name have not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateResponse {
    /// Newest first, capped by the server.
    pub transactions: Vec<Transaction>,
    /// Whether the cap was hit, so a client knows it is looking at a window
    /// rather than the whole history.
    pub transactions_truncated: bool,

    /// Lookups include soft-deleted rows, each carrying its `deleted_at`.
    /// Filter on `deleted_at == None` to build a picker; keep the rest so old
    /// transactions still resolve to a name.
    pub categories: Vec<Category>,
    pub accounts: Vec<Account>,
    pub tags: Vec<Tag>,

    pub rules: Vec<Rule>,
    pub sms_parse_rules: Vec<SmsParseRule>,
    pub budgets: Vec<Budget>,
    pub goals: Vec<Goal>,
    pub recurring_items: Vec<RecurringItem>,
    /// Normalised merchants whose suggestions the user has dismissed.
    pub dismissed_patterns: Vec<String>,

    /// Metadata only — never bytes, never on-disk paths.
    pub documents: Vec<Document>,

    pub settings: Settings,
}

impl StateResponse {
    /// The categories a user may still file something under.
    pub fn live_categories(&self) -> impl Iterator<Item = &Category> {
        self.categories.iter().filter(|c| c.deleted_at.is_none())
    }

    /// The accounts a user may still file something under.
    pub fn live_accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(|a| a.deleted_at.is_none())
    }

    /// The tags a user may still attach.
    pub fn live_tags(&self) -> impl Iterator<Item = &Tag> {
        self.tags.iter().filter(|t| t.deleted_at.is_none())
    }

    /// The name of a category, including one since deleted, so that an old
    /// transaction still renders with a name rather than a bare ID.
    pub fn category_name(&self, id: uuid::Uuid) -> Option<&str> {
        self.categories.iter().find(|c| c.id == id).map(|c| c.name.as_str())
    }

    /// The name of an account, including one since deleted.
    pub fn account_name(&self, id: uuid::Uuid) -> Option<&str> {
        self.accounts.iter().find(|a| a.id == id).map(|a| a.name.as_str())
    }

    /// The name of a tag, including one since deleted.
    pub fn tag_name(&self, id: uuid::Uuid) -> Option<&str> {
        self.tags.iter().find(|t| t.id == id).map(|t| t.name.as_str())
    }
}

/// Body of `POST /api/v1/auth/login`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Reply to a successful login. The session itself arrives as a cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub username: String,
    /// When the session will lapse if left unused.
    pub expires_at: String,
}

/// Reply to `GET /api/v1/me` — who the caller is, and how they proved it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeResponse {
    pub username: String,
    /// `session` or `device_token`.
    pub authenticated_via: String,
    /// The device's label, when signed in as one.
    pub device_label: Option<String>,
}

/// The body every API failure is rendered as.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub status: u16,
    /// A stable slug a client can branch on without parsing prose.
    pub error: String,
    pub message: String,
}

// ---- PUT /api/v1/preferences -------------------------------------------
//
// These are inputs, so they live here for the same reason the responses do:
// the web app constructs them and the server parses them. A field renamed on
// one side has to stop compiling on the other, which it cannot do if each
// side declares its own copy of the shape.
//
// Every group is `Option`, and that distinction carries meaning the server
// relies on: a group that is absent is left alone, while a group that is
// present is taken as the complete list, so an item missing from it is
// deleted. `None` and `Some(vec![])` are therefore very different requests —
// the second empties the group.

fn yes() -> bool {
    true
}

/// A category, account or tag as a client holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedInput {
    /// Absent for something newly created in the UI and not yet saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub name: String,
}

impl NamedInput {
    /// An existing row, unchanged.
    pub fn existing(id: uuid::Uuid, name: impl Into<String>) -> Self {
        Self { id: Some(id), name: name.into() }
    }

    /// Something the user has just typed.
    pub fn new_named(name: impl Into<String>) -> Self {
        Self { id: None, name: name.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub when_text: String,
    #[serde(default)]
    pub then_category_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub then_tag_id: Option<uuid::Uuid>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    #[serde(default)]
    pub category_id: Option<uuid::Uuid>,
    pub monthly_limit_cents: i64,
    #[serde(default = "yes")]
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub name: String,
    pub target_cents: i64,
    #[serde(default)]
    pub current_cents: i64,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringItemInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    pub name: String,
    #[serde(default)]
    pub category_id: Option<uuid::Uuid>,
    pub amount_cents: i64,
    pub cadence: crate::model::Cadence,
    #[serde(default)]
    pub next_date: Option<String>,
    #[serde(default)]
    pub account_id: Option<uuid::Uuid>,
    pub kind: crate::model::RecurringKind,
    #[serde(default = "yes")]
    pub active: bool,
}

/// Any subset of the user's preferences.
///
/// Built by [`PreferencesRequest::default`] and then filled in with only the
/// groups being changed, so a screen that edits budgets cannot accidentally
/// clear the user's categories by omitting them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreferencesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<NamedInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<NamedInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<NamedInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RuleInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Vec<BudgetInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Vec<GoalInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring_items: Option<Vec<RecurringItemInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liabilities_cents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_worth_configured: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_period: Option<Period>,
}

/// The preferences after the update.
///
/// Transactions and documents are excluded — this is the small half of the
/// state, and returning it saves the client a follow-up read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferencesResponse {
    /// Which groups this request actually touched.
    pub updated: Vec<String>,
    pub categories: Vec<Category>,
    pub accounts: Vec<Account>,
    pub tags: Vec<Tag>,
    pub rules: Vec<Rule>,
    pub budgets: Vec<Budget>,
    pub goals: Vec<Goal>,
    pub recurring_items: Vec<RecurringItem>,
    pub dismissed_patterns: Vec<String>,
    pub settings: Settings,
}

// ---- POST /api/v1/import/csv -------------------------------------------

/// Answers to the questions a statement can raise, sent as a JSON `options`
/// field alongside the file.
///
/// Every field is optional, and a tidy statement needs none of them: the web
/// app fills in only what it had to ask the user about. `deny_unknown_fields`
/// so a typo in an option fails loudly instead of silently changing nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// `iso`, `day-first`, `month-first` or `named`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    /// Which way rows go when the file itself does not say: `expense` or
    /// `income`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// Column overrides, by zero-based index.
    #[serde(default)]
    pub columns: ImportColumns,
}

/// Which column holds what, when the user says rather than the parser.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportColumns {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<usize>,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCsvReport {
    pub inserted: usize,
    /// Rows already in the ledger, matched by fingerprint.
    pub duplicates: usize,
    /// Rows the parser could not read, plus rows the server refused.
    pub skipped: usize,
    /// How many inserted rows ended up filed under `Needs review`.
    pub needs_review: usize,
    /// Total data rows in the file.
    pub rows_in_file: usize,
    /// Why each skipped row was skipped, capped so one bad file cannot
    /// produce a response larger than the file itself.
    pub skipped_detail: Vec<SkippedDetail>,
    /// The column names as read, so a client can show what it imported from.
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedDetail {
    /// 1-based line in the file.
    pub line: usize,
    pub reason: String,
}
