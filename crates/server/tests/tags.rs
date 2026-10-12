//! Tag usage counts, and the choice of what a deleted tag does to history.
//!
//! The question this file is about: deleting a tag stops it being offered,
//! but what happens to the transactions already filed against it? Leaving
//! them is the default, because a historical entry that keeps its label keeps
//! its meaning. Stripping them is available, has to be asked for, and cannot
//! be undone.

mod common;

use axum::http::StatusCode;
use common::{Client, find_named};
use serde_json::{Value, json};

/// Records a transaction carrying the given tags.
async fn tagged(client: &Client, merchant: &str, cents: i64, tags: &[String]) -> String {
    let report = client
        .post(
            "/api/v1/transactions",
            json!({
                "date": "2026-10-04",
                "merchant": merchant,
                "amount_cents": cents,
                "kind": "expense",
                "tags": tags,
            }),
        )
        .await
        .expect(StatusCode::CREATED);
    report["transactions"][0]["id"].as_str().expect("an id").to_string()
}

/// Creates tags by name and returns their IDs, in order.
async fn make_tags(client: &Client, names: &[&str]) -> Vec<String> {
    client
        .put(
            "/api/v1/preferences",
            json!({"tags": names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>()}),
        )
        .await
        .expect(StatusCode::OK);

    let state = client.state().await;
    names.iter().map(|name| find_named(&state["tags"], name)).collect()
}

/// The usage count the state reports for one tag.
fn usage_of(state: &Value, tag_id: &str) -> i64 {
    state["tag_usage"]
        .as_array()
        .expect("tag_usage")
        .iter()
        .find(|row| row["tag_id"] == tag_id)
        .map_or(0, |row| row["transactions"].as_i64().unwrap_or(0))
}

// ---- usage counts ----------------------------------------------------------

#[tokio::test]
async fn a_fresh_ledger_reports_no_tag_usage() {
    let client = Client::start().await;
    let state = client.state().await;
    assert!(state["tag_usage"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_tag_with_no_transactions_counts_zero() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["unused"]).await;

    let state = client.state().await;
    // Absent from the list rather than present as a zero, which the helper
    // on `StateResponse` reads as zero either way.
    assert_eq!(usage_of(&state, &tags[0]), 0);
}

#[tokio::test]
async fn usage_counts_the_transactions_carrying_each_tag() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work", "holiday"]).await;

    tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;
    tagged(&client, "Taxi", 3_400, &[tags[0].clone(), tags[1].clone()]).await;
    tagged(&client, "Hotel", 9_900, &[tags[1].clone()]).await;

    let state = client.state().await;
    assert_eq!(usage_of(&state, &tags[0]), 2);
    assert_eq!(usage_of(&state, &tags[1]), 2);
}

#[tokio::test]
async fn a_deleted_transaction_stops_counting_towards_its_tags() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;

    let first = tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;
    tagged(&client, "Taxi", 3_400, &[tags[0].clone()]).await;
    assert_eq!(usage_of(&client.state().await, &tags[0]), 2);

    client.delete(&format!("/api/v1/transactions/{first}")).await.expect(StatusCode::OK);

    // A tag is not "in use" by something the user deleted.
    assert_eq!(usage_of(&client.state().await, &tags[0]), 1);
}

