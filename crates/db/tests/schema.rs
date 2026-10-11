//! Integration tests against a real SQLite file.
//!
//! A temporary file rather than an in-memory database, so that what is being
//! tested is the migration as it will actually run on disk — journal mode,
//! foreign keys, unique indexes and all.

use myledger_core::{
    Budget, DocumentStatus, Goal, Period, Transaction, TransactionKind, TransactionSource,
    now_timestamp, transaction_fingerprint,
};
use myledger_db::lookups::{accounts, categories, tags};
use myledger_db::seed::{STARTER_ACCOUNTS, STARTER_CATEGORIES, seed_starter_data};
use myledger_db::{Db, DbError, budgets, documents, goals, settings, sms_rules, transactions};
use tempfile::TempDir;
use uuid::Uuid;

/// A database in a fresh temporary directory. The directory is returned too:
/// dropping it deletes the file, so it has to outlive the test.
async fn temp_db() -> (Db, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = Db::connect(dir.path().join("myledger.sqlite"))
        .await
        .expect("connect and migrate");
    (db, dir)
}

fn expense(date: &str, merchant: &str, amount_cents: i64, account_name: &str) -> Transaction {
    let now = now_timestamp();
    Transaction {
        id: Uuid::new_v4(),
        date: date.to_string(),
        merchant: merchant.to_string(),
        category_id: None,
        amount_cents,
        kind: TransactionKind::Expense,
        account_id: None,
        tags: Vec::new(),
        receipt: false,
        source: TransactionSource::Manual,
        fingerprint: transaction_fingerprint(date, merchant, amount_cents, account_name),
        sms_hash: None,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
    }
}

#[tokio::test]
async fn migrations_run_against_a_fresh_file_and_again_against_the_same_one() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("myledger.sqlite");

    let db = Db::connect(&path).await.expect("first connect");
    seed_starter_data(&db).await.expect("first seed");
    db.close().await;

    // Reopening must not re-apply migrations or re-seed.
    let db = Db::connect(&path).await.expect("second connect");
    let outcome = seed_starter_data(&db).await.expect("second seed");
    assert!(outcome.is_empty(), "re-seeding a populated database changed something: {outcome:?}");
    assert_eq!(categories::count_categories(&db).await.unwrap(), STARTER_CATEGORIES.len() as i64);
}

