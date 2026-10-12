//! Storage for SMS parsing rules.
//!
//! A rule says how to turn a bank's text alert into a transaction: a regex over
//! the message body, and which capture group holds the amount, the merchant and
//! the date. The engine that *runs* them against real messages lives in the
//! mobile app — the backend keeps them, hands them out, and checks they make
//! sense before storing them.
//!
//! That last part is worth doing here rather than leaving to the phone. A rule
//! whose regex doesn't compile, or that names capture group 4 of a regex with
//! two groups, is broken in a way the user can only discover by watching
//! messages silently fail to import. Catching it at the point of entry turns a
//! mystery into an error message.
//!
//! Nothing is seeded: banks phrase their alerts differently everywhere, so any
//! starter rule would be wrong for almost everyone.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use myledger_core::{SmsKindHint, SmsParseRule, now_timestamp};
use myledger_db::sms_rules as db_sms_rules;
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api::{Lookups, clean_name, explicit_option};
use crate::error::ApiError;
use crate::extract::ApiJson;

#[derive(Debug, Deserialize)]
pub struct NewSmsRuleRequest {
    pub label: String,
    /// Regex the sender must match, or absent to accept any sender.
    #[serde(default)]
    pub sender_pattern: Option<String>,
    pub body_regex: String,
    #[serde(default)]
    pub amount_group: Option<i64>,
    #[serde(default)]
    pub merchant_group: Option<i64>,
    #[serde(default)]
    pub date_group: Option<i64>,
    #[serde(default)]
    pub kind_hint: Option<SmsKindHint>,
    #[serde(default)]
    pub account_hint: Option<String>,
    #[serde(default)]
    pub default_category_id: Option<Uuid>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// A change to a rule. Absent leaves a field alone; `null` clears the ones that
/// can be cleared.
#[derive(Debug, Deserialize)]
pub struct PatchSmsRuleRequest {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub sender_pattern: Option<Option<String>>,
    #[serde(default)]
    pub body_regex: Option<String>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub amount_group: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub merchant_group: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub date_group: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub kind_hint: Option<Option<SmsKindHint>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub account_hint: Option<Option<String>>,
    #[serde(default, deserialize_with = "explicit_option")]
    pub default_category_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn yes() -> bool {
    true
}

/// Every rule, oldest first.
pub async fn list_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<SmsParseRule>>, ApiError> {
    Ok(Json(db_sms_rules::list_sms_rules(&state.db).await?))
}

/// One rule.
pub async fn get_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SmsParseRule>, ApiError> {
    db_sms_rules::get_sms_rule(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no SMS rule with id {id}")))
}

/// Adds a rule.
pub async fn create_rule(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<NewSmsRuleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(id) = request.default_category_id {
        Lookups::load(&state.db).await?.check_category(id)?;
    }

    let now = now_timestamp();
    let rule = SmsParseRule {
        id: Uuid::new_v4(),
        label: clean_name(&request.label, "SMS rule")?,
        sender_pattern: blank_to_none(request.sender_pattern),
        body_regex: request.body_regex.trim().to_string(),
        amount_group: request.amount_group,
        merchant_group: request.merchant_group,
        date_group: request.date_group,
        kind_hint: request.kind_hint,
        account_hint: blank_to_none(request.account_hint),
        default_category_id: request.default_category_id,
        enabled: request.enabled,
        created_at: now.clone(),
        updated_at: now,
    };
    check_rule(&rule)?;

    db_sms_rules::insert_sms_rule(&state.db, &rule).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

/// Edits a rule.
pub async fn patch_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    ApiJson(request): ApiJson<PatchSmsRuleRequest>,
) -> Result<Json<SmsParseRule>, ApiError> {
    let Some(mut rule) = db_sms_rules::get_sms_rule(&state.db, id).await? else {
        return Err(ApiError::not_found(format!("no SMS rule with id {id}")));
    };

    if let Some(label) = request.label {
        rule.label = clean_name(&label, "SMS rule")?;
    }
    if let Some(sender_pattern) = request.sender_pattern {
        rule.sender_pattern = blank_to_none(sender_pattern);
    }
    if let Some(body_regex) = request.body_regex {
        rule.body_regex = body_regex.trim().to_string();
    }
    if let Some(group) = request.amount_group {
        rule.amount_group = group;
    }
    if let Some(group) = request.merchant_group {
        rule.merchant_group = group;
    }
    if let Some(group) = request.date_group {
        rule.date_group = group;
    }
    if let Some(kind_hint) = request.kind_hint {
        rule.kind_hint = kind_hint;
    }
    if let Some(account_hint) = request.account_hint {
        rule.account_hint = blank_to_none(account_hint);
    }
    if let Some(category_id) = request.default_category_id {
        if let Some(category_id) = category_id {
            Lookups::load(&state.db).await?.check_category(category_id)?;
        }
        rule.default_category_id = category_id;
    }
    if let Some(enabled) = request.enabled {
        rule.enabled = enabled;
    }

    // Checked after merging, not before: changing the regex and the group
    // numbers in one request has to be judged as the whole it will become.
    check_rule(&rule)?;
    rule.updated_at = now_timestamp();

    db_sms_rules::update_sms_rule(&state.db, &rule).await?;
    db_sms_rules::get_sms_rule(&state.db, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::internal("the SMS rule vanished mid-update"))
}

#[derive(Debug, serde::Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
    pub id: Uuid,
}

/// Removes a rule. A real delete — nothing references one.
pub async fn delete_rule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    if !db_sms_rules::delete_sms_rule(&state.db, id).await? {
        return Err(ApiError::not_found(format!("no SMS rule with id {id}")));
    }
    Ok(Json(DeleteResponse { deleted: true, id }))
}

/// Whitespace-only optional text means "not set".
fn blank_to_none(value: Option<String>) -> Option<String> {
    value.map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

/// Checks that a rule could actually do its job.
/// Refuses a rule that could never work.
///
/// Delegates to `core::textrules::check_rule`, the same check the document
/// extraction rules use and the same engine that will run these rules on the
/// phone. There was a second copy of this logic here; two copies of "what
/// makes a rule valid" is how a rule starts being accepted by the server and
/// then rejected by whatever has to run it.
fn check_rule(rule: &SmsParseRule) -> Result<(), ApiError> {
    myledger_core::textrules::check_rule(&rule.as_view())
        .map_err(|problem| ApiError::bad_request(problem.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(body_regex: &str, amount: Option<i64>, merchant: Option<i64>) -> SmsParseRule {
        let now = now_timestamp();
        SmsParseRule {
            id: Uuid::new_v4(),
            label: "Test bank".to_string(),
            sender_pattern: None,
            body_regex: body_regex.to_string(),
            amount_group: amount,
            merchant_group: merchant,
            date_group: None,
            kind_hint: None,
            account_hint: None,
            default_category_id: None,
            enabled: true,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn a_workable_rule_passes() {
        let subject = rule(r"spent \$([0-9.]+) at (.+) on", Some(1), Some(2));
        assert!(check_rule(&subject).is_ok());
    }

    #[test]
    fn a_regex_that_does_not_compile_is_refused() {
        let subject = rule(r"spent \$([0-9.]+ at", Some(1), None);
        let err = check_rule(&subject).unwrap_err().to_string();
        assert!(err.contains("not a valid regex"), "{err}");
    }

    #[test]
    fn a_group_number_beyond_the_regex_is_refused() {
        let subject = rule(r"spent \$([0-9.]+)", Some(1), Some(4));
        let err = check_rule(&subject).unwrap_err().to_string();
        assert!(err.contains("merchant group is 4"), "{err}");
        assert!(err.contains("1 capture group"), "{err}");
    }

    #[test]
    fn group_zero_is_refused_because_it_is_the_whole_message() {
        let subject = rule(r"spent \$([0-9.]+)", Some(0), None);
        let err = check_rule(&subject).unwrap_err().to_string();
        // Group 0 is the whole match, so it is reported the same way as
        // any other out-of-range group.
        assert!(err.contains("amount group is 0"), "{err}");
    }

    #[test]
    fn a_rule_with_no_amount_group_cannot_produce_a_transaction() {
        let subject = rule(r"spent \$([0-9.]+)", None, None);
        let err = check_rule(&subject).unwrap_err().to_string();
        assert!(err.contains("no amount group"), "{err}");
    }

    #[test]
    fn an_empty_body_regex_is_refused() {
        let mut subject = rule("", Some(1), None);
        subject.body_regex = String::new();
        assert!(check_rule(&subject).is_err());
    }

    #[test]
    fn a_bad_sender_pattern_is_refused_too() {
        let mut subject = rule(r"spent \$([0-9.]+)", Some(1), None);
        subject.sender_pattern = Some("[unclosed".to_string());
        let err = check_rule(&subject).unwrap_err().to_string();
        assert!(err.contains("sender pattern"), "{err}");
    }

    #[test]
    fn blank_optional_text_becomes_absent() {
        assert_eq!(blank_to_none(Some("  ".to_string())), None);
        assert_eq!(blank_to_none(Some(" HDFC ".to_string())), Some("HDFC".to_string()));
        assert_eq!(blank_to_none(None), None);
    }
}
