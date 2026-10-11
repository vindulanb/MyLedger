//! End-to-end tests of CSV statement import.
//!
//! The parser itself is tested in `core`; these tests are about what only the
//! server can do — resolving names to IDs, falling back to `Needs review`,
//! running every row through the same fingerprint dedupe as a manual entry,
//! and refusing to guess when the file is ambiguous.

mod common;

use axum::http::StatusCode;
use common::{Client, Part};
use serde_json::{Value, json};

/// Posts a statement, with no options.
async fn import(client: &Client, csv: &str) -> Value {
    client
        .upload(
            "/api/v1/import/csv",
            &[Part::file("file", "statement.csv", "text/csv", csv.as_bytes().to_vec())],
        )
        .await
        .expect(StatusCode::CREATED)
}

/// Posts a statement with an `options` field.
async fn import_with(client: &Client, csv: &str, options: Value) -> common::Reply {
    client
        .upload(
            "/api/v1/import/csv",
            &[
                Part::field("options", &options.to_string()),
                Part::file("file", "statement.csv", "text/csv", csv.as_bytes().to_vec()),
            ],
        )
        .await
}

const TIDY: &str = "Date,Description,Amount,Category\n\
                    2026-10-01,Corner Grocer,-62.14,Groceries\n\
                    2026-10-03,Tram Pass,-20.00,Transportation\n\
                    2026-10-25,Acme Payroll,2000.00,Income\n";

// ---- the happy path --------------------------------------------------------