#[tokio::test]
async fn the_count_follows_an_inline_tag_edit() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    let id = tagged(&client, "Lunch", 1_200, &[]).await;
    assert_eq!(usage_of(&client.state().await, &tags[0]), 0);

    client
        .patch(&format!("/api/v1/transactions/{id}"), json!({"tags": [tags[0]]}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(usage_of(&client.state().await, &tags[0]), 1);

    client
        .patch(&format!("/api/v1/transactions/{id}"), json!({"tags": []}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(usage_of(&client.state().await, &tags[0]), 0);
}

#[tokio::test]
async fn the_preferences_reply_carries_fresh_counts_too() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;

    // The screen that just edited the tags is about to redraw the counts, so
    // the reply saves it a second read.
    let reply = client
        .put("/api/v1/preferences", json!({"tags": [{"id": tags[0], "name": "work"}]}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(usage_of(&reply, &tags[0]), 1);
}

// ---- deleting a tag --------------------------------------------------------

#[tokio::test]
async fn deleting_a_tag_leaves_it_on_the_history_by_default() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    let id = tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;

    // Sending an empty tag list removes the tag, and says nothing about
    // history — so history is left alone.
    client.put("/api/v1/preferences", json!({"tags": []})).await.expect(StatusCode::OK);

    let state = client.state().await;
    let tag = state["tags"].as_array().unwrap().iter().find(|t| t["id"] == tags[0]).unwrap();
    assert!(!tag["deleted_at"].is_null(), "the tag is deleted");

    // The transaction still carries it, which is why the entry still makes
    // sense when you read it back.
    let transaction =
        state["transactions"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap();
    assert_eq!(transaction["tags"][0], tags[0]);
}

#[tokio::test]
async fn asking_to_strip_a_removed_tag_takes_it_off_the_history() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    let id = tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;
    tagged(&client, "Taxi", 3_400, &[tags[0].clone()]).await;

    let reply = client
        .put("/api/v1/preferences", json!({"tags": [], "strip_removed_tags": true}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(reply["tag_links_removed"], 2, "both links reported");

    let state = client.state().await;
    let transaction =
        state["transactions"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap();
    assert!(transaction["tags"].as_array().unwrap().is_empty(), "the tag is off the entry");
    assert_eq!(usage_of(&state, &tags[0]), 0);
}

#[tokio::test]
async fn stripping_only_touches_the_tags_this_request_removed() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work", "holiday"]).await;
    let id = tagged(&client, "Taxi", 3_400, &[tags[0].clone(), tags[1].clone()]).await;

    // Remove only the first, and strip it.
    let reply = client
        .put(
            "/api/v1/preferences",
            json!({
                "tags": [{"id": tags[1], "name": "holiday"}],
                "strip_removed_tags": true,
            }),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(reply["tag_links_removed"], 1);

    let state = client.state().await;
    let transaction =
        state["transactions"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap();
    // The surviving tag is untouched.
    assert_eq!(transaction["tags"].as_array().unwrap(), &vec![json!(tags[1])]);
    assert_eq!(usage_of(&state, &tags[1]), 1);
}

#[tokio::test]
async fn stripping_with_nothing_removed_changes_nothing() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;

    // The flag is set, but this request removes no tag, so it has nothing to
    // act on — and must not reach for a tag deleted at some earlier point.
    let reply = client
        .put(
            "/api/v1/preferences",
            json!({"tags": [{"id": tags[0], "name": "work"}], "strip_removed_tags": true}),
        )
        .await
        .expect(StatusCode::OK);
    assert_eq!(reply["tag_links_removed"], 0);
    assert_eq!(usage_of(&client.state().await, &tags[0]), 1);
}

#[tokio::test]
async fn a_tag_deleted_earlier_is_not_stripped_by_a_later_request() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work", "holiday"]).await;
    let id = tagged(&client, "Taxi", 3_400, &[tags[0].clone(), tags[1].clone()]).await;

    // Delete the first without stripping.
    client
        .put("/api/v1/preferences", json!({"tags": [{"id": tags[1], "name": "holiday"}]}))
        .await
        .expect(StatusCode::OK);

    // A later request that strips must only affect what *it* removes.
    let reply = client
        .put("/api/v1/preferences", json!({"tags": [], "strip_removed_tags": true}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(reply["tag_links_removed"], 1, "only the holiday link");

    let state = client.state().await;
    let transaction =
        state["transactions"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap();
    // The one deleted earlier is still on the entry, because nothing ever
    // asked for it to come off.
    assert_eq!(transaction["tags"].as_array().unwrap(), &vec![json!(tags[0])]);
}

#[tokio::test]
async fn the_flag_alone_does_not_touch_tags() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;

    // No `tags` group, so the group is not synced at all and nothing is
    // removed — the flag qualifies a removal rather than causing one.
    let reply = client
        .put("/api/v1/preferences", json!({"strip_removed_tags": true}))
        .await
        .expect(StatusCode::OK);
    assert_eq!(reply["tag_links_removed"], 0);
    assert!(
        !reply["updated"].as_array().unwrap().iter().any(|group| group == "tags"),
        "the tag group was not touched"
    );
    assert_eq!(usage_of(&client.state().await, &tags[0]), 1);
}

#[tokio::test]
async fn an_explicit_false_behaves_like_saying_nothing() {
    let client = Client::start().await;
    let tags = make_tags(&client, &["work"]).await;
    let id = tagged(&client, "Lunch", 1_200, &[tags[0].clone()]).await;

    client
        .put("/api/v1/preferences", json!({"tags": [], "strip_removed_tags": false}))
        .await
        .expect(StatusCode::OK);

    let state = client.state().await;
    let transaction =
        state["transactions"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap();
    assert_eq!(transaction["tags"][0], tags[0], "history kept");
}
