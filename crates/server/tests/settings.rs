//! The endpoints behind the Settings screen.
//!
//! Three things that screen relies on, and that nothing else in the suite
//! pins down: that net worth can be turned *off* again without losing the
//! figures, that an empty `dismissed_patterns` list is how "Restore ignored
//! suggestions" works, and that the managed lists behave the way the screen
//! tells the user they will — a delete that hides a name without losing the
//! history filed under it.
//!
//! Pairing itself is covered in `auth.rs`; what is checked here is the small
//! part Settings adds to it, which is that the list it shows carries no
//! secret of any kind.

mod common;

use axum::http::StatusCode;
use common::{Client, find_named};
use serde_json::{Value, json};

// ---- net worth -------------------------------------------------------------

#[tokio::test]
async fn net_worth_can_be_turned_off_without_losing_the_figures() {
    let client = Client::start().await;

    client
        .put(
            "/api/v1/preferences",
            json!({
                "assets_cents": 1_500_000,
                "liabilities_cents": 400_000,
                "net_worth_configured": true,
            }),
        )
        .await
        .expect(StatusCode::OK);

    // Only the flag. The amounts are left out of the request entirely, which
    // is what the "Stop tracking" button sends.
    let off = client
        .put("/api/v1/preferences", json!({ "net_worth_configured": false }))
        .await
        .expect(StatusCode::OK);

    let settings = &off["settings"];
    assert_eq!(settings["net_worth_configured"], false);
    assert_eq!(settings["assets_cents"], 1_500_000);
    assert_eq!(settings["liabilities_cents"], 400_000);

    // And turning it back on needs nothing retyped.
    let on = client
        .put("/api/v1/preferences", json!({ "net_worth_configured": true }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(on["settings"]["assets_cents"], 1_500_000);
    assert_eq!(on["settings"]["liabilities_cents"], 400_000);
}

#[tokio::test]
async fn a_negative_asset_figure_is_refused() {
    let client = Client::start().await;

    let refused = client
        .put("/api/v1/preferences", json!({ "assets_cents": -1 }))
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(
        refused["message"].as_str().unwrap().contains("assets"),
        "{}",
        refused["message"]
    );

    // A debt goes under liabilities as a positive number, so a negative one
    // there is equally meaningless.
    client
        .put("/api/v1/preferences", json!({ "liabilities_cents": -500 }))
        .await
        .expect(StatusCode::BAD_REQUEST);

    // Neither attempt changed anything.
    let settings = client.state().await;
    assert_eq!(settings["settings"]["assets_cents"], 0);
    assert_eq!(settings["settings"]["liabilities_cents"], 0);
}

#[tokio::test]
async fn net_worth_is_never_derived_from_the_ledger() {
    let client = Client::start().await;

    client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-10-01",
                "merchant": "Salary",
                "amount_cents": 500_000,
                "kind": "income",
            }),
        )
        .await
        .expect(StatusCode::CREATED);

    // Income arrived, and net worth is still unset — the figure is input, not
    // a running total of what passed through.
    let state = client.state().await;
    assert_eq!(state["settings"]["net_worth_configured"], false);
    assert_eq!(state["settings"]["assets_cents"], 0);
}

// ---- restoring ignored suggestions ------------------------------------------

