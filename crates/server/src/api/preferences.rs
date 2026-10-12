//! `PUT /api/v1/preferences` — everything that isn't a transaction.
//!
//! Each group is optional. A group that is absent is not read, not written and
//! not touched; a group that is present is *synced* — the list sent is taken as
//! the complete list, so an item missing from it is deleted. That makes the
//! endpoint idempotent: a client can send back what it has and end up with what
//! it sent.
//!
//! Everything is validated before anything is applied, including references
//! between groups. A rule may not point at a category the same request deletes,
//! and finding that out has to happen before half the request has landed.

use std::collections::HashSet;

use axum::Json;
use axum::extract::State;
use myledger_core::{
    Account, Budget, Category, Goal, RecurringItem, Rule, Tag, TagUsage, normalize_merchant,
    now_timestamp,
};
// The request and response shapes live in `core::wire`: the web app builds
// them and this module parses them, so one definition is what keeps the two
// sides from drifting apart. See the note at the top of that module.
pub use myledger_core::wire::{
    BudgetInput, GoalInput, NamedInput, PreferencesRequest, PreferencesResponse,
    RecurringItemInput, RuleInput,
};
use myledger_db::lookups::{accounts, categories, tags};
use myledger_db::settings::keys;
use myledger_db::{
    Db, budgets, dismissed, goals, recurring_items, rules as db_rules, settings as db_settings,
};
use uuid::Uuid;

use crate::AppState;
use crate::api::{check_not_negative, clean_date, clean_name, clean_recurring_fields};
use crate::error::ApiError;
use crate::extract::ApiJson;

/// Applies a subset of preferences.
pub async fn put_preferences(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<PreferencesRequest>,
) -> Result<Json<PreferencesResponse>, ApiError> {
    let db = &state.db;

    let existing_categories = categories::list_all_categories(db).await?;
    let existing_accounts = accounts::list_all_accounts(db).await?;
    let existing_tags = tags::list_all_tags(db).await?;

    // ---- plan, and validate, before touching anything -----------------

    let category_plan = plan_lookup(&existing_categories, request.categories.as_deref(), "category")?;
    let account_plan = plan_lookup(&existing_accounts, request.accounts.as_deref(), "account")?;
    let tag_plan = plan_lookup(&existing_tags, request.tags.as_deref(), "tag")?;

    // References are checked against what will *remain* live once this request
    // has been applied, so a rule cannot survive pointing at a category the
    // same request removes.
    let refs = Surviving {
        categories: category_plan.surviving.clone(),
        accounts: account_plan.surviving.clone(),
        tags: tag_plan.surviving.clone(),
    };

    if let Some(rules) = &request.rules {
        validate_rules(rules, &refs)?;
    }
    if let Some(budgets) = &request.budgets {
        validate_budgets(budgets, &refs)?;
    }
    if let Some(goals) = &request.goals {
        validate_goals(goals)?;
    }
    if let Some(items) = &request.recurring_items {
        validate_recurring_items(items, &refs)?;
    }
    if let Some(assets) = request.assets_cents {
        check_not_negative(assets, "assets")?;
    }
    if let Some(liabilities) = request.liabilities_cents {
        check_not_negative(liabilities, "liabilities")?;
    }

    // ---- apply ---------------------------------------------------------

    let mut updated = Vec::new();

    if request.categories.is_some() {
        apply_category_plan(db, category_plan).await?;
        updated.push("categories");
    }
    if request.accounts.is_some() {
        apply_account_plan(db, account_plan).await?;
        updated.push("accounts");
    }
    let mut tag_links_removed = 0u64;
    if request.tags.is_some() {
        // The IDs are needed before the plan is consumed by applying it.
        let removed = tag_plan.deletes.clone();
        apply_tag_plan(db, tag_plan).await?;
        updated.push("tags");

        // Only ever on an explicit request, and only for the tags this
        // request actually removed — never for one deleted earlier.
        if request.strip_removed_tags == Some(true) && !removed.is_empty() {
            tag_links_removed = tags::strip_tags_from_transactions(db, &removed).await?;
            updated.push("tag_links");
        }
    }
    if let Some(rules) = request.rules {
        sync_rules(db, rules).await?;
        updated.push("rules");
    }
    if let Some(items) = request.budgets {
        sync_budgets(db, items).await?;
        updated.push("budgets");
    }
    if let Some(items) = request.goals {
        sync_goals(db, items).await?;
        updated.push("goals");
    }
    if let Some(items) = request.recurring_items {
        sync_recurring_items(db, items).await?;
        updated.push("recurring_items");
    }
    if let Some(patterns) = request.dismissed_patterns {
        sync_dismissed_patterns(db, patterns).await?;
        updated.push("dismissed_patterns");
    }

    // Settings are written key by key, so one preference can never clear another.
    if let Some(assets) = request.assets_cents {
        db_settings::set_setting(db, keys::ASSETS_CENTS, &assets.to_string()).await?;
        updated.push("assets_cents");
    }
    if let Some(liabilities) = request.liabilities_cents {
        db_settings::set_setting(db, keys::LIABILITIES_CENTS, &liabilities.to_string()).await?;
        updated.push("liabilities_cents");
    }
    if let Some(configured) = request.net_worth_configured {
        let value = if configured { "true" } else { "false" };
        db_settings::set_setting(db, keys::NET_WORTH_CONFIGURED, value).await?;
        updated.push("net_worth_configured");
    }
    if let Some(period) = request.selected_period {
        db_settings::set_setting(db, keys::SELECTED_PERIOD, period.as_str()).await?;
        updated.push("selected_period");
    }

    Ok(Json(PreferencesResponse {
        tag_links_removed,
        // The shared shape holds owned strings so a client can deserialise
        // it; the push sites stay `&'static str`.
        updated: updated.into_iter().map(String::from).collect(),
        categories: categories::list_all_categories(db).await?,
        accounts: accounts::list_all_accounts(db).await?,
        tags: tags::list_all_tags(db).await?,
        tag_usage: tags::tag_usage(db)
            .await?
            .into_iter()
            .map(|(tag_id, transactions)| TagUsage { tag_id, transactions })
            .collect(),
        rules: db_rules::list_rules(db).await?,
        budgets: budgets::list_budgets(db).await?,
        goals: goals::list_goals(db).await?,
        recurring_items: recurring_items::list_recurring_items(db).await?,
        dismissed_patterns: dismissed::list_dismissed_patterns(db).await?,
        settings: db_settings::load_settings(db).await?,
    }))
}

