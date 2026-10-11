//! End-to-end tests of the core data API: state, transactions, preferences.

mod common;

use axum::http::StatusCode;
use common::{Client, find_named};
use serde_json::{Value, json};

/// A minimal valid transaction body.
fn coffee() -> Value {
    json!({
        "date": "2026-03-04",
        "merchant": "Starbucks",
        "amount_cents": 545,
        "kind": "expense",
        "account": "Main Account"
    })
}

// ---- reading ---------------------------------------------------------------

#[tokio::test]
async fn a_fresh_ledger_reads_as_empty_with_only_the_starter_lookups() {
    let client = Client::start().await;

    let state = client.state().await;

    assert_eq!(state["transactions"], json!([]));
    assert_eq!(state["transactions_truncated"], false);
    assert_eq!(state["budgets"], json!([]));
    assert_eq!(state["goals"], json!([]));
    assert_eq!(state["recurring_items"], json!([]));
    assert_eq!(state["rules"], json!([]));
    assert_eq!(state["sms_parse_rules"], json!([]));
    assert_eq!(state["documents"], json!([]));
    assert_eq!(state["dismissed_patterns"], json!([]));
    assert_eq!(state["tags"], json!([]));

    assert_eq!(state["categories"].as_array().unwrap().len(), 13);
    assert_eq!(state["accounts"].as_array().unwrap().len(), 3);

    assert_eq!(state["settings"]["selected_period"], "all-time");
    assert_eq!(state["settings"]["net_worth_configured"], false);
    assert_eq!(state["settings"]["assets_cents"], 0);
    assert_eq!(state["settings"]["liabilities_cents"], 0);
}

#[tokio::test]
async fn state_carries_no_secrets_and_no_disk_paths() {
    let client = Client::start().await;

    // A document row exists, so there is something whose path could leak.
    let document = myledger_core::Document {
        id: uuid::Uuid::new_v4(),
        filename: "statement.pdf".to_string(),
        mime_type: Some("application/pdf".to_string()),
        size: Some(1024),
        storage_path: "/very/private/path/abc.pdf".to_string(),
        status: myledger_core::DocumentStatus::Queued,
        source: Some("upload".to_string()),
        linked_transaction_id: None,
        created_at: myledger_core::now_timestamp(),
    };
    myledger_db::documents::insert_document(&client.db, &document).await.unwrap();

    let state = client.state().await;
    let rendered = state.to_string();

    assert_eq!(state["documents"].as_array().unwrap().len(), 1);
    assert_eq!(state["documents"][0]["filename"], "statement.pdf");
    assert!(state["documents"][0].get("storage_path").is_none());

    for secret in ["/very/private/path", "password_hash", "token_hash", "$argon2"] {
        assert!(!rendered.contains(secret), "state leaked {secret:?}");
    }
}

// ---- writing ---------------------------------------------------------------

#[tokio::test]
async fn a_transaction_can_be_added_and_read_back() {
    let client = Client::start().await;
    let groceries = client.category_id("Groceries").await;

    let mut body = coffee();
    body["merchant"] = json!("  Whole Foods  ");
    body["amount_cents"] = json!(8532);
    body["category_id"] = json!(groceries);

    let report = client.post("/api/v1/transactions", body).await.expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 1);
    assert_eq!(report["duplicates"], 0);
    assert_eq!(report["rejected"], 0);
    assert_eq!(report["results"][0]["outcome"], "inserted");

    let state = client.state().await;
    let stored = &state["transactions"][0];
    assert_eq!(state["transactions"].as_array().unwrap().len(), 1);
    assert_eq!(stored["merchant"], "Whole Foods", "the merchant is stored trimmed");
    assert_eq!(stored["amount_cents"], 8532);
    assert_eq!(stored["kind"], "expense");
    assert_eq!(stored["date"], "2026-03-04");
    assert_eq!(stored["category_id"], groceries);
    assert_eq!(stored["source"], "manual");
    assert_eq!(stored["tags"], json!([]));
    assert_eq!(
        stored["fingerprint"], "2026-03-04|whole foods|8532|main account",
        "the fingerprint folds case and trims"
    );
}

#[tokio::test]
async fn the_same_transaction_twice_leaves_one_row() {
    let client = Client::start().await;

    let first = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::CREATED);
    let inserted_id = first["results"][0]["id"].as_str().unwrap().to_string();

    let second = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::OK);
    assert_eq!(second["inserted"], 0);
    assert_eq!(second["duplicates"], 1);
    assert_eq!(second["results"][0]["outcome"], "duplicate");
    assert_eq!(
        second["results"][0]["existing_id"], inserted_id,
        "the report should name the row it clashed with"
    );

    let state = client.state().await;
    assert_eq!(state["transactions"].as_array().unwrap().len(), 1, "no second row");
}

