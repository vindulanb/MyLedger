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
    Account, Budget, Category, Document, Goal, RecurringItem, Rule, Settings, SmsParseRule, Tag,
    Transaction,
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