/// The IDs that will still be live after this request lands.
struct Surviving {
    categories: HashSet<Uuid>,
    accounts: HashSet<Uuid>,
    tags: HashSet<Uuid>,
}

impl Surviving {
    fn category(&self, id: Uuid) -> Result<(), ApiError> {
        self.check(&self.categories, id, "category")
    }
    fn account(&self, id: Uuid) -> Result<(), ApiError> {
        self.check(&self.accounts, id, "account")
    }
    fn tag(&self, id: Uuid) -> Result<(), ApiError> {
        self.check(&self.tags, id, "tag")
    }

    fn check(&self, set: &HashSet<Uuid>, id: Uuid, what: &str) -> Result<(), ApiError> {
        if set.contains(&id) {
            return Ok(());
        }
        Err(ApiError::bad_request(format!(
            "no live {what} with id {id} — it does not exist, or this request deletes it"
        )))
    }
}

/// What syncing one lookup table will do.
struct LookupPlan {
    renames: Vec<(Uuid, String)>,
    inserts: Vec<String>,
    deletes: Vec<Uuid>,
    /// IDs live once this is applied. For a group the request didn't mention,
    /// this is simply everything currently live.
    surviving: HashSet<Uuid>,
}

/// Works out the difference between what is stored and what was sent.
fn plan_lookup<T: LookupRecord>(
    existing: &[T],
    input: Option<&[NamedInput]>,
    what: &str,
) -> Result<LookupPlan, ApiError> {
    let live: Vec<&T> = existing.iter().filter(|row| row.deleted_at().is_none()).collect();

    let Some(input) = input else {
        return Ok(LookupPlan {
            renames: Vec::new(),
            inserts: Vec::new(),
            deletes: Vec::new(),
            surviving: live.iter().map(|row| row.id()).collect(),
        });
    };

    let mut renames = Vec::new();
    let mut inserts = Vec::new();
    let mut kept: HashSet<Uuid> = HashSet::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for item in input {
        let name = clean_name(&item.name, what)?;
        // Two things with the same name would be indistinguishable in a picker.
        if !seen_names.insert(name.to_lowercase()) {
            return Err(ApiError::bad_request(format!("two {what} entries called {name:?}")));
        }

        match item.id {
            Some(id) => {
                let Some(row) = live.iter().find(|row| row.id() == id) else {
                    return Err(ApiError::bad_request(format!("no live {what} with id {id}")));
                };
                if row.name() != name {
                    renames.push((id, name));
                }
                kept.insert(id);
            }
            // No ID: reuse a live row of the same name if there is one, so that
            // re-sending the list doesn't churn IDs and orphan transactions.
            None => match live.iter().find(|row| row.name().eq_ignore_ascii_case(&name)) {
                Some(row) => {
                    kept.insert(row.id());
                }
                None => inserts.push(name),
            },
        }
    }

    let deletes: Vec<Uuid> =
        live.iter().map(|row| row.id()).filter(|id| !kept.contains(id)).collect();

    Ok(LookupPlan { renames, inserts, deletes, surviving: kept })
}