#[tokio::test]
async fn a_second_transaction_with_the_same_fingerprint_is_refused() {
    let (db, _dir) = temp_db().await;

    let first = expense("2026-03-04", "Starbucks", 545, "Main Account");
    transactions::insert_transaction(&db, &first).await.expect("first insert");

    // Same purchase, arriving again from a re-imported statement: a different
    // row ID, but the fingerprint is what identifies it.
    let duplicate = expense("2026-03-04", "Starbucks", 545, "Main Account");
    assert_ne!(duplicate.id, first.id);
    assert_eq!(duplicate.fingerprint, first.fingerprint);

    let err = transactions::insert_transaction(&db, &duplicate)
        .await
        .expect_err("the unique constraint should have rejected this");
    assert!(
        matches!(err, DbError::DuplicateFingerprint),
        "expected a duplicate-fingerprint error, got {err:?}"
    );

    assert_eq!(transactions::count_transactions(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn the_fingerprint_constraint_is_enforced_by_the_database_itself() {
    let (db, _dir) = temp_db().await;

    let transaction = expense("2026-03-04", "Starbucks", 545, "Main Account");
    transactions::insert_transaction(&db, &transaction).await.unwrap();

    // Bypassing the query layer entirely: the guarantee has to hold even for
    // SQL that never goes near application code.
    let raw = sqlx::query(
        "INSERT INTO transactions (id, date, merchant, amount_cents, kind, receipt, source, \
         fingerprint, created_at, updated_at) VALUES (?, ?, ?, ?, 'expense', 0, 'manual', ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind("2026-03-04")
    .bind("Starbucks")
    .bind(545_i64)
    .bind(&transaction.fingerprint)
    .bind(now_timestamp())
    .bind(now_timestamp())
    .execute(db.pool())
    .await;

    let err = raw.expect_err("raw insert should violate the unique index");
    assert!(
        err.as_database_error().is_some_and(|e| e.is_unique_violation()),
        "expected a unique violation, got {err:?}"
    );
}

#[tokio::test]
async fn a_transaction_round_trips_with_its_tags() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    let groceries = categories::find_category_by_name(&db, "Groceries").await.unwrap().unwrap();
    let account = accounts::find_account_by_name(&db, "Main Account").await.unwrap().unwrap();
    let essential = tags::insert_tag(&db, "essential").await.unwrap();
    let weekly = tags::insert_tag(&db, "weekly").await.unwrap();

    let mut transaction = expense("2026-03-04", "Whole Foods", 8532, "Main Account");
    transaction.category_id = Some(groceries.id);
    transaction.account_id = Some(account.id);
    transaction.tags = vec![essential.id, weekly.id];
    transaction.receipt = true;
    transactions::insert_transaction(&db, &transaction).await.unwrap();

    let stored = transactions::get_transaction(&db, transaction.id).await.unwrap().unwrap();
    assert_eq!(stored.merchant, "Whole Foods");
    assert_eq!(stored.amount_cents, 8532);
    assert_eq!(stored.kind, TransactionKind::Expense);
    assert_eq!(stored.category_id, Some(groceries.id));
    assert_eq!(stored.account_id, Some(account.id));
    assert!(stored.receipt);

    let mut stored_tags = stored.tags.clone();
    stored_tags.sort();
    let mut expected_tags = vec![essential.id, weekly.id];
    expected_tags.sort();
    assert_eq!(stored_tags, expected_tags);
}

#[tokio::test]
async fn changing_the_category_leaves_the_tags_alone() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    let dining = categories::find_category_by_name(&db, "Dining").await.unwrap().unwrap();
    let treat = tags::insert_tag(&db, "treat").await.unwrap();

    let mut transaction = expense("2026-03-04", "Starbucks", 545, "Main Account");
    transaction.tags = vec![treat.id];
    transactions::insert_transaction(&db, &transaction).await.unwrap();

    let mut edited = transactions::get_transaction(&db, transaction.id).await.unwrap().unwrap();
    edited.category_id = Some(dining.id);
    assert!(transactions::update_transaction(&db, &edited).await.unwrap());

    let stored = transactions::get_transaction(&db, transaction.id).await.unwrap().unwrap();
    assert_eq!(stored.category_id, Some(dining.id));
    assert_eq!(stored.tags, vec![treat.id]);
}

#[tokio::test]
async fn a_soft_deleted_transaction_leaves_the_listing_but_not_the_table() {
    let (db, _dir) = temp_db().await;

    let transaction = expense("2026-03-04", "Starbucks", 545, "Main Account");
    transactions::insert_transaction(&db, &transaction).await.unwrap();

    assert!(transactions::soft_delete_transaction(&db, transaction.id).await.unwrap());
    assert!(
        !transactions::soft_delete_transaction(&db, transaction.id).await.unwrap(),
        "deleting twice should report no change"
    );

    assert_eq!(transactions::count_transactions(&db).await.unwrap(), 0);
    assert!(transactions::list_transactions(&db, 100).await.unwrap().is_empty());

    let stored = transactions::get_transaction(&db, transaction.id).await.unwrap().unwrap();
    assert!(stored.deleted_at.is_some(), "the row itself should still be there");

    // And the fingerprint is still taken, so a re-import cannot resurrect it
    // as a second row.
    let again = expense("2026-03-04", "Starbucks", 545, "Main Account");
    assert!(matches!(
        transactions::insert_transaction(&db, &again).await,
        Err(DbError::DuplicateFingerprint)
    ));
}

#[tokio::test]
async fn seeding_creates_names_and_nothing_else() {
    let (db, _dir) = temp_db().await;

    let outcome = seed_starter_data(&db).await.unwrap();
    assert_eq!(outcome.categories_inserted, STARTER_CATEGORIES.len());
    assert_eq!(outcome.accounts_inserted, STARTER_ACCOUNTS.len());
    assert_eq!(outcome.settings_inserted, 4);

    let seeded_categories: Vec<String> =
        categories::list_categories(&db).await.unwrap().into_iter().map(|c| c.name).collect();
    for name in STARTER_CATEGORIES {
        assert!(seeded_categories.iter().any(|c| c == name), "missing category {name}");
    }
    let seeded_accounts: Vec<String> =
        accounts::list_accounts(&db).await.unwrap().into_iter().map(|a| a.name).collect();
    for name in STARTER_ACCOUNTS {
        assert!(seeded_accounts.iter().any(|a| a == name), "missing account {name}");
    }

    // Nothing financial came with them.
    assert_eq!(transactions::count_transactions(&db).await.unwrap(), 0);
    assert!(budgets::list_budgets(&db).await.unwrap().is_empty());
    assert!(goals::list_goals(&db).await.unwrap().is_empty());
    assert!(tags::list_tags(&db).await.unwrap().is_empty());
    assert!(
        sms_rules::list_sms_rules(&db).await.unwrap().is_empty(),
        "no SMS parsing rules are pre-seeded"
    );

    let loaded = settings::load_settings(&db).await.unwrap();
    assert_eq!(loaded.selected_period, Period::AllTime);
    assert!(!loaded.net_worth_configured);
    assert_eq!(loaded.assets_cents, 0);
    assert_eq!(loaded.liabilities_cents, 0);
    assert_eq!(loaded.wipe_reset_at, None);
}

#[tokio::test]
async fn a_deleted_starter_name_stays_deleted_across_a_reseed() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    let dining = categories::find_category_by_name(&db, "Dining").await.unwrap().unwrap();
    assert!(categories::soft_delete_category(&db, dining.id).await.unwrap());

    seed_starter_data(&db).await.unwrap();

    assert!(
        categories::find_category_by_name(&db, "Dining").await.unwrap().is_none(),
        "the seed must not undo the user's choices"
    );
}

#[tokio::test]
async fn deleting_every_category_gets_the_starter_names_back() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    for category in categories::list_categories(&db).await.unwrap() {
        categories::soft_delete_category(&db, category.id).await.unwrap();
    }
    assert!(categories::list_categories(&db).await.unwrap().is_empty());

    // Leaving none at all is not a preference — there would be nowhere to file
    // a transaction — so this one state does re-seed.
    let outcome = seed_starter_data(&db).await.unwrap();

    assert_eq!(outcome.categories_inserted, STARTER_CATEGORIES.len());
    assert_eq!(outcome.accounts_inserted, 0, "the accounts were left alone");
    assert_eq!(categories::list_categories(&db).await.unwrap().len(), STARTER_CATEGORIES.len());
}