#[tokio::test]
async fn a_tidy_statement_imports_every_row() {
    let client = Client::start().await;

    let report = import(&client, TIDY).await;
    assert_eq!(report["inserted"], 3);
    assert_eq!(report["duplicates"], 0);
    assert_eq!(report["skipped"], 0);
    assert_eq!(report["rows_in_file"], 3);
    assert_eq!(report["headers"][0], "Date");

    let state = client.state().await;
    let rows = state["transactions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
}

#[tokio::test]
async fn amounts_are_stored_positive_with_the_direction_in_the_kind() {
    let client = Client::start().await;
    import(&client, TIDY).await;

    let state = client.state().await;
    let rows = state["transactions"].as_array().unwrap();

    let grocer = rows.iter().find(|t| t["merchant"] == "Corner Grocer").unwrap();
    assert_eq!(grocer["amount_cents"], 6214, "stored positive, not -6214");
    assert_eq!(grocer["kind"], "expense");

    let payroll = rows.iter().find(|t| t["merchant"] == "Acme Payroll").unwrap();
    assert_eq!(payroll["amount_cents"], 200_000);
    assert_eq!(payroll["kind"], "income");
}

#[tokio::test]
async fn imported_rows_are_marked_as_coming_from_a_csv_and_carry_no_receipt() {
    let client = Client::start().await;
    import(&client, TIDY).await;

    for row in client.state().await["transactions"].as_array().unwrap() {
        assert_eq!(row["source"], "csv");
        assert_eq!(row["receipt"], false);
    }
}

#[tokio::test]
async fn dates_are_normalised_to_iso() {
    let client = Client::start().await;

    let report = import_with(
        &client,
        "Date,Description,Amount\n01/10/2026,Shop,-1.00\n25/10/2026,Other,-2.00\n",
        json!({}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 2);

    let state = client.state().await;
    let dates: Vec<&str> = state["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap())
        .collect();
    // 25/10 settles the column as day-first, so 01/10 is the 1st of October.
    assert!(dates.contains(&"2026-10-01"), "{dates:?}");
    assert!(dates.contains(&"2026-10-25"), "{dates:?}");
}

// ---- names to IDs ----------------------------------------------------------

#[tokio::test]
async fn a_category_the_statement_names_is_matched_to_the_users_own() {
    let client = Client::start().await;
    import(&client, TIDY).await;

    let state = client.state().await;
    let groceries = common::find_named(&state["categories"], "Groceries");
    let grocer = state["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["merchant"] == "Corner Grocer")
        .unwrap();
    assert_eq!(grocer["category_id"], groceries);
}

#[tokio::test]
async fn a_category_name_is_matched_regardless_of_case() {
    let client = Client::start().await;

    import(&client, "Date,Description,Amount,Category\n2026-10-01,Shop,-1.00,GROCERIES\n").await;

    let state = client.state().await;
    let groceries = common::find_named(&state["categories"], "Groceries");
    assert_eq!(state["transactions"][0]["category_id"], groceries);
}

#[tokio::test]
async fn a_category_the_user_does_not_have_falls_back_to_needs_review() {
    let client = Client::start().await;

    let report =
        import(&client, "Date,Description,Amount,Category\n2026-10-01,Shop,-1.00,Blorp\n").await;
    assert_eq!(report["needs_review"], 1);

    let state = client.state().await;
    let review = common::find_named(&state["categories"], "Needs review");
    assert_eq!(state["transactions"][0]["category_id"], review);
}

#[tokio::test]
async fn a_statement_with_no_category_column_falls_back_to_needs_review() {
    let client = Client::start().await;

    let report = import(&client, "Date,Description,Amount\n2026-10-01,Shop,-1.00\n").await;
    assert_eq!(report["needs_review"], 1);

    let state = client.state().await;
    let review = common::find_named(&state["categories"], "Needs review");
    assert_eq!(state["transactions"][0]["category_id"], review);
}

#[tokio::test]
async fn an_account_the_statement_names_is_matched_by_name() {
    let client = Client::start().await;

    import(
        &client,
        "Date,Description,Amount,Account\n2026-10-01,Shop,-1.00,Credit Card\n",
    )
    .await;

    let state = client.state().await;
    let card = common::find_named(&state["accounts"], "Credit Card");
    assert_eq!(state["transactions"][0]["account_id"], card);
}

#[tokio::test]
async fn an_unknown_account_name_is_left_unset_rather_than_created() {
    let client = Client::start().await;

    import(&client, "Date,Description,Amount,Account\n2026-10-01,Shop,-1.00,Barclays 4411\n")
        .await;

    let state = client.state().await;
    assert_eq!(state["transactions"][0]["account_id"], Value::Null);
    // The import must not invent accounts the user never asked for.
    assert_eq!(state["accounts"].as_array().unwrap().len(), 3);
}

// ---- rules still apply -----------------------------------------------------

#[tokio::test]
async fn a_rule_gets_first_refusal_before_the_needs_review_fallback() {
    let client = Client::start().await;

    let state = client.state().await;
    let dining = common::find_named(&state["categories"], "Dining");
    client
        .put(
            "/api/v1/preferences",
            json!({"rules": [{"when_text": "pasta", "then_category_id": dining}]}),
        )
        .await
        .expect(StatusCode::OK);

    let report = import(&client, "Date,Description,Amount\n2026-10-01,PASTA PLACE,-9.00\n").await;
    assert_eq!(report["inserted"], 1);
    // The rule matched, so this row is filed properly rather than parked.
    assert_eq!(report["needs_review"], 0);

    let state = client.state().await;
    assert_eq!(state["transactions"][0]["category_id"], dining);
}

// ---- dedupe ----------------------------------------------------------------

#[tokio::test]
async fn re_importing_the_same_file_inserts_nothing() {
    let client = Client::start().await;

    let first = import(&client, TIDY).await;
    assert_eq!(first["inserted"], 3);

    // The spec's own check: every row reported as a duplicate, none added.
    let again = import_with(&client, TIDY, json!({})).await.expect(StatusCode::OK);
    assert_eq!(again["inserted"], 0);
    assert_eq!(again["duplicates"], 3);
    assert_eq!(again["skipped"], 0);

    assert_eq!(client.state().await["transactions"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_row_already_entered_by_hand_is_a_duplicate_of_the_imported_one() {
    let client = Client::start().await;

    let state = client.state().await;
    let account = common::find_named(&state["accounts"], "Main Account");
    client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-10-01",
                "merchant": "Corner Grocer",
                "amount_cents": 6214,
                "kind": "expense",
                "account_id": account,
            }),
        )
        .await
        .expect(StatusCode::CREATED);

    // Same date, merchant, amount and account: one fingerprint, whichever
    // path created it. The import has no dedupe of its own to disagree with.
    let report = import_with(
        &client,
        "Date,Description,Amount,Account\n2026-10-01,Corner Grocer,-62.14,Main Account\n",
        json!({}),
    )
    .await
    .expect(StatusCode::OK);
    assert_eq!(report["duplicates"], 1);
    assert_eq!(report["inserted"], 0);
}

#[tokio::test]
async fn the_same_row_twice_in_one_file_is_imported_once() {
    let client = Client::start().await;

    let report = import_with(
        &client,
        "Date,Description,Amount\n2026-10-01,Shop,-1.00\n2026-10-01,Shop,-1.00\n",
        json!({}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 1);
    assert_eq!(report["duplicates"], 1);
}

// ---- unreadable rows -------------------------------------------------------

#[tokio::test]
async fn unparseable_lines_are_reported_and_never_turned_into_placeholder_rows() {
    let client = Client::start().await;

    let report = import_with(
        &client,
        "Date,Description,Amount\n\
         2026-10-01,Good Row,-1.00\n\
         not-a-date,Bad Date,-2.00\n\
         2026-10-03,Bad Amount,pending\n\
         2026-10-04,Zero,0.00\n",
        json!({}),
    )
    .await
    .expect(StatusCode::CREATED);

    assert_eq!(report["inserted"], 1);
    assert_eq!(report["skipped"], 3);
    assert_eq!(report["rows_in_file"], 4);

    // Only the good row exists. Nothing was approximated.
    let rows = client.state().await["transactions"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["merchant"], "Good Row");

    let detail = report["skipped_detail"].as_array().unwrap();
    assert_eq!(detail.len(), 3);
    let lines: Vec<u64> = detail.iter().map(|d| d["line"].as_u64().unwrap()).collect();
    // Line 1 is the header row.
    assert_eq!(lines, vec![3, 4, 5]);
    assert!(detail[0]["reason"].as_str().unwrap().contains("date"));
}

// ---- ambiguity is refused, not guessed -------------------------------------

#[tokio::test]
async fn an_all_positive_file_is_refused_until_the_direction_is_given() {
    let client = Client::start().await;

    // Nothing in this file says which way the money went. Guessing would turn
    // the salary into a cost and report a clean success.
    let refusal = import_with(
        &client,
        "Date,Description,Amount\n2026-10-01,Shop,12.50\n2026-10-25,Payroll,2000.00\n",
        json!({}),
    )
    .await
    .expect(StatusCode::BAD_REQUEST);
    assert!(
        refusal["message"].as_str().unwrap().contains("which way"),
        "{}",
        refusal["message"]
    );
    assert!(client.state().await["transactions"].as_array().unwrap().is_empty());

    let report = import_with(
        &client,
        "Date,Description,Amount\n2026-10-01,Shop,12.50\n2026-10-25,Payroll,2000.00\n",
        json!({"direction": "expense"}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 2);
    for row in client.state().await["transactions"].as_array().unwrap() {
        assert_eq!(row["kind"], "expense");
    }
}

#[tokio::test]
async fn an_ambiguous_date_order_is_refused_until_it_is_settled() {
    let client = Client::start().await;

    let csv = "Date,Description,Amount\n03/04/2026,Shop,-1.00\n05/06/2026,Other,-2.00\n";
    let refusal = import_with(&client, csv, json!({})).await.expect(StatusCode::BAD_REQUEST);
    assert!(
        refusal["message"].as_str().unwrap().contains("day-first"),
        "{}",
        refusal["message"]
    );

    let report = import_with(&client, csv, json!({"date_format": "month-first"}))
        .await
        .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 2);

    let state = client.state().await;
    let dates: Vec<&str> = state["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap())
        .collect();
    assert!(dates.contains(&"2026-03-04"), "{dates:?}");
}

#[tokio::test]
async fn a_statement_with_debit_and_credit_columns_needs_no_help() {
    let client = Client::start().await;

    let report = import_with(
        &client,
        "Transaction Date,Narrative,Debit,Credit\n\
         2026-10-01,Corner Grocer,62.14,\n\
         2026-10-25,Acme Payroll,,2000.00\n",
        json!({}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 2);

    let state = client.state().await;
    let rows = state["transactions"].as_array().unwrap();
    let grocer = rows.iter().find(|t| t["merchant"] == "Corner Grocer").unwrap();
    assert_eq!(grocer["kind"], "expense");
    let payroll = rows.iter().find(|t| t["merchant"] == "Acme Payroll").unwrap();
    assert_eq!(payroll["kind"], "income");
}

#[tokio::test]
async fn a_positive_debit_statement_is_read_the_way_the_file_says() {
    let client = Client::start().await;

    // Every amount positive, with a type column carrying the direction. The
    // sign convention is the file's, not an assumption.
    let report = import_with(
        &client,
        "Date,Description,Amount,Transaction Type\n\
         2026-10-01,Corner Grocer,62.14,DEBIT\n\
         2026-10-25,Acme Payroll,2000.00,CREDIT\n",
        json!({}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 2);

    let state = client.state().await;
    let rows = state["transactions"].as_array().unwrap();
    assert_eq!(rows.iter().find(|t| t["merchant"] == "Corner Grocer").unwrap()["kind"], "expense");
    assert_eq!(rows.iter().find(|t| t["merchant"] == "Acme Payroll").unwrap()["kind"], "income");
}

#[tokio::test]
async fn an_explicit_column_mapping_is_honoured() {
    let client = Client::start().await;

    // Headers nothing would recognise. The mapping is how the user says what
    // is what, instead of the import guessing.
    let report = import_with(
        &client,
        "Col A,Col B,Col C\n2026-10-01,Corner Grocer,-62.14\n",
        json!({"columns": {"date": 0, "merchant": 1, "amount": 2}, "date_format": "iso"}),
    )
    .await
    .expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 1);

    let state = client.state().await;
    assert_eq!(state["transactions"][0]["merchant"], "Corner Grocer");
    assert_eq!(state["transactions"][0]["amount_cents"], 6214);
}

#[tokio::test]
async fn a_file_with_no_recognisable_columns_is_refused_with_questions() {
    let client = Client::start().await;

    let refusal = import_with(&client, "Col A,Col B\nx,y\n", json!({}))
        .await
        .expect(StatusCode::BAD_REQUEST);
    assert!(refusal["message"].as_str().unwrap().contains("column"), "{}", refusal["message"]);
}

// ---- malformed requests ----------------------------------------------------

#[tokio::test]
async fn a_request_with_no_file_is_a_bad_request() {
    let client = Client::start().await;
    client
        .upload("/api/v1/import/csv", &[Part::field("options", "{}")])
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_empty_file_is_a_bad_request() {
    let client = Client::start().await;
    client
        .upload(
            "/api/v1/import/csv",
            &[Part::file("file", "empty.csv", "text/csv", Vec::new())],
        )
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_header_row_with_no_data_is_a_bad_request() {
    let client = Client::start().await;
    import_with(&client, "Date,Description,Amount\n", json!({}))
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unreadable_options_are_refused_rather_than_ignored() {
    let client = Client::start().await;
    import_with(&client, TIDY, json!({"date_formatt": "iso"}))
        .await
        .expect(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_import_is_behind_the_same_gate_as_everything_else() {
    let (app, _db, _dir) = common::app_with_user().await;

    let request = common::multipart_request(
        "/api/v1/import/csv",
        &[Part::file("file", "s.csv", "text/csv", TIDY.as_bytes().to_vec())],
    );
    let reply = common::call(&app, request).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_awkward_but_real_statement_imports_cleanly() {
    let client = Client::start().await;

    // Quoted descriptions with commas, a BOM, CRLF endings, currency
    // symbols, a blank trailing line, and a day-first column settled by one
    // row with a day above twelve.
    let csv = "\u{feff}Transaction Date,Description,Amount,Category\r\n\
               01/10/2026,\"GROCER, CORNER #4411\",-\u{a3}62.14,Groceries\r\n\
               03/10/2026,TRAM PASS,-\u{a3}20.00,Transportation\r\n\
               25/10/2026,ACME PAYROLL,\u{a3}2000.00,Income\r\n\
               \r\n";

    let report = import_with(&client, csv, json!({})).await.expect(StatusCode::CREATED);
    assert_eq!(report["inserted"], 3);
    assert_eq!(report["skipped"], 0);

    let state = client.state().await;
    let rows = state["transactions"].as_array().unwrap();
    let grocer = rows.iter().find(|t| t["merchant"] == "GROCER, CORNER #4411").unwrap();
    assert_eq!(grocer["date"], "2026-10-01");
    assert_eq!(grocer["amount_cents"], 6214);
    assert_eq!(grocer["kind"], "expense");
}
