//! End-to-end tests of document upload and retrieval.

mod common;

use axum::http::StatusCode;
use common::{Client, Part, find_named};
use myledger_server::api::documents::{MAX_FILE_BYTES, MAX_FILES};
use serde_json::{Value, json};

const PDF: &[u8] = b"%PDF-1.4\nfake but plausible receipt bytes\n%%EOF";

/// A file of exactly `bytes` bytes, filled with something compressible enough
/// that a 20 MiB one doesn't slow the test down.
fn filler(bytes: usize) -> Vec<u8> {
    vec![b'x'; bytes]
}

async fn upload_one(client: &Client, filename: &str, bytes: Vec<u8>) -> Value {
    client
        .upload("/api/v1/documents", &[Part::file("file", filename, "application/pdf", bytes)])
        .await
        .expect(StatusCode::CREATED)
}

// ---- storing and fetching --------------------------------------------------

#[tokio::test]
async fn a_small_file_is_stored_and_can_be_fetched_back() {
    let client = Client::start().await;

    let response = upload_one(&client, "receipt.pdf", PDF.to_vec()).await;
    assert_eq!(response["uploaded"], 1);

    let document = &response["documents"][0];
    assert_eq!(document["filename"], "receipt.pdf");
    assert_eq!(document["mime_type"], "application/pdf");
    assert_eq!(document["size"], PDF.len());
    assert_eq!(document["status"], "queued");
    assert_eq!(document["linked_transaction_id"], Value::Null);
    assert!(document.get("storage_path").is_none(), "the on-disk path is never sent");

    let id = document["id"].as_str().unwrap().to_string();

    // The metadata turns up in /state, without the path.
    let state = client.state().await;
    assert_eq!(state["documents"].as_array().unwrap().len(), 1);
    assert_eq!(state["documents"][0]["id"], id);
    assert_eq!(state["documents"][0]["filename"], "receipt.pdf");
    assert!(!state.to_string().contains("storage_path"));

    // And the bytes come back exactly as they went in.
    let downloaded = client.raw(&format!("/api/v1/documents/{id}/download")).await;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert_eq!(downloaded.bytes, PDF);
    assert_eq!(downloaded.header("content-type"), Some("application/pdf"));
    assert_eq!(downloaded.header("content-length"), Some(PDF.len().to_string().as_str()));
    let disposition = downloaded.header("content-disposition").unwrap();
    assert!(disposition.contains(r#"filename="receipt.pdf""#), "{disposition}");
}

#[tokio::test]
async fn several_files_go_up_in_one_request() {
    let client = Client::start().await;

    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("source", "phone-share"),
                Part::file("files", "jan.pdf", "application/pdf", PDF.to_vec()),
                Part::file("files", "feb.png", "image/png", b"\x89PNG\r\n\x1a\nfake".to_vec()),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["uploaded"], 2);
    let names: Vec<&str> =
        response["documents"].as_array().unwrap().iter().map(|d| d["filename"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["jan.pdf", "feb.png"]);
    assert_eq!(response["documents"][1]["mime_type"], "image/png");
    // The plain text field became the source on every file in the request.
    assert_eq!(response["documents"][0]["source"], "phone-share");
    assert_eq!(response["documents"][1]["source"], "phone-share");

    assert_eq!(client.state().await["documents"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn the_source_field_applies_wherever_it_appears_in_the_body() {
    let client = Client::start().await;

    // curl sends `-F file=@…` before `-F source=…`, so a `source` read as the
    // body streams past would arrive too late for every file before it.
    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::file("file", "jan.pdf", "application/pdf", PDF.to_vec()),
                Part::field("source", "phone-share"),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["source"], "phone-share");
}

#[tokio::test]
async fn a_missing_content_type_is_guessed_from_the_name() {
    let client = Client::start().await;

    let response = client
        .upload(
            "/api/v1/documents",
            &[Part { name: "file", filename: Some("scan.JPG"), content_type: None, bytes: b"jpegish".to_vec() }],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["mime_type"], "image/jpeg");
}

#[tokio::test]
async fn an_awkward_filename_is_stored_safely() {
    let client = Client::start().await;

    let response = client
        .upload(
            "/api/v1/documents",
            &[Part::file("file", "../../etc/passwd", "text/plain", b"not really".to_vec())],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["filename"], "passwd", "the path part is stripped");

    let id = response["documents"][0]["id"].as_str().unwrap();
    let downloaded = client.raw(&format!("/api/v1/documents/{id}/download")).await;
    assert_eq!(downloaded.status, StatusCode::OK);
    assert_eq!(downloaded.bytes, b"not really");
}

// ---- the size limit --------------------------------------------------------

#[tokio::test]
async fn a_file_over_twenty_mebibytes_is_refused_outright() {
    let client = Client::start().await;

    let oversized = filler(MAX_FILE_BYTES as usize + 1);
    let reply = client
        .upload("/api/v1/documents", &[Part::file("file", "huge.pdf", "application/pdf", oversized)])
        .await;

    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE, "body was {}", reply.body);
    assert_eq!(reply.body["error"], "payload_too_large");
    assert!(reply.body["message"].as_str().unwrap().contains("20 MiB"), "{}", reply.body["message"]);

    // Not truncated, not partly accepted: nothing was recorded at all.
    assert_eq!(
        client.state().await["documents"],
        json!([]),
        "an oversized upload must leave no metadata behind"
    );
}

#[tokio::test]
async fn a_file_of_exactly_the_limit_is_accepted() {
    let client = Client::start().await;

    // The boundary case in the other direction, so "over 20 MiB" means over.
    let exact = filler(MAX_FILE_BYTES as usize);
    let response = client
        .upload("/api/v1/documents", &[Part::file("file", "big.pdf", "application/pdf", exact)])
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["size"], MAX_FILE_BYTES);
}

#[tokio::test]
async fn one_oversized_file_takes_the_whole_request_with_it() {
    let client = Client::start().await;

    let reply = client
        .upload(
            "/api/v1/documents",
            &[
                Part::file("a", "fine.pdf", "application/pdf", PDF.to_vec()),
                Part::file("b", "huge.pdf", "application/pdf", filler(MAX_FILE_BYTES as usize + 1)),
            ],
        )
        .await;

    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        client.state().await["documents"],
        json!([]),
        "the file that was fine is rolled back too, so an upload is all or nothing"
    );
}

#[tokio::test]
async fn too_many_files_in_one_request_are_refused() {
    let client = Client::start().await;

    let parts: Vec<Part> = (0..=MAX_FILES)
        .map(|_| Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()))
        .collect();
    let reply = client.upload("/api/v1/documents", &parts).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body["message"].as_str().unwrap().contains("too many files"), "{}", reply.body);
    assert_eq!(client.state().await["documents"], json!([]));
}

