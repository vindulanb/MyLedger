//! First-run starter data.
//!
//! A brand-new ledger has no transactions, no budgets, no goals and no
//! balances — the app opens empty and stays empty until the user puts something
//! in it. What it does need is somewhere to file the first transaction, so this
//! seeds a handful of category and account **names**. Nothing else: no amounts,
//! no dates, no demo purchases. Every one of them can be renamed or deleted.
//!
//! Idempotent, and careful not to undo the user's choices. Categories and
//! accounts are only touched when there are no *live* ones left, so a user who
//! deletes "Dining" does not find it back after a restart. The one case that
//! does re-seed is having none at all: an app with nowhere to file a
//! transaction cannot be used, so that state is treated as uninitialised rather
//! than as a preference. Settings use insert-if-absent for the same reason.

use crate::error::DbError;
use crate::lookups::{accounts, categories};
use crate::settings::{self, keys};
use crate::{Db, settings::set_setting_if_absent};

/// Categories a new ledger starts with. Names only.
pub const STARTER_CATEGORIES: &[&str] = &[
    "Housing",
    "Groceries",
    "Shopping",
    "Dining",
    "Transportation",
    "Utilities",
    "Subscriptions",
    "Insurance",
    "Health",
    "Entertainment",
    "Income",
    "Needs review",
    "Other",
];

/// Accounts a new ledger starts with. Names only — no balances.
pub const STARTER_ACCOUNTS: &[&str] = &["Main Account", "Credit Card", "Cash"];

/// What a run of the seed actually did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeedOutcome {
    pub categories_inserted: usize,
    pub accounts_inserted: usize,
    pub settings_inserted: usize,
}

impl SeedOutcome {
    /// Whether this run changed anything.
    pub fn is_empty(self) -> bool {
        self.categories_inserted == 0
            && self.accounts_inserted == 0
            && self.settings_inserted == 0
    }
}

/// Puts the starter lookup names and default settings in place.
///
/// Call on every startup; it does nothing on all runs but the first.
pub async fn seed_starter_data(db: &Db) -> Result<SeedOutcome, DbError> {
    let mut outcome = SeedOutcome::default();

    // Live rows, not total rows: soft-deleting every category leaves the table
    // full and the app unusable, which is exactly the state worth fixing.
    if categories::list_categories(db).await?.is_empty() {
        for name in STARTER_CATEGORIES {
            categories::insert_category(db, name).await?;
            outcome.categories_inserted += 1;
        }
    }

    if accounts::list_accounts(db).await?.is_empty() {
        for name in STARTER_ACCOUNTS {
            accounts::insert_account(db, name).await?;
            outcome.accounts_inserted += 1;
        }
    }

    let defaults = [
        (keys::SELECTED_PERIOD, "all-time"),
        (keys::NET_WORTH_CONFIGURED, "false"),
        (keys::ASSETS_CENTS, "0"),
        (keys::LIABILITIES_CENTS, "0"),
    ];
    for (key, value) in defaults {
        if set_setting_if_absent(db, key, value).await? {
            outcome.settings_inserted += 1;
        }
    }

    Ok(outcome)
}

/// Forces the net-worth and period settings back to their defaults.
///
/// Used by the data wipe, which unlike the seed genuinely means to overwrite
/// whatever is there.
pub async fn reset_settings_to_defaults(db: &Db) -> Result<(), DbError> {
    settings::set_setting(db, keys::SELECTED_PERIOD, "all-time").await?;
    settings::set_setting(db, keys::NET_WORTH_CONFIGURED, "false").await?;
    settings::set_setting(db, keys::ASSETS_CENTS, "0").await?;
    settings::set_setting(db, keys::LIABILITIES_CENTS, "0").await?;
    Ok(())
}