#[tokio::test]
async fn writing_one_setting_leaves_the_others_alone() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    settings::set_setting(&db, settings::keys::ASSETS_CENTS, "1234500").await.unwrap();
    settings::set_setting(&db, settings::keys::SELECTED_PERIOD, "last-3-months").await.unwrap();

    let loaded = settings::load_settings(&db).await.unwrap();
    assert_eq!(loaded.assets_cents, 1_234_500);
    assert_eq!(loaded.selected_period, Period::LastThreeMonths);
    assert_eq!(loaded.liabilities_cents, 0, "untouched settings keep their value");
    assert!(!loaded.net_worth_configured);
}

#[tokio::test]
async fn budgets_goals_and_documents_round_trip() {
    let (db, _dir) = temp_db().await;
    seed_starter_data(&db).await.unwrap();

    let housing = categories::find_category_by_name(&db, "Housing").await.unwrap().unwrap();
    let now = now_timestamp();

    let budget = Budget {
        id: Uuid::new_v4(),
        category_id: Some(housing.id),
        monthly_limit_cents: 150_000,
        active: true,
        created_at: now.clone(),
        updated_at: now.clone(),
        deleted_at: None,
    };
    budgets::insert_budget(&db, &budget).await.unwrap();
    assert_eq!(budgets::list_budgets(&db).await.unwrap(), vec![budget.clone()]);

    let goal = Goal {
        id: Uuid::new_v4(),
        name: "Emergency fund".to_string(),
        target_cents: 500_000,
        current_cents: 0,
        due_date: Some("2027-01-01".to_string()),
        note: None,
        created_at: now.clone(),
        updated_at: now.clone(),
        deleted_at: None,
    };
    goals::insert_goal(&db, &goal).await.unwrap();
    assert_eq!(goals::list_goals(&db).await.unwrap(), vec![goal.clone()]);

    let document = myledger_core::Document {
        id: Uuid::new_v4(),
        filename: "receipt.pdf".to_string(),
        mime_type: Some("application/pdf".to_string()),
        size: Some(2048),
        storage_path: "documents/abc.pdf".to_string(),
        status: DocumentStatus::Queued,
        source: Some("upload".to_string()),
        linked_transaction_id: None,
        created_at: now,
    };
    documents::insert_document(&db, &document).await.unwrap();
    assert_eq!(documents::list_recent_documents(&db, 100).await.unwrap(), vec![document.clone()]);

    let removed = documents::delete_document(&db, document.id).await.unwrap().unwrap();
    assert_eq!(removed.storage_path, "documents/abc.pdf", "the caller needs the path to unlink");
    assert!(documents::get_document(&db, document.id).await.unwrap().is_none());

    assert!(budgets::soft_delete_budget(&db, budget.id).await.unwrap());
    assert!(budgets::list_budgets(&db).await.unwrap().is_empty());
    assert!(goals::soft_delete_goal(&db, goal.id).await.unwrap());
    assert!(goals::list_goals(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_check_constraint_rejects_a_token_no_enum_knows() {
    let (db, _dir) = temp_db().await;

    let err = sqlx::query(
        "INSERT INTO transactions (id, date, merchant, amount_cents, kind, receipt, source, \
         fingerprint, created_at, updated_at) VALUES (?, ?, ?, ?, 'transfer', 0, 'manual', ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind("2026-03-04")
    .bind("Starbucks")
    .bind(545_i64)
    .bind("2026-03-04|starbucks|545|main account")
    .bind(now_timestamp())
    .bind(now_timestamp())
    .execute(db.pool())
    .await
    .expect_err("'transfer' is not a transaction kind");

    assert!(
        err.to_string().to_lowercase().contains("check"),
        "expected a CHECK constraint failure, got {err}"
    );
}
