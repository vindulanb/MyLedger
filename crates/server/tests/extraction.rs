//! End-to-end tests of document extraction.
//!
//! The matching engine is tested in `core::textrules` and the text reader in
//! `server::text`; these cover what only the whole pipeline can show — that
//! an unambiguous reading becomes a transaction with the receipt attached,
//! that anything less leaves the document for a human, and that re-running an
//! extraction never produces a second transaction for the same receipt.
//!
//! The uploads here are `.txt` files. That is not a shortcut: a text file goes
//! through the identical path a PDF's extracted text does, and building a
//! valid PDF by hand in a test would be testing `pdf-extract` rather than
//! this code.

mod common;

use axum::http::StatusCode;
use common::{Client, Part, find_named};
use serde_json::{Value, json};

const RECEIPT: &str = "CORNER GROCER #4411\n\
                       123 Example Street\n\
                       Date: 2026-10-04\n\
                       Bananas 2.40\n\
                       Milk 1.80\n\
                       TOTAL: 62.14\n\
                       Thank you\n";

/// Uploads a text document and returns its ID.
async fn upload(client: &Client, filename: &str, body: &str) -> String {
    let response = client
        .upload(
            "/api/v1/documents",
            &[Part::file("file", filename, "text/plain", body.as_bytes().to_vec())],
        )
        .await
        .expect(StatusCode::CREATED);
    response["documents"][0]["id"].as_str().expect("an id").to_string()
}

/// A rule that reads the total out of the receipt above.
fn total_rule() -> Value {
    json!({
        "label": "Corner Grocer receipt",
        "body_regex": r"(?i)date:\s*(\S+)[\s\S]*?total:\s*([0-9.,]+)",
        "date_group": 1,
        "amount_group": 2,
        "kind_hint": "debit",
    })
}

async fn extract(client: &Client, id: &str) -> Value {
    client.post(&format!("/api/v1/documents/{id}/extract"), json!({})).await.expect(StatusCode::OK)
}

// ---- the rules -------------------------------------------------------------

#[tokio::test]
async fn a_ledger_starts_with_no_extraction_rules() {
    let client = Client::start().await;
    let rules = client.get("/api/v1/extraction-rules").await.expect(StatusCode::OK);
    assert!(rules.as_array().unwrap().is_empty(), "none are seeded");
    assert!(
        client.state().await["document_extraction_rules"].as_array().unwrap().is_empty(),
        "and none appear in the state"
    );
}

