//! End-to-end tests of `DELETE /api/v1/state`.
//!
//! The wipe is the one irreversible action in the app, so the tests care as
//! much about what it *doesn't* do — on a wrong confirmation, and to the
//! credentials — as about what it does.

mod common;

use axum::http::StatusCode;
use common::{Client, Part};
use serde_json::{Value, json};

const PDF: &[u8] = b"%PDF-1.4\nreceipt\n%%EOF";
const CONFIRM: &str = "DELETE ALL MYLEDGER DATA";

/// Fills a ledger with one of everything the wipe is supposed to remove, plus
/// a couple of customisations it is supposed to leave alone.
async fn populate(client: &Client) {
    let groceries = client.category_id("Groceries").await;

    // A renamed category and an extra account: curated, not financial.
    let categories = client.state().await["categories"].clone();
    let renamed: Vec<Value> = categories
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let name = if c["name"] == "Other" { json!("Miscellaneous") } else { c["name"].clone() };
            json!({ "id": c["id"], "name": name })
        })
        .collect();
    let accounts = client.state().await["accounts"].clone();
    let mut accounts_sent: Vec<Value> = accounts
        .as_array()
        .unwrap()
        .iter()
        .map(|a| json!({ "id": a["id"], "name": a["name"] }))
        .collect();
    accounts_sent.push(json!({ "name": "Savings" }));

    client
        .put(
            "/api/v1/preferences",
            json!({
                "categories": renamed,
                "accounts": accounts_sent,
                "tags": [{ "name": "essential" }],
                "rules": [{ "when_text": "starbucks", "then_category_id": groceries }],
                "budgets": [{ "category_id": groceries, "monthly_limit_cents": 40_000 }],
                "goals": [{ "name": "Emergency fund", "target_cents": 500_000 }],
                "recurring_items": [{ "name": "Netflix", "amount_cents": 1599,
                                      "cadence": "monthly", "kind": "subscription" }],
                "dismissed_patterns": ["spotify"],
                "assets_cents": 9_000_000,
                "liabilities_cents": 1_000_000,
                "net_worth_configured": true,
                "selected_period": "this-year",
            }),
        )
        .await
        .expect(StatusCode::OK);

    let tag = common::find_named(&client.state().await["tags"], "essential");
    client
        .post(
            "/api/v1/transactions",
            json!([
                { "date": "2026-03-04", "merchant": "Starbucks", "amount_cents": 545,
                  "kind": "expense", "account": "Main Account", "tags": [tag] },
                { "date": "2026-03-05", "merchant": "Whole Foods", "amount_cents": 8532,
                  "kind": "expense", "account": "Main Account" },
            ]),
        )
        .await
        .expect(StatusCode::CREATED);

    client
        .post(
            "/api/v1/sms-rules",
            json!({ "label": "Bank", "body_regex": r"Spent ([0-9.]+)", "amount_group": 1 }),
        )
        .await
        .expect(StatusCode::CREATED);

    client
        .upload("/api/v1/documents", &[Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec())])
        .await
        .expect(StatusCode::CREATED);
}

// ---- refusing ---------------------------------------------------------------

#[tokio::test]
async fn a_wrong_confirmation_changes_absolutely_nothing() {
    let client = Client::start().await;
    populate(&client).await;
    let before = client.state().await;
    let files_before = std::fs::read_dir(client.data_dir()).unwrap().count();

    for body in [
        json!({ "confirm": "delete all myledger data" }),
        json!({ "confirm": "DELETE ALL MY LEDGER DATA" }),
        json!({ "confirm": " DELETE ALL MYLEDGER DATA " }),
        json!({ "confirm": "yes" }),
        json!({ "confirm": "" }),
        json!({}),
        json!({ "confirmed": CONFIRM }),
        // Anything beyond the confirmation is refused rather than half-read.
        json!({ "confirm": CONFIRM, "also": "please" }),
    ] {
        let reply = client.delete_with("/api/v1/state", body.clone()).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body} should have been refused");
    }

    assert_eq!(client.state().await, before, "a refused wipe must leave the ledger untouched");
    assert_eq!(std::fs::read_dir(client.data_dir()).unwrap().count(), files_before);
}