/// The shape `plan_lookup` needs, so one function serves all three tables.
trait LookupRecord {
    fn id(&self) -> Uuid;
    fn name(&self) -> &str;
    fn deleted_at(&self) -> Option<&String>;
}

macro_rules! impl_lookup_record {
    ($($ty:ty),+ $(,)?) => {
        $(impl LookupRecord for $ty {
            fn id(&self) -> Uuid { self.id }
            fn name(&self) -> &str { &self.name }
            fn deleted_at(&self) -> Option<&String> { self.deleted_at.as_ref() }
        })+
    };
}

impl_lookup_record!(Category, Account, Tag);

macro_rules! apply_plan {
    ($fn_name:ident, $module:ident, $insert:ident, $rename:ident, $delete:ident) => {
        async fn $fn_name(db: &Db, plan: LookupPlan) -> Result<(), ApiError> {
            for (id, name) in plan.renames {
                $module::$rename(db, id, &name).await?;
            }
            for name in plan.inserts {
                $module::$insert(db, &name).await?;
            }
            for id in plan.deletes {
                $module::$delete(db, id).await?;
            }
            Ok(())
        }
    };
}

apply_plan!(apply_category_plan, categories, insert_category, rename_category, soft_delete_category);
apply_plan!(apply_account_plan, accounts, insert_account, rename_account, soft_delete_account);
apply_plan!(apply_tag_plan, tags, insert_tag, rename_tag, soft_delete_tag);

// ---- rules -----------------------------------------------------------------

fn validate_rules(input: &[RuleInput], refs: &Surviving) -> Result<(), ApiError> {
    for rule in input {
        if rule.when_text.trim().is_empty() {
            return Err(ApiError::bad_request(
                "a rule needs some text to match on; an empty rule would match every merchant",
            ));
        }
        if let Some(id) = rule.then_category_id {
            refs.category(id)?;
        }
        if let Some(id) = rule.then_tag_id {
            refs.tag(id)?;
        }
        if rule.then_category_id.is_none() && rule.then_tag_id.is_none() {
            return Err(ApiError::bad_request(format!(
                "the rule for {:?} does nothing: give it a category, a tag, or both",
                rule.when_text.trim()
            )));
        }
    }
    Ok(())
}

async fn sync_rules(db: &Db, input: Vec<RuleInput>) -> Result<(), ApiError> {
    let existing = db_rules::list_rules(db).await?;
    let mut kept = HashSet::new();

    for item in input {
        let now = now_timestamp();
        let id = item.id.unwrap_or_else(Uuid::new_v4);
        let record = Rule {
            id,
            when_text: item.when_text.trim().to_string(),
            then_category_id: item.then_category_id,
            then_tag_id: item.then_tag_id,
            enabled: item.enabled,
            created_at: existing
                .iter()
                .find(|r| r.id == id)
                .map_or(now.clone(), |r| r.created_at.clone()),
            updated_at: now,
        };

        if existing.iter().any(|r| r.id == id) {
            db_rules::update_rule(db, &record).await?;
        } else {
            db_rules::insert_rule(db, &record).await?;
        }
        kept.insert(id);
    }

    // Rules have no `deleted_at` — nothing references them, so a delete is real.
    for rule in existing.iter().filter(|r| !kept.contains(&r.id)) {
        db_rules::delete_rule(db, rule.id).await?;
    }
    Ok(())
}

// ---- budgets ---------------------------------------------------------------

fn validate_budgets(input: &[BudgetInput], refs: &Surviving) -> Result<(), ApiError> {
    let mut seen: HashSet<Uuid> = HashSet::new();
    for budget in input {
        check_not_negative(budget.monthly_limit_cents, "a budget limit")?;
        if let Some(id) = budget.category_id {
            refs.category(id)?;
            // Two ceilings on one category is a contradiction, not a preference.
            if !seen.insert(id) {
                return Err(ApiError::bad_request(
                    "two budgets for the same category; keep one",
                ));
            }
        }
    }
    Ok(())
}

async fn sync_budgets(db: &Db, input: Vec<BudgetInput>) -> Result<(), ApiError> {
    let existing = budgets::list_budgets(db).await?;
    let mut kept = HashSet::new();

    for item in input {
        let now = now_timestamp();
        let id = item.id.unwrap_or_else(Uuid::new_v4);
        let record = Budget {
            id,
            category_id: item.category_id,
            monthly_limit_cents: item.monthly_limit_cents,
            active: item.active,
            created_at: existing
                .iter()
                .find(|b| b.id == id)
                .map_or(now.clone(), |b| b.created_at.clone()),
            updated_at: now,
            deleted_at: None,
        };

        if existing.iter().any(|b| b.id == id) {
            budgets::update_budget(db, &record).await?;
        } else {
            budgets::insert_budget(db, &record).await?;
        }
        kept.insert(id);
    }

    for budget in existing.iter().filter(|b| !kept.contains(&b.id)) {
        budgets::soft_delete_budget(db, budget.id).await?;
    }
    Ok(())
}

