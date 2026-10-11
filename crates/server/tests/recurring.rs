//! End-to-end tests of recurring/subscription suggestions.
//!
//! Dates are built relative to today rather than hard-coded. Detection drops a
//! pattern once it is three periods overdue, so a fixture pinned to a literal
//! date would quietly stop being suggested a few months after it was written.

mod common;

use axum::http::StatusCode;
use chrono::{Months, NaiveDate, Utc};
use common::Client;
use serde_json::{Value, json};

/// A date `months_ago` before today, as `YYYY-MM-DD`.
///
/// Day-of-month clamps at the end of short months, which is exactly the
/// behaviour a real monthly charge has.
fn months_ago(months: u32) -> String {
    let today: NaiveDate = Utc::now().date_naive();
    today
        .checked_sub_months(Months::new(months))
        .expect("a date a few months back")
        .format("%Y-%m-%d")
        .to_string()
}

/// Three identical monthly charges from one merchant: the cleanest possible
/// pattern, ending today so it is nowhere near stale.
fn monthly_pattern(merchant: &str, amount_cents: i64) -> Value {
    json!([
        { "date": months_ago(2), "merchant": merchant, "amount_cents": amount_cents,
          "kind": "expense", "account": "Main Account" },
        { "date": months_ago(1), "merchant": merchant, "amount_cents": amount_cents,
          "kind": "expense", "account": "Main Account" },
        { "date": months_ago(0), "merchant": merchant, "amount_cents": amount_cents,
          "kind": "expense", "account": "Main Account" },
    ])
}

/// Posts a pattern and asserts every row landed.
async fn seed_pattern(client: &Client, merchant: &str, amount_cents: i64) {
    let report = client
        .post("/api/v1/transactions", monthly_pattern(merchant, amount_cents))
        .await
        .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 3, "seeding {merchant} failed: {report}");
}

async fn suggestions(client: &Client) -> Value {
    client.get("/api/v1/recurring/suggestions").await.expect(StatusCode::OK)
}

/// The merchants currently being suggested.
async fn suggested_merchants(client: &Client) -> Vec<String> {
    suggestions(client).await["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["merchant"].as_str().unwrap().to_string())
        .collect()
}

/// The names of the confirmed recurring items.
async fn confirmed_names(client: &Client) -> Vec<String> {
    client.get("/api/v1/recurring-items").await.expect(StatusCode::OK)
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_string())
        .collect()
}

// ---- the walkthrough the spec asks for -------------------------------------

#[tokio::test]
async fn a_pattern_is_suggested_then_ignored_then_a_second_one_is_kept() {
    let client = Client::start().await;

    // A clean monthly pattern is suggested.
    seed_pattern(&client, "Netflix", 1599).await;
    assert_eq!(suggested_merchants(&client).await, vec!["Netflix"]);

    // Ignoring it takes it off the list…
    let ignored =
        client.post("/api/v1/recurring/netflix/ignore", json!({})).await.expect(StatusCode::OK);
    assert_eq!(ignored["pattern_key"], "netflix");
    assert!(suggested_merchants(&client).await.is_empty());

    // …and it does not come back on the next call.
    let after = suggestions(&client).await;
    assert_eq!(after["suggestions"], json!([]));
    assert_eq!(after["hidden_as_ignored"], 1);

    // A fresh pattern turns up, and this one is kept.
    seed_pattern(&client, "Spotify", 1099).await;
    assert_eq!(suggested_merchants(&client).await, vec!["Spotify"]);

    let kept =
        client.post("/api/v1/recurring/spotify/keep", json!({})).await.expect(StatusCode::CREATED);
    assert_eq!(kept["name"], "Spotify");
    assert_eq!(kept["kind"], "subscription");
    assert_eq!(kept["cadence"], "monthly");
    assert_eq!(kept["amount_cents"], 1099);
    assert!(kept["active"].as_bool().unwrap());

    // It is now a confirmed item and no longer a suggestion.
    assert_eq!(confirmed_names(&client).await, vec!["Spotify"]);
    let final_state = suggestions(&client).await;
    assert_eq!(final_state["suggestions"], json!([]), "a kept pattern is not still suggested");
    assert_eq!(final_state["hidden_as_kept"], 1);
    assert_eq!(final_state["hidden_as_ignored"], 1);

    // The rest of the data is untouched by any of this.
    let state = client.state().await;
    assert_eq!(state["transactions"].as_array().unwrap().len(), 6);
    assert_eq!(state["recurring_items"].as_array().unwrap().len(), 1);
    assert_eq!(state["dismissed_patterns"], json!(["netflix"]));
}