#[tokio::test]
async fn an_upload_with_no_files_is_a_bad_request() {
    let client = Client::start().await;

    let reply = client.upload("/api/v1/documents", &[Part::field("source", "nowhere")]).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body["message"].as_str().unwrap().contains("no files"), "{}", reply.body);
}

// ---- removal and access ----------------------------------------------------

#[tokio::test]
async fn deleting_a_document_removes_the_bytes_too() {
    let client = Client::start().await;
    let response = upload_one(&client, "receipt.pdf", PDF.to_vec()).await;
    let id = response["documents"][0]["id"].as_str().unwrap().to_string();

    let files_before = std::fs::read_dir(client.data_dir()).unwrap().count();
    assert_eq!(files_before, 1);

    client.delete(&format!("/api/v1/documents/{id}")).await.expect(StatusCode::OK);

    assert_eq!(client.state().await["documents"], json!([]));
    assert_eq!(
        std::fs::read_dir(client.data_dir()).unwrap().count(),
        0,
        "the file on disk should go with the row"
    );

    client.raw(&format!("/api/v1/documents/{id}/download")).await;
    client.delete(&format!("/api/v1/documents/{id}")).await.expect(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fetching_a_document_that_does_not_exist_is_a_404() {
    let client = Client::start().await;
    let stranger = uuid::Uuid::new_v4();

    client.get(&format!("/api/v1/documents/{stranger}")).await.expect(StatusCode::NOT_FOUND);

    let download = client.raw(&format!("/api/v1/documents/{stranger}/download")).await;
    assert_eq!(download.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn documents_are_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;
    let stranger = uuid::Uuid::new_v4();

    for request in [
        common::multipart_request(
            "/api/v1/documents",
            &[Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec())],
        ),
        common::get(&format!("/api/v1/documents/{stranger}")),
        common::get(&format!("/api/v1/documents/{stranger}/download")),
    ] {
        let path = request.uri().path().to_string();
        let reply = common::call(&app, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path} was reachable unauthenticated");
    }
}

// ---- attaching a receipt to a transaction ----------------------------------

/// Records a transaction and returns its ID.
async fn a_transaction(client: &Client) -> String {
    let state = client.state().await;
    let account = find_named(&state["accounts"], "Main Account");

    let report = client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-10-04",
                "merchant": "Teapot Emporium",
                "amount_cents": 2345,
                "kind": "expense",
                "account_id": account,
            }),
        )
        .await
        .expect(StatusCode::CREATED);

    report["transactions"][0]["id"].as_str().expect("an id").to_string()
}