#[tokio::test]
async fn a_rule_can_be_created_and_read_back() {
    let client = Client::start().await;

    let created =
        client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    assert_eq!(created["label"], "Corner Grocer receipt");
    assert_eq!(created["amount_group"], 2);
    assert_eq!(created["enabled"], true);

    let id = created["id"].as_str().unwrap();
    let fetched =
        client.get(&format!("/api/v1/extraction-rules/{id}")).await.expect(StatusCode::OK);
    assert_eq!(fetched["id"], created["id"]);

    // And it reaches the clients through the state.
    let state = client.state().await;
    assert_eq!(state["document_extraction_rules"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_rule_that_could_never_work_is_refused_when_it_is_written() {
    let client = Client::start().await;

    // No amount group: nothing this rule matched could become a transaction.
    let refusal = client
        .post("/api/v1/extraction-rules", json!({"label": "x", "body_regex": "total"}))
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(refusal["message"].as_str().unwrap().contains("amount"), "{}", refusal["message"]);

    // A pattern that does not compile.
    client
        .post(
            "/api/v1/extraction-rules",
            json!({"label": "x", "body_regex": "total ([0-9", "amount_group": 1}),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);

    // A group the pattern does not have.
    let refusal = client
        .post(
            "/api/v1/extraction-rules",
            json!({"label": "x", "body_regex": "total ([0-9.]+)", "amount_group": 1,
                   "merchant_group": 7}),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(refusal["message"].as_str().unwrap().contains("merchant"), "{}", refusal["message"]);

    // Group 0 is the whole match, so it can never be the amount.
    client
        .post(
            "/api/v1/extraction-rules",
            json!({"label": "x", "body_regex": "total ([0-9.]+)", "amount_group": 0}),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);

    assert!(
        client.get("/api/v1/extraction-rules").await.expect(StatusCode::OK).as_array().unwrap().is_empty(),
        "nothing was stored"
    );
}

#[tokio::test]
async fn a_rule_can_be_edited_and_disabled() {
    let client = Client::start().await;
    let created =
        client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap();

    let patched = client
        .patch(&format!("/api/v1/extraction-rules/{id}"), json!({"enabled": false}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["enabled"], false);
    // Everything else is left alone.
    assert_eq!(patched["body_regex"], created["body_regex"]);

    let patched = client
        .patch(&format!("/api/v1/extraction-rules/{id}"), json!({"label": "Renamed"}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["label"], "Renamed");
    assert_eq!(patched["enabled"], false, "the earlier change survived");
}

#[tokio::test]
async fn an_edit_that_would_break_a_rule_is_refused() {
    let client = Client::start().await;
    let created =
        client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap();

    client
        .patch(&format!("/api/v1/extraction-rules/{id}"), json!({"body_regex": "total ([0-9"}))
        .await
        .expect(StatusCode::BAD_REQUEST);

    // The stored rule is untouched.
    let fetched =
        client.get(&format!("/api/v1/extraction-rules/{id}")).await.expect(StatusCode::OK);
    assert_eq!(fetched["body_regex"], created["body_regex"]);
}

#[tokio::test]
async fn a_rule_can_be_deleted() {
    let client = Client::start().await;
    let created =
        client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = created["id"].as_str().unwrap();

    client.delete(&format!("/api/v1/extraction-rules/{id}")).await.expect(StatusCode::OK);
    client.get(&format!("/api/v1/extraction-rules/{id}")).await.expect(StatusCode::NOT_FOUND);
    client.delete(&format!("/api/v1/extraction-rules/{id}")).await.expect(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_rule_may_not_point_at_a_category_that_does_not_exist() {
    let client = Client::start().await;
    let stranger = uuid::Uuid::new_v4();
    let mut rule = total_rule();
    rule["default_category_id"] = json!(stranger);
    client.post("/api/v1/extraction-rules", rule).await.expect(StatusCode::BAD_REQUEST);
}

// ---- running an extraction -------------------------------------------------

#[tokio::test]
async fn with_no_rules_a_document_is_read_but_matches_nothing() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let result = extract(&client, &id).await;
    assert_eq!(result["outcome"], "no-match");
    assert_eq!(result["rules_tried"], 0, "so the UI can say 'no rules yet' rather than 'no match'");
    // The text is returned, which is how the user writes a rule against it.
    assert!(result["text"].as_str().unwrap().contains("TOTAL: 62.14"));
    assert_eq!(result["document"]["status"], "review");
    assert!(result["created_transaction"].is_null());
    assert!(client.state().await["transactions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn an_unambiguous_match_becomes_a_transaction_with_the_receipt_attached() {
    let client = Client::start().await;
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let result = extract(&client, &id).await;
    assert_eq!(result["outcome"], "matched");
    assert_eq!(result["rules_tried"], 1);

    let created = &result["created_transaction"];
    assert_eq!(created["amount_cents"], 6214);
    assert_eq!(created["date"], "2026-10-04", "read from the receipt, not the upload date");
    assert_eq!(created["kind"], "expense");
    assert_eq!(created["source"], "document");
    assert_eq!(created["receipt"], true, "the document being read is the receipt");

    // No merchant group, so the filename stands in and the user can see where
    // the name came from.
    assert_eq!(created["merchant"], "receipt.txt");

    assert_eq!(result["document"]["status"], "stored");
    assert_eq!(result["document"]["linked_transaction_id"], created["id"]);

    let state = client.state().await;
    assert_eq!(state["transactions"].as_array().unwrap().len(), 1);
    assert_eq!(state["documents"][0]["linked_transaction_id"], created["id"]);
}

#[tokio::test]
async fn a_captured_merchant_is_used_when_the_rule_has_one() {
    let client = Client::start().await;
    client
        .post(
            "/api/v1/extraction-rules",
            json!({
                "label": "named",
                "body_regex": r"(?i)^(.+?)\s*#\d+[\s\S]*?total:\s*([0-9.,]+)",
                "merchant_group": 1,
                "amount_group": 2,
            }),
        )
        .await
        .expect(StatusCode::CREATED);

    let id = upload(&client, "receipt.txt", RECEIPT).await;
    let result = extract(&client, &id).await;
    assert_eq!(result["created_transaction"]["merchant"], "CORNER GROCER");
}

#[tokio::test]
async fn two_rules_that_disagree_leave_the_document_for_a_human() {
    let client = Client::start().await;

    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    // Reads a line item instead of the total: a different, equally valid-
    // looking answer.
    client
        .post(
            "/api/v1/extraction-rules",
            json!({"label": "bananas", "body_regex": r"Bananas ([0-9.]+)", "amount_group": 1}),
        )
        .await
        .expect(StatusCode::CREATED);

    let id = upload(&client, "receipt.txt", RECEIPT).await;
    let result = extract(&client, &id).await;

    assert_eq!(result["outcome"], "ambiguous");
    assert_eq!(result["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(result["document"]["status"], "review");
    assert!(result["created_transaction"].is_null(), "guessing between them is not allowed");
    assert!(client.state().await["transactions"].as_array().unwrap().is_empty());

    // Both readings are offered, so the user can pick.
    let amounts: Vec<i64> = result["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["amount_cents"].as_i64().unwrap())
        .collect();
    assert!(amounts.contains(&6214) && amounts.contains(&240), "{amounts:?}");
}

#[tokio::test]
async fn a_file_with_no_text_says_why_and_waits_for_the_user() {
    let client = Client::start().await;
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);

    let response = client
        .upload(
            "/api/v1/documents",
            &[Part::file("file", "photo.jpg", "image/jpeg", b"\xff\xd8\xff\xe0 not text".to_vec())],
        )
        .await
        .expect(StatusCode::CREATED);
    let id = response["documents"][0]["id"].as_str().unwrap().to_string();

    let result = extract(&client, &id).await;
    assert_eq!(result["outcome"], "no-text");
    // The honest explanation, on the screen rather than in a log.
    let note = result["note"].as_str().unwrap();
    assert!(note.contains("no OCR"), "{note}");
    assert_eq!(result["document"]["status"], "review");
    assert!(result["text"].is_null());
}

#[tokio::test]
async fn re_running_an_extraction_does_not_create_a_second_transaction() {
    let client = Client::start().await;
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let first = extract(&client, &id).await;
    let transaction_id = first["created_transaction"]["id"].clone();

    // Re-running after writing a rule is the normal way to use this, so it
    // must be safe.
    let again = extract(&client, &id).await;
    assert_eq!(again["outcome"], "matched");
    assert!(again["created_transaction"].is_null());
    assert_eq!(again["document"]["linked_transaction_id"], transaction_id);

    assert_eq!(client.state().await["transactions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_receipt_for_a_transaction_already_on_file_is_linked_to_it() {
    let client = Client::start().await;

    // The same purchase, entered by hand first.
    let state = client.state().await;
    let account = find_named(&state["accounts"], "Main Account");
    let manual = client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-10-04",
                "merchant": "receipt.txt",
                "amount_cents": 6214,
                "kind": "expense",
            }),
        )
        .await
        .expect(StatusCode::CREATED);
    let manual_id = manual["transactions"][0]["id"].clone();
    let _ = account;

    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let result = extract(&client, &id).await;
    // Linked to what was already there rather than duplicating it: the
    // extraction goes through the same insert path as everything else.
    assert!(result["created_transaction"].is_null());
    assert_eq!(result["document"]["linked_transaction_id"], manual_id);
    assert_eq!(result["document"]["status"], "stored");
    assert_eq!(client.state().await["transactions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_disabled_rule_is_not_tried() {
    let client = Client::start().await;
    let created =
        client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let rule_id = created["id"].as_str().unwrap();
    client
        .patch(&format!("/api/v1/extraction-rules/{rule_id}"), json!({"enabled": false}))
        .await
        .expect(StatusCode::OK);

    let id = upload(&client, "receipt.txt", RECEIPT).await;
    let result = extract(&client, &id).await;
    assert_eq!(result["outcome"], "no-match");
    assert_eq!(result["rules_tried"], 0);
}

#[tokio::test]
async fn a_filename_pattern_restricts_which_documents_a_rule_tries() {
    let client = Client::start().await;
    let mut rule = total_rule();
    rule["filename_pattern"] = json!("^grocer");
    client.post("/api/v1/extraction-rules", rule).await.expect(StatusCode::CREATED);

    let other = upload(&client, "receipt.txt", RECEIPT).await;
    assert_eq!(extract(&client, &other).await["outcome"], "no-match");

    let matching = upload(&client, "grocer-oct.txt", RECEIPT).await;
    assert_eq!(extract(&client, &matching).await["outcome"], "matched");
}

#[tokio::test]
async fn extracting_a_document_that_does_not_exist_is_a_404() {
    let client = Client::start().await;
    let stranger = uuid::Uuid::new_v4();
    client
        .post(&format!("/api/v1/documents/{stranger}/extract"), json!({}))
        .await
        .expect(StatusCode::NOT_FOUND);
}

// ---- reading back what was found -------------------------------------------

#[tokio::test]
async fn the_stored_text_can_be_read_back_and_is_matched_against_the_current_rules() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    // Read once with no rules.
    extract(&client, &id).await;
    let stored = client
        .get(&format!("/api/v1/documents/{id}/extraction"))
        .await
        .expect(StatusCode::OK);
    assert_eq!(stored["outcome"], "no-match");
    assert!(stored["text"].as_str().unwrap().contains("TOTAL: 62.14"));

    // Write a rule afterwards. The stored reading is recomputed, not
    // remembered, so it reflects the rules as they are now.
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let stored = client
        .get(&format!("/api/v1/documents/{id}/extraction"))
        .await
        .expect(StatusCode::OK);
    assert_eq!(stored["outcome"], "matched");
    assert_eq!(stored["candidates"][0]["amount_cents"], 6214);
}

#[tokio::test]
async fn reading_back_a_document_that_was_never_extracted_is_a_404() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;
    client
        .get(&format!("/api/v1/documents/{id}/extraction"))
        .await
        .expect(StatusCode::NOT_FOUND);
}

// ---- filing a document by hand ---------------------------------------------

#[tokio::test]
async fn a_document_can_be_linked_to_a_transaction_by_hand() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let manual = client
        .post(
            "/api/v1/transactions",
            json!({"date": "2026-10-04", "merchant": "Corner Grocer",
                   "amount_cents": 6214, "kind": "expense"}),
        )
        .await
        .expect(StatusCode::CREATED);
    let transaction_id = manual["transactions"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(manual["transactions"][0]["receipt"], false);

    let patched = client
        .patch(
            &format!("/api/v1/documents/{id}"),
            json!({"linked_transaction_id": transaction_id, "status": "stored"}),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["linked_transaction_id"], transaction_id);
    assert_eq!(patched["status"], "stored");

    // And the transaction now says it has a receipt.
    let state = client.state().await;
    assert_eq!(state["transactions"][0]["receipt"], true);
}

#[tokio::test]
async fn a_documents_status_can_be_changed_on_its_own() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let patched = client
        .patch(&format!("/api/v1/documents/{id}"), json!({"status": "review"}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(patched["status"], "review");
    assert!(patched["linked_transaction_id"].is_null(), "the link was left alone");
}

#[tokio::test]
async fn a_link_can_be_cleared_with_an_explicit_null() {
    let client = Client::start().await;
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = upload(&client, "receipt.txt", RECEIPT).await;
    extract(&client, &id).await;

    let patched = client
        .patch(&format!("/api/v1/documents/{id}"), json!({"linked_transaction_id": null}))
        .await
        .expect(StatusCode::OK);
    assert!(patched["linked_transaction_id"].is_null());
}

#[tokio::test]
async fn linking_to_a_transaction_that_does_not_exist_is_refused() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;
    let stranger = uuid::Uuid::new_v4();

    client
        .patch(
            &format!("/api/v1/documents/{id}"),
            json!({"linked_transaction_id": stranger}),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn linking_to_a_deleted_transaction_is_refused() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;

    let manual = client
        .post(
            "/api/v1/transactions",
            json!({"date": "2026-10-04", "merchant": "Gone", "amount_cents": 100,
                   "kind": "expense"}),
        )
        .await
        .expect(StatusCode::CREATED);
    let transaction_id = manual["transactions"][0]["id"].as_str().unwrap().to_string();
    client.delete(&format!("/api/v1/transactions/{transaction_id}")).await.expect(StatusCode::OK);

    client
        .patch(
            &format!("/api/v1/documents/{id}"),
            json!({"linked_transaction_id": transaction_id}),
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_patch_with_nothing_in_it_is_a_bad_request() {
    let client = Client::start().await;
    let id = upload(&client, "receipt.txt", RECEIPT).await;
    client.patch(&format!("/api/v1/documents/{id}"), json!({})).await.expect(StatusCode::BAD_REQUEST);
}

// ---- the wipe --------------------------------------------------------------

#[tokio::test]
async fn the_wipe_removes_extraction_rules_and_stored_text() {
    let client = Client::start().await;
    client.post("/api/v1/extraction-rules", total_rule()).await.expect(StatusCode::CREATED);
    let id = upload(&client, "receipt.txt", RECEIPT).await;
    extract(&client, &id).await;

    let report = client
        .delete_with("/api/v1/state", json!({"confirm": "DELETE ALL MYLEDGER DATA"}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(report["deleted"]["document_extraction_rules"], 1);
    assert_eq!(report["deleted"]["document_extractions"], 1);

    let state = client.state().await;
    assert!(state["document_extraction_rules"].as_array().unwrap().is_empty());
    assert!(state["documents"].as_array().unwrap().is_empty());
}

// ---- the gate --------------------------------------------------------------

#[tokio::test]
async fn every_extraction_route_is_behind_the_same_gate() {
    let (app, _db, _dir) = common::app_with_user().await;
    let stranger = uuid::Uuid::new_v4();

    for request in [
        common::get("/api/v1/extraction-rules"),
        common::get(&format!("/api/v1/extraction-rules/{stranger}")),
        common::get(&format!("/api/v1/documents/{stranger}/extraction")),
    ] {
        let path = request.uri().path().to_string();
        let reply = common::call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable");
    }
}