// ---- what is and isn't suggested -------------------------------------------

#[tokio::test]
async fn an_empty_ledger_suggests_nothing() {
    let client = Client::start().await;

    let response = suggestions(&client).await;

    assert_eq!(response["suggestions"], json!([]));
    assert_eq!(response["hidden_as_kept"], 0);
    assert_eq!(response["hidden_as_ignored"], 0);
    assert!(response["as_of"].as_str().unwrap().len() == 10, "as_of should be a YYYY-MM-DD date");
}

#[tokio::test]
async fn a_suggestion_carries_everything_needed_to_confirm_it() {
    let client = Client::start().await;
    let main_account = client.account_id("Main Account").await;
    seed_pattern(&client, "Netflix", 1599).await;

    let suggestion = suggestions(&client).await["suggestions"][0].clone();

    assert_eq!(suggestion["pattern_key"], "netflix");
    assert_eq!(suggestion["merchant"], "Netflix");
    assert_eq!(suggestion["kind"], "subscription");
    assert_eq!(suggestion["cadence"], "monthly");
    assert_eq!(suggestion["occurrences"], 3);
    assert_eq!(suggestion["confidence"], "high");
    assert_eq!(suggestion["average_amount_cents"], 1599);
    assert_eq!(suggestion["monthly_equivalent_cents"], 1599);
    assert_eq!(suggestion["account_id"], main_account);
    assert_eq!(suggestion["last_seen"], months_ago(0));
    assert!(suggestion["next_expected_date"].as_str().unwrap() > suggestion["last_seen"].as_str().unwrap());
}

#[tokio::test]
async fn a_recurring_bill_is_told_apart_from_a_subscription() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;
    seed_pattern(&client, "Acme Mortgage", 180_000).await;

    let listed = suggestions(&client).await["suggestions"].clone();
    let by_merchant = |name: &str| {
        listed.as_array().unwrap().iter().find(|s| s["merchant"] == name).cloned().unwrap()
    };

    assert_eq!(by_merchant("Netflix")["kind"], "subscription");
    assert_eq!(by_merchant("Acme Mortgage")["kind"], "recurring");
    // The costliest per month comes first, which is the order a user cares about.
    assert_eq!(listed[0]["merchant"], "Acme Mortgage");
}

#[tokio::test]
async fn ordinary_repeat_shopping_is_not_suggested() {
    let client = Client::start().await;

    // Weekly, perfectly regular, perfectly stable — and not a bill.
    let weekly: Vec<Value> = (0..5)
        .map(|week| {
            let date = Utc::now()
                .date_naive()
                .checked_sub_days(chrono::Days::new(week * 7))
                .unwrap()
                .format("%Y-%m-%d")
                .to_string();
            json!({ "date": date, "merchant": "Trader Joes", "amount_cents": 8500,
                    "kind": "expense", "account": "Main Account" })
        })
        .collect();
    client.post("/api/v1/transactions", json!(weekly)).await.expect(StatusCode::CREATED);

    assert!(suggested_merchants(&client).await.is_empty());
}

// ---- keep and ignore are mutually exclusive --------------------------------