#[tokio::test]
async fn a_refusal_says_what_to_send_instead() {
    let client = Client::start().await;

    let reply = client.delete_with("/api/v1/state", json!({ "confirm": "yes" })).await;
    let message = reply.body["message"].as_str().unwrap();

    assert!(message.contains(CONFIRM), "{message}");
    assert!(message.contains("nothing has been changed"), "{message}");
}

// ---- wiping -----------------------------------------------------------------

#[tokio::test]
async fn a_confirmed_wipe_clears_the_ledger_and_reports_what_went() {
    let client = Client::start().await;
    populate(&client).await;
    assert_eq!(std::fs::read_dir(client.data_dir()).unwrap().count(), 1);

    let report = client
        .delete_with("/api/v1/state", json!({ "confirm": CONFIRM }))
        .await
        .expect(StatusCode::OK);

    assert_eq!(report["wiped"], true);
    assert_eq!(report["deleted"]["transactions"], 2);
    assert_eq!(report["deleted"]["transaction_tags"], 1);
    assert_eq!(report["deleted"]["tags"], 1);
    assert_eq!(report["deleted"]["documents"], 1);
    assert_eq!(report["deleted"]["rules"], 1);
    assert_eq!(report["deleted"]["sms_parse_rules"], 1);
    assert_eq!(report["deleted"]["budgets"], 1);
    assert_eq!(report["deleted"]["goals"], 1);
    assert_eq!(report["deleted"]["recurring_items"], 1);
    assert_eq!(report["deleted"]["dismissed_patterns"], 1);
    assert_eq!(report["deleted"]["total"], 11);
    assert_eq!(report["document_files_removed"], 1);
    assert!(report["wipe_reset_at"].is_string());

    // Every financial group is empty again.
    let state = client.state().await;
    for group in [
        "transactions",
        "tags",
        "rules",
        "sms_parse_rules",
        "budgets",
        "goals",
        "recurring_items",
        "dismissed_patterns",
        "documents",
    ] {
        assert_eq!(state[group], json!([]), "{group} should be empty after a wipe");
    }

    // Settings are back to their fresh-install values, with the wipe recorded.
    assert_eq!(state["settings"]["selected_period"], "all-time");
    assert_eq!(state["settings"]["net_worth_configured"], false);
    assert_eq!(state["settings"]["assets_cents"], 0);
    assert_eq!(state["settings"]["liabilities_cents"], 0);
    assert_eq!(state["settings"]["wipe_reset_at"], report["wipe_reset_at"]);

    // And the bytes are gone from disk, not merely unreferenced.
    assert_eq!(std::fs::read_dir(client.data_dir()).unwrap().count(), 0);
}

#[tokio::test]
async fn the_curated_lookups_survive() {
    let client = Client::start().await;
    populate(&client).await;

    let report = client
        .delete_with("/api/v1/state", json!({ "confirm": CONFIRM }))
        .await
        .expect(StatusCode::OK);

    // Nothing needed restoring, because nothing was deleted.
    assert_eq!(report["starter_categories_restored"], 0);
    assert_eq!(report["starter_accounts_restored"], 0);
    assert_eq!(report["kept"]["categories"], 13);
    assert_eq!(report["kept"]["accounts"], 4);

    let state = client.state().await;
    let names = |list: &Value| -> Vec<String> {
        list.as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect()
    };
    assert!(names(&state["categories"]).contains(&"Miscellaneous".to_string()), "the rename survives");
    assert!(names(&state["accounts"]).contains(&"Savings".to_string()), "the added account survives");
}