// ---- goals -----------------------------------------------------------------

fn validate_goals(input: &[GoalInput]) -> Result<(), ApiError> {
    for goal in input {
        clean_name(&goal.name, "goal")?;
        check_not_negative(goal.target_cents, "a goal target")?;
        check_not_negative(goal.current_cents, "a goal balance")?;
        clean_date(goal.due_date.as_deref())?;
    }
    Ok(())
}

async fn sync_goals(db: &Db, input: Vec<GoalInput>) -> Result<(), ApiError> {
    let existing = goals::list_goals(db).await?;
    let mut kept = HashSet::new();

    for item in input {
        let now = now_timestamp();
        let id = item.id.unwrap_or_else(Uuid::new_v4);
        let record = Goal {
            id,
            name: item.name.trim().to_string(),
            target_cents: item.target_cents,
            current_cents: item.current_cents,
            due_date: item.due_date.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
            note: item.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
            created_at: existing
                .iter()
                .find(|g| g.id == id)
                .map_or(now.clone(), |g| g.created_at.clone()),
            updated_at: now,
            deleted_at: None,
        };

        if existing.iter().any(|g| g.id == id) {
            goals::update_goal(db, &record).await?;
        } else {
            goals::insert_goal(db, &record).await?;
        }
        kept.insert(id);
    }

    for goal in existing.iter().filter(|g| !kept.contains(&g.id)) {
        goals::soft_delete_goal(db, goal.id).await?;
    }
    Ok(())
}

// ---- recurring items -------------------------------------------------------

fn validate_recurring_items(
    input: &[RecurringItemInput],
    refs: &Surviving,
) -> Result<(), ApiError> {
    for item in input {
        // The same checks a hand-added item goes through, so an item cannot be
        // valid by one route and invalid by the other.
        clean_recurring_fields(&item.name, item.amount_cents, item.next_date.as_deref())?;
        if let Some(id) = item.category_id {
            refs.category(id)?;
        }
        if let Some(id) = item.account_id {
            refs.account(id)?;
        }
    }
    Ok(())
}

async fn sync_recurring_items(db: &Db, input: Vec<RecurringItemInput>) -> Result<(), ApiError> {
    let existing = recurring_items::list_recurring_items(db).await?;
    let mut kept = HashSet::new();

    for item in input {
        let now = now_timestamp();
        let id = item.id.unwrap_or_else(Uuid::new_v4);
        let record = RecurringItem {
            id,
            name: item.name.trim().to_string(),
            category_id: item.category_id,
            amount_cents: item.amount_cents,
            cadence: item.cadence,
            next_date: item.next_date.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
            account_id: item.account_id,
            kind: item.kind,
            active: item.active,
            created_at: existing
                .iter()
                .find(|r| r.id == id)
                .map_or(now.clone(), |r| r.created_at.clone()),
            updated_at: now,
            deleted_at: None,
        };

        if existing.iter().any(|r| r.id == id) {
            recurring_items::update_recurring_item(db, &record).await?;
        } else {
            recurring_items::insert_recurring_item(db, &record).await?;
        }
        kept.insert(id);
    }

    for item in existing.iter().filter(|r| !kept.contains(&r.id)) {
        recurring_items::soft_delete_recurring_item(db, item.id).await?;
    }
    Ok(())
}

// ---- dismissed patterns ----------------------------------------------------

/// Replaces the dismissal list.
///
/// Keys are run through the same normalisation detection uses, so a pattern
/// dismissed by hand matches what the algorithm would produce for the same
/// merchant. Without that, a hand-typed "Netflix" would never hide anything.
async fn sync_dismissed_patterns(db: &Db, input: Vec<String>) -> Result<(), ApiError> {
    let wanted: HashSet<String> = input
        .iter()
        .map(|raw| normalize_merchant(raw))
        .filter(|key| !key.is_empty())
        .collect();

    let existing: HashSet<String> = dismissed::list_dismissed_patterns(db).await?.into_iter().collect();

    for key in wanted.difference(&existing) {
        dismissed::dismiss_pattern(db, key).await?;
    }
    for key in existing.difference(&wanted) {
        dismissed::restore_pattern(db, key).await?;
    }
    Ok(())
}