#[tokio::test]
async fn a_pattern_cannot_be_both_kept_and_ignored() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;
    client.post("/api/v1/recurring/netflix/keep", json!({})).await.expect(StatusCode::CREATED);

    // Ignoring something already confirmed is refused, and says what to do.
    let refused =
        client.post("/api/v1/recurring/netflix/ignore", json!({})).await.expect(StatusCode::CONFLICT);
    assert!(
        refused["message"].as_str().unwrap().contains("DELETE /recurring-items"),
        "{}",
        refused["message"]
    );

    // Keeping it twice is refused too — two rows would double-count.
    client.post("/api/v1/recurring/netflix/keep", json!({})).await.expect(StatusCode::CONFLICT);

    assert_eq!(client.state().await["dismissed_patterns"], json!([]), "nothing was dismissed");
}

#[tokio::test]
async fn keeping_a_pattern_overrides_an_earlier_dismissal() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;

    client.post("/api/v1/recurring/netflix/ignore", json!({})).await.expect(StatusCode::OK);
    assert_eq!(client.state().await["dismissed_patterns"], json!(["netflix"]));

    // Changing your mind should not need an undo first.
    client.post("/api/v1/recurring/netflix/keep", json!({})).await.expect(StatusCode::CREATED);

    assert_eq!(
        client.state().await["dismissed_patterns"],
        json!([]),
        "keeping clears the dismissal, so the pattern is in one state only"
    );
    assert_eq!(confirmed_names(&client).await, vec!["Netflix"]);
}

#[tokio::test]
async fn deleting_a_confirmed_item_lets_the_pattern_be_suggested_again() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;

    let kept =
        client.post("/api/v1/recurring/netflix/keep", json!({})).await.expect(StatusCode::CREATED);
    let id = kept["id"].as_str().unwrap().to_string();
    assert!(suggested_merchants(&client).await.is_empty());

    let deleted =
        client.delete(&format!("/api/v1/recurring-items/{id}")).await.expect(StatusCode::OK);
    assert_eq!(deleted["pattern_key"], "netflix");

    assert_eq!(
        suggested_merchants(&client).await,
        vec!["Netflix"],
        "with the item gone, the pattern is open again"
    );
    assert!(confirmed_names(&client).await.is_empty());
}

#[tokio::test]
async fn keeping_accepts_the_users_edits() {
    let client = Client::start().await;
    let dining = client.category_id("Dining").await;
    seed_pattern(&client, "Gold's Gym", 4500).await;

    // Detection calls a gym a subscription; the user knows this one is a loan.
    // "Gold's Gym" normalises to "golds gym", and the space is percent-encoded
    // because the key is a path segment.
    let kept = client
        .post(
            "/api/v1/recurring/golds%20gym/keep",
            json!({ "name": "Gym loan", "kind": "recurring", "amount_cents": 5000,
                    "category_id": dining, "next_date": "2027-01-15" }),
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(kept["name"], "Gym loan");
    assert_eq!(kept["kind"], "recurring");
    assert_eq!(kept["amount_cents"], 5000);
    assert_eq!(kept["category_id"], dining);
    assert_eq!(kept["next_date"], "2027-01-15");
    assert_eq!(kept["cadence"], "monthly", "what wasn't overridden comes from detection");
}

#[tokio::test]
async fn keeping_something_that_is_not_being_suggested_is_a_404() {
    let client = Client::start().await;

    let missing = client
        .post("/api/v1/recurring/never-seen-this/keep", json!({}))
        .await
        .expect(StatusCode::NOT_FOUND);
    assert!(
        missing["message"].as_str().unwrap().contains("POST /recurring-items"),
        "{}",
        missing["message"]
    );
}

#[tokio::test]
async fn a_pattern_key_is_normalised_however_it_arrives() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;

    // The client sends the merchant as displayed rather than the exact key.
    client
        .post("/api/v1/recurring/NETFLIX%20%231234/ignore", json!({}))
        .await
        .expect(StatusCode::OK);

    assert_eq!(client.state().await["dismissed_patterns"], json!(["netflix"]));
    assert!(suggested_merchants(&client).await.is_empty());
}