#[tokio::test]
async fn a_batch_reports_on_every_row_independently() {
    let client = Client::start().await;

    let mut different = coffee();
    different["merchant"] = json!("Peets");

    let mut invalid = coffee();
    invalid["date"] = json!("2026-02-30");

    // The second row duplicates the first inside the same batch, which only the
    // database's unique constraint can catch.
    let report = client
        .post("/api/v1/transactions", json!([coffee(), coffee(), different, invalid]))
        .await
        .expect(StatusCode::OK);

    assert_eq!(report["inserted"], 2);
    assert_eq!(report["duplicates"], 1);
    assert_eq!(report["rejected"], 1);

    let results = report["results"].as_array().unwrap();
    assert_eq!(results[0]["outcome"], "inserted");
    assert_eq!(results[1]["outcome"], "duplicate");
    assert_eq!(results[2]["outcome"], "inserted");
    assert_eq!(results[3]["outcome"], "rejected");
    assert!(
        results[3]["message"].as_str().unwrap().contains("2026-02-30"),
        "the rejection should name the bad value: {}",
        results[3]["message"]
    );

    assert_eq!(client.state().await["transactions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_batch_where_everything_is_invalid_is_a_bad_request() {
    let client = Client::start().await;

    let mut no_merchant = coffee();
    no_merchant["merchant"] = json!("   ");

    let report = client
        .post("/api/v1/transactions", json!([no_merchant]))
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert_eq!(report["inserted"], 0);
    assert_eq!(report["rejected"], 1);
}

#[tokio::test]
async fn each_validation_rule_is_enforced() {
    let client = Client::start().await;

    for (field, value, expected) in [
        ("date", json!("2026-3-4"), "YYYY-MM-DD"),
        ("merchant", json!(""), "merchant"),
        ("amount_cents", json!(0), "greater than zero"),
        ("amount_cents", json!(-100), "greater than zero"),
        ("kind", json!("transfer"), "expense, income"),
    ] {
        let mut body = coffee();
        body[field] = value.clone();

        let report = client.post("/api/v1/transactions", body).await.expect(StatusCode::BAD_REQUEST);
        let message = report["results"][0]["message"].as_str().unwrap();
        assert!(
            message.contains(expected),
            "{field}={value} should have complained about {expected:?}, said {message:?}"
        );
    }
}

#[tokio::test]
async fn an_account_can_be_given_by_name_or_id_but_must_exist() {
    let client = Client::start().await;
    let credit_card = client.account_id("Credit Card").await;

    let mut by_id = coffee();
    by_id.as_object_mut().unwrap().remove("account");
    by_id["account_id"] = json!(credit_card);
    let report = client.post("/api/v1/transactions", by_id).await.expect(StatusCode::CREATED);
    assert_eq!(report["transactions"][0]["account_id"], credit_card);
    assert_eq!(
        report["transactions"][0]["fingerprint"], "2026-03-04|starbucks|545|credit card",
        "the account's name is what feeds the fingerprint, even when given by id"
    );

    let mut unknown = coffee();
    unknown["account"] = json!("Offshore Vault");
    unknown["merchant"] = json!("Peets");
    let rejected =
        client.post("/api/v1/transactions", unknown).await.expect(StatusCode::BAD_REQUEST);
    assert!(
        rejected["results"][0]["message"].as_str().unwrap().contains("Offshore Vault"),
        "{}",
        rejected["results"][0]["message"]
    );
}

// ---- rules -----------------------------------------------------------------

#[tokio::test]
async fn a_rule_categorises_an_incoming_transaction() {
    let client = Client::start().await;
    let dining = client.category_id("Dining").await;

    client
        .put(
            "/api/v1/preferences",
            json!({ "rules": [{ "when_text": "starbucks", "then_category_id": dining }] }),
        )
        .await
        .expect(StatusCode::OK);

    let report = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::CREATED);

    assert_eq!(report["transactions"][0]["category_id"], dining);
}

#[tokio::test]
async fn a_rule_cannot_turn_a_duplicate_into_a_second_row() {
    let client = Client::start().await;
    let dining = client.category_id("Dining").await;

    client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::CREATED);

    // Add a rule that would change how this merchant is filed, then re-import
    // the same purchase. Rules run after duplicate detection, so the second
    // arrival is still a duplicate and the first row is left as it was.
    client
        .put(
            "/api/v1/preferences",
            json!({ "rules": [{ "when_text": "starbucks", "then_category_id": dining }] }),
        )
        .await
        .expect(StatusCode::OK);

    let second = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::OK);
    assert_eq!(second["duplicates"], 1);
    assert_eq!(second["inserted"], 0);

    let state = client.state().await;
    assert_eq!(state["transactions"].as_array().unwrap().len(), 1);
    assert_eq!(
        state["transactions"][0]["category_id"],
        Value::Null,
        "the row already on file is untouched by a rule added later"
    );
}