#[tokio::test]
async fn an_empty_list_restores_every_ignored_suggestion() {
    let client = Client::start().await;

    client
        .put("/api/v1/preferences", json!({ "dismissed_patterns": ["netflix", "spotify"] }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(client.state().await["dismissed_patterns"].as_array().unwrap().len(), 2);

    let restored = client
        .put("/api/v1/preferences", json!({ "dismissed_patterns": [] }))
        .await
        .expect(StatusCode::OK);

    assert_eq!(restored["dismissed_patterns"], json!([]));
    assert_eq!(client.state().await["dismissed_patterns"], json!([]));
}

#[tokio::test]
async fn omitting_the_list_is_not_the_same_as_emptying_it() {
    // The distinction the whole preferences endpoint turns on, checked here
    // because Settings is the only screen with a button that empties a group
    // on purpose.
    let client = Client::start().await;

    client
        .put("/api/v1/preferences", json!({ "dismissed_patterns": ["netflix"] }))
        .await
        .expect(StatusCode::OK);

    client
        .put("/api/v1/preferences", json!({ "assets_cents": 100 }))
        .await
        .expect(StatusCode::OK);

    assert_eq!(client.state().await["dismissed_patterns"], json!(["netflix"]));
}

// ---- the managed lists -------------------------------------------------------

#[tokio::test]
async fn a_deleted_category_stops_being_offered_but_keeps_naming_its_history() {
    let client = Client::start().await;
    let groceries = client.category_id("Groceries").await;

    client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-09-14",
                "merchant": "Corner grocer",
                "amount_cents": 4_210,
                "kind": "expense",
                "category_id": groceries,
            }),
        )
        .await
        .expect(StatusCode::CREATED);

    // The sync sends everything being kept, which is how a delete is
    // expressed: the row simply isn't in the list.
    let keeping: Vec<Value> = client.state().await["categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["id"] != json!(groceries))
        .map(|c| json!({ "id": c["id"], "name": c["name"] }))
        .collect();
    client
        .put("/api/v1/preferences", json!({ "categories": keeping }))
        .await
        .expect(StatusCode::OK);

    let state = client.state().await;
    let row = state["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == json!(groceries))
        .expect("the category is still returned so old rows can be named");
    assert!(!row["deleted_at"].is_null(), "it should be soft-deleted, not gone");
    assert_eq!(row["name"], "Groceries");

    // And the transaction still points at it.
    assert_eq!(state["transactions"][0]["category_id"], json!(groceries));
}

#[tokio::test]
async fn two_names_differing_only_in_case_are_refused() {
    let client = Client::start().await;

    let refused = client
        .put(
            "/api/v1/preferences",
            json!({ "accounts": [{ "name": "Current" }, { "name": "current" }] }),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(
        refused["message"].as_str().unwrap().contains("two account"),
        "{}",
        refused["message"]
    );
}

#[tokio::test]
async fn adding_a_name_that_already_exists_reuses_the_row_rather_than_doubling_it() {
    // The Settings add form checks for this and refuses, so the user never
    // gets here — but if they did, re-sending a live name must not churn the
    // ID and orphan everything filed against it.
    let client = Client::start().await;
    let before = client.category_id("Groceries").await;

    let mut keeping: Vec<Value> = client.state().await["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({ "id": c["id"], "name": c["name"] }))
        .collect();
    keeping.push(json!({ "name": "groceries" }));

    client.put("/api/v1/preferences", json!({ "categories": keeping })).await;

    let state = client.state().await;
    let live: Vec<&Value> =
        state["categories"].as_array().unwrap().iter().filter(|c| c["deleted_at"].is_null()).collect();
    let matches = live.iter().filter(|c| c["name"] == "Groceries").count();
    assert_eq!(matches, 1, "no second Groceries should have appeared");
    assert_eq!(find_named(&state["categories"], "Groceries"), before);
}

// ---- the paired-devices list -------------------------------------------------

#[tokio::test]
async fn the_device_list_carries_no_secret() {
    let client = Client::start().await;

    let issued = client
        .post("/api/v1/auth/device-tokens", json!({ "label": "Pixel" }))
        .await
        .expect(StatusCode::OK);
    let token = issued["token"].as_str().expect("a token").to_string();
    assert!(!token.is_empty());

    let listed = client.get("/api/v1/auth/device-tokens").await.expect(StatusCode::OK);
    let body = serde_json::to_string(&listed).unwrap();

    assert!(!body.contains(&token), "the raw token must never be listed again");
    assert!(!body.contains("hash"), "nor the hash of it: {body}");
    assert_eq!(listed[0]["label"], "Pixel");
    assert!(listed[0]["last_used_at"].is_null(), "it has not been used yet");
}

#[tokio::test]
async fn an_unlabelled_device_is_still_listed() {
    let client = Client::start().await;

    client.post("/api/v1/auth/device-tokens", json!({})).await.expect(StatusCode::OK);

    let listed = client.get("/api/v1/auth/device-tokens").await.expect(StatusCode::OK);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(listed[0]["label"].is_null(), "the screen shows it as unnamed");
    assert!(listed[0]["created_at"].as_str().is_some());
}

#[tokio::test]
async fn the_warning_about_showing_the_token_once_is_part_of_the_response() {
    // The screen renders this rather than wording its own, so that the
    // promise the server is actually keeping is the one the user reads.
    let client = Client::start().await;

    let issued =
        client.post("/api/v1/auth/device-tokens", json!({})).await.expect(StatusCode::OK);
    let warning = issued["warning"].as_str().expect("a warning");
    assert!(warning.contains("once"), "{warning}");
}