// ---- confirmed items, added and edited by hand -----------------------------

#[tokio::test]
async fn an_item_can_be_added_edited_and_removed_by_hand() {
    let client = Client::start().await;
    let utilities = client.category_id("Utilities").await;
    let cash = client.account_id("Cash").await;

    let created = client
        .post(
            "/api/v1/recurring-items",
            json!({ "name": "Council tax", "amount_cents": 21_500, "cadence": "monthly",
                    "kind": "recurring", "category_id": utilities, "next_date": "2027-01-01" }),
        )
        .await
        .expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "Council tax");

    let patched = client
        .patch(
            &format!("/api/v1/recurring-items/{id}"),
            json!({ "amount_cents": 22_000, "account_id": cash, "active": false }),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["amount_cents"], 22_000);
    assert_eq!(patched["account_id"], cash);
    assert_eq!(patched["active"], false);
    assert_eq!(patched["name"], "Council tax", "untouched fields stay put");
    assert_eq!(patched["category_id"], utilities);

    // An explicit null clears a field.
    let cleared = client
        .patch(&format!("/api/v1/recurring-items/{id}"), json!({ "category_id": null }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(cleared["category_id"], Value::Null);

    client.delete(&format!("/api/v1/recurring-items/{id}")).await.expect(StatusCode::OK);
    client.get(&format!("/api/v1/recurring-items/{id}")).await.expect(StatusCode::NOT_FOUND);
    client.delete(&format!("/api/v1/recurring-items/{id}")).await.expect(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_hand_added_item_suppresses_the_matching_suggestion() {
    let client = Client::start().await;
    seed_pattern(&client, "Netflix", 1599).await;
    assert_eq!(suggested_merchants(&client).await, vec!["Netflix"]);

    // Added by hand, spelled differently, but the same merchant once normalised.
    client
        .post(
            "/api/v1/recurring-items",
            json!({ "name": "NETFLIX #99", "amount_cents": 1599, "cadence": "monthly",
                    "kind": "subscription" }),
        )
        .await
        .expect(StatusCode::CREATED);

    assert!(
        suggested_merchants(&client).await.is_empty(),
        "the user has already told us about this one"
    );
}

#[tokio::test]
async fn hand_added_items_are_validated() {
    let client = Client::start().await;
    let stranger = uuid::Uuid::new_v4();

    for body in [
        json!({ "name": "  ", "amount_cents": 100, "cadence": "monthly", "kind": "recurring" }),
        json!({ "name": "Rent", "amount_cents": 0, "cadence": "monthly", "kind": "recurring" }),
        json!({ "name": "Rent", "amount_cents": -1, "cadence": "monthly", "kind": "recurring" }),
        json!({ "name": "Rent", "amount_cents": 100, "cadence": "fortnightly", "kind": "recurring" }),
        json!({ "name": "Rent", "amount_cents": 100, "cadence": "monthly", "kind": "bill" }),
        json!({ "name": "Rent", "amount_cents": 100, "cadence": "monthly", "kind": "recurring",
                "next_date": "2026-02-30" }),
        json!({ "name": "Rent", "amount_cents": 100, "cadence": "monthly", "kind": "recurring",
                "category_id": stranger }),
    ] {
        let reply = client.post("/api/v1/recurring-items", body.clone()).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body} should have been refused");
    }
}

#[tokio::test]
async fn the_recurring_api_is_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;

    for request in [
        common::get("/api/v1/recurring/suggestions"),
        common::get("/api/v1/recurring-items"),
        common::post_json("/api/v1/recurring/netflix/keep", json!({})),
        common::post_json("/api/v1/recurring/netflix/ignore", json!({})),
    ] {
        let path = request.uri().path().to_string();
        let reply = common::call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable unauthenticated");
    }
}
