//! The captured `review-conversation.json`: each note's author ID, username and
//! bot state, and runs whose version 1 capture predates author accounts.

use std::fs;

use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness, read_json, sha256},
    server::{Fault, Response},
};

/// `[author, author_username, author_bot]` of every note in stored order.
fn accounts(document: &Value) -> Vec<Value> {
    document["discussions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|discussion| discussion["notes"].as_array().unwrap())
        .map(|note| json!([note["author"], note["author_username"], note["author_bot"]]))
        .collect()
}

#[test]
fn run_start_records_each_note_authors_username_and_bot_state() {
    let harness = Harness::new();
    {
        let mut state = harness.http.state.lock().unwrap();
        state.bot_users.insert(91);
        for (discussion, author, body) in [
            ("person", 77, "Could the error path log its cause?"),
            ("bot", 91, "Automated review: no blocking issues."),
            ("hidden", 92, "A note by an account GitLab does not show."),
        ] {
            let note = state.note(body, author);
            state
                .discussions
                .push(json!({"id": discussion, "individual_note": true, "notes": [note]}));
        }
        state
            .faults
            .push(Fault::response("GET", "/users/92", Response::status(404)));
    }
    let run_id = harness.start();
    let document = read_json(&harness.file(&run_id, "review-conversation.json"));
    assert_eq!(document["version"], 2);
    assert_eq!(
        accounts(&document),
        [
            json!(["77", "user-77", false]),
            json!(["91", "user-91", true]),
            json!(["92", "user-92", null]),
        ]
    );
}

/// A run whose conversation has the version 1 layout of rkc 0.1.0 still loads,
/// passes the freshness preflight against a current version 2 capture and
/// publishes.
#[test]
fn a_run_with_a_version_1_conversation_still_plans_and_publishes() {
    let harness = Harness::new();
    harness.add_comment("question", "Is the cause kept?");
    let run_id = harness.start();
    let path = harness.file(&run_id, "review-conversation.json");
    let mut document = read_json(&path);
    document["version"] = 1.into();
    for discussion in document["discussions"].as_array_mut().unwrap() {
        for note in discussion["notes"].as_array_mut().unwrap() {
            let note = note.as_object_mut().unwrap();
            note.remove("author_username").unwrap();
            note.remove("author_bot").unwrap();
        }
    }
    let bytes = serde_json::to_vec_pretty(&document).unwrap();
    fs::write(&path, &bytes).unwrap();
    let manifest = harness.file(&run_id, "review-run.json");
    let mut value = read_json(&manifest);
    value["snapshot_files"]["review-conversation.json"] = json!(sha256(&bytes));
    fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    harness.declare_policies(&run_id, &[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    harness.ok(&["accept"]);
    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "intact", "{status:#}");
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned:#}");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(
        read_json(&path)["version"],
        1,
        "the retained capture is never rewritten"
    );
}
