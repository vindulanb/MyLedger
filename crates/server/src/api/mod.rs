//! The data API.
//!
//! Three groups of endpoints, sharing one set of rules about what a valid
//! reference is:
//!
//! - [`state`] — one read that gives a client everything it needs.
//! - [`transactions`] — writing to the ledger, one row or a whole import.
//! - [`preferences`] — everything that isn't a transaction or a document.
//!
//! Anything naming a category, account or tag by ID is checked against
//! [`Lookups`] before it reaches SQL. The foreign keys would catch a bad
//! reference anyway, but as a 500 rather than as an answer the user can act on.

pub mod documents;
pub mod import;
pub mod preferences;
pub mod recurring;
pub mod sms_rules;
pub mod state;
pub mod transactions;

use myledger_core::{Account, Category, Tag};
use myledger_db::Db;
use myledger_db::lookups::{accounts, categories, tags};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use crate::error::ApiError;

/// The categories, accounts and tags, loaded once per request.
///
/// Holds soft-deleted rows too. A transaction filed last year against a
/// category since deleted still has to render with its name, so lookups are
/// resolved across everything — while *assigning* one is restricted to rows
/// that are still live.
pub struct Lookups {
    pub accounts: Vec<Account>,
    pub categories: Vec<Category>,
    pub tags: Vec<Tag>,
}

impl Lookups {
    pub async fn load(db: &Db) -> Result<Self, ApiError> {
        Ok(Self {
            accounts: accounts::list_all_accounts(db).await?,
            categories: categories::list_all_categories(db).await?,
            tags: tags::list_all_tags(db).await?,
        })
    }

    /// The name of an account, deleted or not.
    pub fn account_name(&self, id: Uuid) -> Option<&str> {
        self.accounts.iter().find(|a| a.id == id).map(|a| a.name.as_str())
    }

    /// A live account with this name, ignoring case and surrounding space.
    ///
    /// Imports identify an account by what the bank called it.
    pub fn live_account_named(&self, name: &str) -> Option<&Account> {
        let wanted = name.trim();
        self.accounts
            .iter()
            .find(|a| a.deleted_at.is_none() && a.name.eq_ignore_ascii_case(wanted))
    }

    /// Checks that an account may be assigned to.
    pub fn check_account(&self, id: Uuid) -> Result<(), ApiError> {
        match self.accounts.iter().find(|a| a.id == id) {
            Some(account) if account.deleted_at.is_none() => Ok(()),
            Some(_) => Err(ApiError::bad_request(format!("account {id} has been deleted"))),
            None => Err(ApiError::bad_request(format!("no account with id {id}"))),
        }
    }

    /// Checks that a category may be assigned to.
    pub fn check_category(&self, id: Uuid) -> Result<(), ApiError> {
        match self.categories.iter().find(|c| c.id == id) {
            Some(category) if category.deleted_at.is_none() => Ok(()),
            Some(_) => Err(ApiError::bad_request(format!("category {id} has been deleted"))),
            None => Err(ApiError::bad_request(format!("no category with id {id}"))),
        }
    }

    /// Checks that every tag in a list may be assigned to.
    pub fn check_tags(&self, ids: &[Uuid]) -> Result<(), ApiError> {
        for id in ids {
            match self.tags.iter().find(|t| t.id == *id) {
                Some(tag) if tag.deleted_at.is_none() => {}
                Some(_) => {
                    return Err(ApiError::bad_request(format!("tag {id} has been deleted")));
                }
                None => return Err(ApiError::bad_request(format!("no tag with id {id}"))),
            }
        }
        Ok(())
    }
}

/// Deserialises a field that may be absent, `null`, or a value — keeping those
/// three cases apart.
///
/// A `PATCH` needs the distinction: leaving `category_id` out means "don't
/// touch it", and sending `null` means "clear it". Plain `Option` collapses
/// both into `None`.
pub fn explicit_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// Trims a name and refuses an empty one.
pub fn clean_name(raw: &str, what: &str) -> Result<String, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request(format!("a {what} needs a name")));
    }
    Ok(trimmed.to_string())
}

/// Refuses a negative amount, for the fields where zero is meaningful but
/// below zero is not — a budget ceiling, a savings target.
pub fn check_not_negative(amount_cents: i64, what: &str) -> Result<i64, ApiError> {
    if amount_cents < 0 {
        return Err(ApiError::bad_request(format!("{what} cannot be negative")));
    }
    Ok(amount_cents)
}

/// Normalises an optional date field: blank becomes absent, anything else has
/// to be a real `YYYY-MM-DD`.
pub fn clean_date(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    myledger_core::validate_date(value).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Some(value.to_string()))
}

/// The checks a recurring item has to pass however it was created — accepted
/// from a suggestion, added by hand, or synced through preferences.
///
/// Returns the cleaned name and next date.
pub fn clean_recurring_fields(
    name: &str,
    amount_cents: i64,
    next_date: Option<&str>,
) -> Result<(String, Option<String>), ApiError> {
    let name = clean_name(name, "recurring item")?;
    if amount_cents <= 0 {
        return Err(ApiError::bad_request("a recurring charge must be more than zero"));
    }
    Ok((name, clean_date(next_date)?))
}