#[tokio::test]
async fn a_receipt_can_be_attached_to_a_transaction() {
    let client = Client::start().await;
    let id = a_transaction(&client).await;

    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", &id),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["linked_transaction_id"], id);
    // A receipt filed against a known transaction is stored, not queued for
    // somebody to sort out later.
    assert_eq!(response["documents"][0]["status"], "stored");
}

#[tokio::test]
async fn attaching_a_receipt_sets_the_transactions_receipt_flag() {
    let client = Client::start().await;
    let id = a_transaction(&client).await;

    let before = client.state().await;
    assert_eq!(before["transactions"][0]["receipt"], false);

    client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", &id),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    // The flag is what the UI's receipt indicator reads, so it has to mean
    // "a document is really on file" rather than "a client said so".
    let after = client.state().await;
    assert_eq!(after["transactions"][0]["receipt"], true);
}

#[tokio::test]
async fn the_link_applies_even_when_it_arrives_after_the_files() {
    let client = Client::start().await;
    let id = a_transaction(&client).await;

    // Same hazard as the `source` field: a client that writes its plain
    // fields last must still be understood.
    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
                Part::field("linked_transaction_id", &id),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["documents"][0]["linked_transaction_id"], id);
    assert_eq!(client.state().await["transactions"][0]["receipt"], true);
}

#[tokio::test]
async fn every_file_in_one_upload_is_linked() {
    let client = Client::start().await;
    let id = a_transaction(&client).await;

    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", &id),
                Part::file("a", "front.pdf", "application/pdf", PDF.to_vec()),
                Part::file("b", "back.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["uploaded"], 2);
    for document in response["documents"].as_array().unwrap() {
        assert_eq!(document["linked_transaction_id"], id);
    }
}

#[tokio::test]
async fn a_link_to_a_transaction_that_does_not_exist_is_refused() {
    let client = Client::start().await;

    let missing = uuid::Uuid::new_v4().to_string();
    client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", &missing),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::BAD_REQUEST);

    // Refused outright rather than stored unlinked: a receipt filed against
    // nothing is a file nobody will ever find again.
    assert!(client.state().await["documents"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_link_that_is_not_a_uuid_is_refused() {
    let client = Client::start().await;

    client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", "not-a-uuid"),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::BAD_REQUEST);

    assert!(client.state().await["documents"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_link_to_a_deleted_transaction_is_refused() {
    let client = Client::start().await;
    let id = a_transaction(&client).await;

    client.delete(&format!("/api/v1/transactions/{id}")).await.expect(StatusCode::OK);

    client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("linked_transaction_id", &id),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_unknown_plain_field_is_ignored_rather_than_refused() {
    let client = Client::start().await;

    // A form gaining a field should not break uploading.
    let response = client
        .upload(
            "/api/v1/documents",
            &[
                Part::field("something_new", "whatever"),
                Part::file("file", "receipt.pdf", "application/pdf", PDF.to_vec()),
            ],
        )
        .await
        .expect(StatusCode::CREATED);

    assert_eq!(response["uploaded"], 1);
}