// ---- editing ---------------------------------------------------------------

#[tokio::test]
async fn patching_the_category_leaves_the_tags_alone() {
    let client = Client::start().await;

    client
        .put("/api/v1/preferences", json!({ "tags": [{ "name": "treat" }, { "name": "weekly" }] }))
        .await
        .expect(StatusCode::OK);
    let state = client.state().await;
    let treat = find_named(&state["tags"], "treat");
    let weekly = find_named(&state["tags"], "weekly");
    let dining = find_named(&state["categories"], "Dining");

    let mut body = coffee();
    body["tags"] = json!([treat, weekly]);
    let report = client.post("/api/v1/transactions", body).await.expect(StatusCode::CREATED);
    let id = report["results"][0]["id"].as_str().unwrap().to_string();

    let patched = client
        .patch(&format!("/api/v1/transactions/{id}"), json!({ "category_id": dining }))
        .await
        .expect(StatusCode::OK);

    assert_eq!(patched["category_id"], dining);
    let mut tags: Vec<&str> = patched["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    tags.sort_unstable();
    let mut expected = vec![treat.as_str(), weekly.as_str()];
    expected.sort_unstable();
    assert_eq!(tags, expected, "a category-only patch must not disturb the tags");

    // And the same is true the other way round.
    let retagged = client
        .patch(&format!("/api/v1/transactions/{id}"), json!({ "tags": [treat] }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(retagged["tags"], json!([treat]));
    assert_eq!(retagged["category_id"], dining, "a tags-only patch must not disturb the category");
}

#[tokio::test]
async fn a_patch_tells_absent_apart_from_null() {
    let client = Client::start().await;
    let dining = client.category_id("Dining").await;

    let mut body = coffee();
    body["category_id"] = json!(dining);
    let report = client.post("/api/v1/transactions", body).await.expect(StatusCode::CREATED);
    let id = report["results"][0]["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/transactions/{id}");

    // An explicit null clears it…
    let cleared = client.patch(&path, json!({ "category_id": null })).await.expect(StatusCode::OK);
    assert_eq!(cleared["category_id"], Value::Null);

    // …while sending neither field is a request that does nothing, and says so.
    client.patch(&path, json!({})).await.expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patching_rejects_unknown_references_and_missing_rows() {
    let client = Client::start().await;
    let report = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::CREATED);
    let id = report["results"][0]["id"].as_str().unwrap().to_string();
    let stranger = uuid::Uuid::new_v4();

    client
        .patch(&format!("/api/v1/transactions/{id}"), json!({ "category_id": stranger }))
        .await
        .expect(StatusCode::BAD_REQUEST);
    client
        .patch(&format!("/api/v1/transactions/{id}"), json!({ "tags": [stranger] }))
        .await
        .expect(StatusCode::BAD_REQUEST);
    client
        .patch(&format!("/api/v1/transactions/{stranger}"), json!({ "category_id": null }))
        .await
        .expect(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_deleted_transaction_goes_from_state_but_keeps_its_fingerprint() {
    let client = Client::start().await;
    let report = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::CREATED);
    let id = report["results"][0]["id"].as_str().unwrap().to_string();

    let deleted = client.delete(&format!("/api/v1/transactions/{id}")).await.expect(StatusCode::OK);
    assert_eq!(deleted["deleted"], true);

    assert_eq!(client.state().await["transactions"], json!([]));

    // Deleting twice is a 404, not a silent success.
    client.delete(&format!("/api/v1/transactions/{id}")).await.expect(StatusCode::NOT_FOUND);

    // Re-importing the statement must not resurrect it as a new row.
    let again = client.post("/api/v1/transactions", coffee()).await.expect(StatusCode::OK);
    assert_eq!(again["duplicates"], 1);
    assert_eq!(client.state().await["transactions"], json!([]));
}

// ---- preferences -----------------------------------------------------------

#[tokio::test]
async fn changing_the_period_leaves_every_other_group_alone() {
    let client = Client::start().await;
    let before = client.state().await;

    let response = client
        .put("/api/v1/preferences", json!({ "selected_period": "last-3-months" }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(response["updated"], json!(["selected_period"]));

    let after = client.state().await;
    assert_eq!(after["settings"]["selected_period"], "last-3-months");
    assert_eq!(after["categories"], before["categories"], "categories are untouched");
    assert_eq!(after["accounts"], before["accounts"], "accounts are untouched");
    assert_eq!(after["settings"]["assets_cents"], 0, "other settings are untouched");
    assert_eq!(after["settings"]["net_worth_configured"], false);
}

#[tokio::test]
async fn net_worth_can_be_set_without_disturbing_the_period() {
    let client = Client::start().await;
    client
        .put("/api/v1/preferences", json!({ "selected_period": "this-year" }))
        .await
        .expect(StatusCode::OK);

    client
        .put(
            "/api/v1/preferences",
            json!({ "assets_cents": 12_345_600, "liabilities_cents": 4_000_000, "net_worth_configured": true }),
        )
        .await
        .expect(StatusCode::OK);

    let settings = client.state().await["settings"].clone();
    assert_eq!(settings["assets_cents"], 12_345_600);
    assert_eq!(settings["liabilities_cents"], 4_000_000);
    assert_eq!(settings["net_worth_configured"], true);
    assert_eq!(settings["selected_period"], "this-year", "still what it was set to");
}

#[tokio::test]
async fn sending_a_category_list_adds_renames_and_removes_in_one_go() {
    let client = Client::start().await;
    let before = client.state().await;
    let categories = before["categories"].as_array().unwrap();
    let other_id = find_named(&before["categories"], "Other");

    // Keep everything but "Dining", rename "Other", and add one.
    let mut sent: Vec<Value> = categories
        .iter()
        .filter(|c| c["name"] != "Dining")
        .map(|c| {
            let name = if c["id"] == Value::String(other_id.clone()) {
                json!("Miscellaneous")
            } else {
                c["name"].clone()
            };
            json!({ "id": c["id"], "name": name })
        })
        .collect();
    sent.push(json!({ "name": "Travel" }));

    client.put("/api/v1/preferences", json!({ "categories": sent })).await.expect(StatusCode::OK);

    let after = client.state().await;
    let live: Vec<&str> = after["categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["deleted_at"].is_null())
        .map(|c| c["name"].as_str().unwrap())
        .collect();

    assert!(live.contains(&"Travel"), "the new one was added");
    assert!(live.contains(&"Miscellaneous"), "the renamed one is there");
    assert!(!live.contains(&"Other"), "under its old name it is gone");
    assert!(!live.contains(&"Dining"), "the omitted one was removed");

    // A rename keeps the ID, so transactions filed against it are unaffected.
    assert_eq!(find_named(&after["categories"], "Miscellaneous"), other_id);

    // The removed one is soft-deleted and still present, so an old transaction
    // pointing at it can still be rendered with a name.
    let dining = after["categories"].as_array().unwrap().iter().find(|c| c["name"] == "Dining");
    assert!(dining.is_some_and(|c| !c["deleted_at"].is_null()), "Dining should be soft-deleted");
}

#[tokio::test]
async fn budgets_goals_and_recurring_items_sync_the_same_way() {
    let client = Client::start().await;
    let housing = client.category_id("Housing").await;

    let created = client
        .put(
            "/api/v1/preferences",
            json!({
                "budgets": [{ "category_id": housing, "monthly_limit_cents": 150_000 }],
                "goals": [{ "name": "Emergency fund", "target_cents": 500_000, "due_date": "2027-01-01" }],
                "recurring_items": [{
                    "name": "Netflix", "amount_cents": 1599, "cadence": "monthly",
                    "kind": "subscription", "next_date": "2026-04-05"
                }],
            }),
        )
        .await
        .expect(StatusCode::OK);

    assert_eq!(created["budgets"].as_array().unwrap().len(), 1);
    assert_eq!(created["goals"][0]["name"], "Emergency fund");
    assert_eq!(created["recurring_items"][0]["cadence"], "monthly");
    assert_eq!(created["recurring_items"][0]["kind"], "subscription");

    // Sending an empty list for one group clears that group and no other.
    let cleared = client
        .put("/api/v1/preferences", json!({ "goals": [] }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(cleared["goals"], json!([]));
    assert_eq!(cleared["budgets"].as_array().unwrap().len(), 1, "budgets are untouched");
    assert_eq!(cleared["recurring_items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn dismissed_patterns_are_normalised_the_way_detection_normalises_them() {
    let client = Client::start().await;

    let response = client
        .put("/api/v1/preferences", json!({ "dismissed_patterns": ["NETFLIX #1234", "  Spotify  "] }))
        .await
        .expect(StatusCode::OK);

    let mut keys: Vec<&str> =
        response["dismissed_patterns"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["netflix", "spotify"]);

    let cleared = client
        .put("/api/v1/preferences", json!({ "dismissed_patterns": ["netflix"] }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(cleared["dismissed_patterns"], json!(["netflix"]));
}

#[tokio::test]
async fn a_reference_to_something_the_same_request_deletes_is_refused_outright() {
    let client = Client::start().await;
    let before = client.state().await;
    let dining = find_named(&before["categories"], "Dining");

    // Delete "Dining" and, in the same breath, add a rule pointing at it.
    let kept: Vec<Value> = before["categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] != "Dining")
        .map(|c| json!({ "id": c["id"], "name": c["name"] }))
        .collect();

    let refused = client
        .put(
            "/api/v1/preferences",
            json!({
                "categories": kept,
                "rules": [{ "when_text": "starbucks", "then_category_id": dining }],
            }),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(
        refused["message"].as_str().unwrap().contains("this request deletes it"),
        "{}",
        refused["message"]
    );

    // Nothing was applied: validation happens before any write.
    let after = client.state().await;
    assert_eq!(after["categories"], before["categories"], "the category list is unchanged");
    assert_eq!(after["rules"], json!([]));
}

#[tokio::test]
async fn preferences_refuse_the_nonsensical() {
    let client = Client::start().await;
    let housing = client.category_id("Housing").await;

    for body in [
        json!({ "categories": [{ "name": "  " }] }),
        json!({ "categories": [{ "name": "Food" }, { "name": "FOOD" }] }),
        json!({ "budgets": [{ "category_id": housing, "monthly_limit_cents": -1 }] }),
        json!({ "budgets": [
            { "category_id": housing, "monthly_limit_cents": 100 },
            { "category_id": housing, "monthly_limit_cents": 200 }
        ]}),
        json!({ "goals": [{ "name": "Trip", "target_cents": 100, "due_date": "2026-02-30" }] }),
        json!({ "rules": [{ "when_text": "   ", "then_category_id": housing }] }),
        json!({ "rules": [{ "when_text": "starbucks" }] }),
        json!({ "recurring_items": [{ "name": "Netflix", "amount_cents": 0, "cadence": "monthly", "kind": "subscription" }] }),
        json!({ "selected_period": "last-week" }),
        json!({ "assets_cents": -1 }),
    ] {
        let reply = client.put("/api/v1/preferences", body.clone()).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body} should have been refused");
    }
}

#[tokio::test]
async fn an_empty_preferences_request_changes_nothing() {
    let client = Client::start().await;
    let before = client.state().await;

    let response = client.put("/api/v1/preferences", json!({})).await.expect(StatusCode::OK);
    assert_eq!(response["updated"], json!([]));

    let after = client.state().await;
    assert_eq!(after["categories"], before["categories"]);
    assert_eq!(after["settings"], before["settings"]);
}

// ---- access ----------------------------------------------------------------

#[tokio::test]
async fn the_data_api_is_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;

    for request in [
        common::get("/api/v1/state"),
        common::post_json("/api/v1/transactions", coffee()),
        common::request_with_json("PUT", "/api/v1/preferences", &json!({})),
        common::request("DELETE", &format!("/api/v1/transactions/{}", uuid::Uuid::new_v4())),
    ] {
        let path = request.uri().path().to_string();
        let reply = common::call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable unauthenticated");
    }
}

#[tokio::test]
async fn a_paired_device_can_use_the_data_api_too() {
    let (app, _db, _dir) = common::app_with_user().await;
    let session = common::sign_in(&app).await;
    let device = common::pair_device(&app, &session).await;

    let reply =
        common::call(&app, common::with_bearer(common::get("/api/v1/state"), &device)).await;
    assert_eq!(reply.status, StatusCode::OK);

    let posted = common::call(
        &app,
        common::with_bearer(common::post_json("/api/v1/transactions", coffee()), &device),
    )
    .await;
    assert_eq!(posted.status, StatusCode::CREATED, "the phone must be able to add transactions");
}
