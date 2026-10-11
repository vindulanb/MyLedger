//! End-to-end tests of SMS parsing rule storage.

mod common;

use axum::http::StatusCode;
use common::Client;
use serde_json::{Value, json};

/// A plausible rule for a bank that texts "Spent $12.34 at MERCHANT on DATE".
fn hdfc_rule() -> Value {
    json!({
        "label": "HDFC debit alert",
        "sender_pattern": "^HDFCBK$",
        "body_regex": r"Spent Rs\.?([0-9,.]+) at (.+) on ([0-9]{2}-[0-9]{2}-[0-9]{2})",
        "amount_group": 1,
        "merchant_group": 2,
        "date_group": 3,
        "kind_hint": "debit",
        "account_hint": "Main Account"
    })
}

#[tokio::test]
async fn a_fresh_migration_has_no_sms_rules() {
    let client = Client::start().await;

    assert_eq!(client.get("/api/v1/sms-rules").await.expect(StatusCode::OK), json!([]));
    assert_eq!(client.state().await["sms_parse_rules"], json!([]));
}

#[tokio::test]
async fn a_rule_can_be_created_read_edited_and_deleted() {
    let client = Client::start().await;
    let subscriptions = client.category_id("Subscriptions").await;

    // Create.
    let mut body = hdfc_rule();
    body["default_category_id"] = json!(subscriptions);
    let created = client.post("/api/v1/sms-rules", body).await.expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();

    assert_eq!(created["label"], "HDFC debit alert");
    assert_eq!(created["amount_group"], 1);
    assert_eq!(created["merchant_group"], 2);
    assert_eq!(created["date_group"], 3);
    assert_eq!(created["kind_hint"], "debit");
    assert_eq!(created["account_hint"], "Main Account");
    assert_eq!(created["default_category_id"], subscriptions);
    assert_eq!(created["enabled"], true, "rules are enabled unless said otherwise");

    // Read, both ways.
    assert_eq!(client.get("/api/v1/sms-rules").await.expect(StatusCode::OK)[0]["id"], id);
    assert_eq!(client.get(&format!("/api/v1/sms-rules/{id}")).await.expect(StatusCode::OK)["id"], id);
    assert_eq!(client.state().await["sms_parse_rules"].as_array().unwrap().len(), 1);

    // Update.
    let patched = client
        .patch(
            &format!("/api/v1/sms-rules/{id}"),
            json!({ "label": "HDFC card", "enabled": false, "kind_hint": null }),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["label"], "HDFC card");
    assert_eq!(patched["enabled"], false);
    assert_eq!(patched["kind_hint"], Value::Null, "an explicit null clears a field");
    assert_eq!(patched["amount_group"], 1, "untouched fields stay put");
    assert_eq!(patched["default_category_id"], subscriptions);

    // Delete.
    client.delete(&format!("/api/v1/sms-rules/{id}")).await.expect(StatusCode::OK);
    assert_eq!(client.get("/api/v1/sms-rules").await.expect(StatusCode::OK), json!([]));
    client.delete(&format!("/api/v1/sms-rules/{id}")).await.expect(StatusCode::NOT_FOUND);
    client.get(&format!("/api/v1/sms-rules/{id}")).await.expect(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_rule_that_could_never_work_is_refused() {
    let client = Client::start().await;
    let stranger = uuid::Uuid::new_v4();

    let cases = [
        ("a blank label", json!({ "label": "  ", "body_regex": "(.+)", "amount_group": 1 })),
        (
            "a regex that does not compile",
            json!({ "label": "Bank", "body_regex": r"Spent \$([0-9.]+ at", "amount_group": 1 }),
        ),
        (
            "a sender pattern that does not compile",
            json!({ "label": "Bank", "body_regex": "(.+)", "amount_group": 1,
                    "sender_pattern": "[unclosed" }),
        ),
        (
            "a group number the regex does not have",
            json!({ "label": "Bank", "body_regex": r"Spent ([0-9.]+)", "amount_group": 1,
                    "merchant_group": 4 }),
        ),
        (
            "group zero",
            json!({ "label": "Bank", "body_regex": r"Spent ([0-9.]+)", "amount_group": 0 }),
        ),
        ("no amount group", json!({ "label": "Bank", "body_regex": r"Spent ([0-9.]+)" })),
        ("an empty regex", json!({ "label": "Bank", "body_regex": "   ", "amount_group": 1 })),
        (
            "an unknown category",
            json!({ "label": "Bank", "body_regex": "(.+)", "amount_group": 1,
                    "default_category_id": stranger }),
        ),
        (
            "a kind hint that is not one",
            json!({ "label": "Bank", "body_regex": "(.+)", "amount_group": 1,
                    "kind_hint": "maybe" }),
        ),
    ];

    for (what, body) in cases {
        let reply = client.post("/api/v1/sms-rules", body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what} should have been refused");
    }

    assert_eq!(client.get("/api/v1/sms-rules").await.expect(StatusCode::OK), json!([]));
}

#[tokio::test]
async fn a_patch_is_judged_on_what_the_rule_will_become() {
    let client = Client::start().await;
    let created = client.post("/api/v1/sms-rules", hdfc_rule()).await.expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/sms-rules/{id}");

    // Narrowing the regex to one group while three are still referenced is
    // refused: the rule as a whole would be broken.
    client
        .patch(&path, json!({ "body_regex": r"Spent Rs\.?([0-9,.]+)" }))
        .await
        .expect(StatusCode::BAD_REQUEST);

    // Doing both in one request is fine, because the merged rule holds up.
    let fixed = client
        .patch(
            &path,
            json!({ "body_regex": r"Spent Rs\.?([0-9,.]+)", "merchant_group": null, "date_group": null }),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(fixed["merchant_group"], Value::Null);
    assert_eq!(fixed["date_group"], Value::Null);

    // The stored rule really did change.
    assert_eq!(
        client.get(&path).await.expect(StatusCode::OK)["body_regex"],
        r"Spent Rs\.?([0-9,.]+)"
    );
}

#[tokio::test]
async fn blank_optional_text_is_stored_as_absent() {
    let client = Client::start().await;

    let created = client
        .post(
            "/api/v1/sms-rules",
            json!({ "label": " Bank ", "body_regex": "(.+)", "amount_group": 1,
                    "sender_pattern": "   ", "account_hint": "  " }),
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(created["label"], "Bank");
    assert_eq!(created["sender_pattern"], Value::Null);
    assert_eq!(created["account_hint"], Value::Null);
}

#[tokio::test]
async fn sms_rules_are_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;

    for request in [
        common::get("/api/v1/sms-rules"),
        common::post_json("/api/v1/sms-rules", hdfc_rule()),
        common::request("DELETE", &format!("/api/v1/sms-rules/{}", uuid::Uuid::new_v4())),
    ] {
        let path = request.uri().path().to_string();
        let reply = common::call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable unauthenticated");
    }
}