#[tokio::test]
async fn emptied_lookups_are_restored_to_the_starter_names() {
    let client = Client::start().await;

    // The one case where the seed has work to do: the user deleted the lot.
    client
        .put("/api/v1/preferences", json!({ "categories": [], "accounts": [] }))
        .await
        .expect(StatusCode::OK);

    let report = client
        .delete_with("/api/v1/state", json!({ "confirm": CONFIRM }))
        .await
        .expect(StatusCode::OK);

    assert_eq!(report["starter_categories_restored"], 13);
    assert_eq!(report["starter_accounts_restored"], 3);

    let state = client.state().await;
    let live = |list: &Value| -> usize {
        list.as_array().unwrap().iter().filter(|c| c["deleted_at"].is_null()).count()
    };
    assert_eq!(live(&state["categories"]), 13);
    assert_eq!(live(&state["accounts"]), 3);
}

#[tokio::test]
async fn the_wipe_does_not_sign_you_out_or_unpair_your_phone() {
    let (app, _db, _dir) = common::app_with_user().await;
    let session = common::sign_in(&app).await;
    let device = common::pair_device(&app, &session).await;

    let wipe = common::call(
        &app,
        common::with_cookie(
            common::request_with_json("DELETE", "/api/v1/state", &json!({ "confirm": CONFIRM })),
            &session,
        ),
    )
    .await;
    assert_eq!(wipe.status, StatusCode::OK);
    assert_eq!(wipe.body["kept"]["signed_in"], true);

    // The same session cookie still works…
    let by_session = common::call(&app, common::with_cookie(common::get("/api/v1/me"), &session)).await;
    assert_eq!(by_session.status, StatusCode::OK, "the wipe must not end the session");

    // …and so does the paired device.
    let by_device = common::call(&app, common::with_bearer(common::get("/api/v1/me"), &device)).await;
    assert_eq!(by_device.status, StatusCode::OK, "the wipe must not unpair a device");

    // Signing in again from scratch works too.
    let fresh = common::sign_in(&app).await;
    assert_ne!(fresh, session);

    // The device is still listed, with its label intact.
    let devices =
        common::call(&app, common::with_cookie(common::get("/api/v1/auth/device-tokens"), &fresh)).await;
    assert_eq!(devices.body.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_wiped_transaction_can_be_imported_again() {
    let client = Client::start().await;
    let coffee = json!({ "date": "2026-03-04", "merchant": "Starbucks", "amount_cents": 545,
                         "kind": "expense", "account": "Main Account" });

    client.post("/api/v1/transactions", coffee.clone()).await.expect(StatusCode::CREATED);
    client.delete_with("/api/v1/state", json!({ "confirm": CONFIRM })).await.expect(StatusCode::OK);

    // The wipe is a hard delete, so the fingerprint is free again. A soft
    // delete would leave the row blocking re-import of data the user believes
    // is gone.
    let again = client.post("/api/v1/transactions", coffee).await.expect(StatusCode::CREATED);
    assert_eq!(again["inserted"], 1);
    assert_eq!(again["duplicates"], 0);
}

#[tokio::test]
async fn wiping_an_already_empty_ledger_is_harmless() {
    let client = Client::start().await;

    let first = client
        .delete_with("/api/v1/state", json!({ "confirm": CONFIRM }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(first["deleted"]["total"], 0);

    let second = client
        .delete_with("/api/v1/state", json!({ "confirm": CONFIRM }))
        .await
        .expect(StatusCode::OK);
    assert_eq!(second["deleted"]["total"], 0);

    let state = client.state().await;
    assert_eq!(state["transactions"], json!([]));
    assert_eq!(state["categories"].as_array().unwrap().len(), 13);
    assert_ne!(first["wipe_reset_at"], Value::Null);
    assert_eq!(state["settings"]["wipe_reset_at"], second["wipe_reset_at"]);
}

#[tokio::test]
async fn the_wipe_is_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;

    let reply = common::call(
        &app,
        common::request_with_json("DELETE", "/api/v1/state", &json!({ "confirm": CONFIRM })),
    )
    .await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "an unauthenticated wipe must be refused");
}
