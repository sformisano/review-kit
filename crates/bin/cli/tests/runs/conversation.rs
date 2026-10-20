//! The captured `review-conversation.json`: each note's author ID, username and
//! bot state, and runs whose version 1 capture predates author accounts.

use std::fs;

use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness, read_json},
    server::{Fault, MR_PATH, Response},
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
            ("nameless", 93, "A note whose author has no username."),
        ] {
            let mut note = state.note(body, author);
            if author == 93 {
                note["author"].as_object_mut().unwrap().remove("username");
            }
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
            json!(["93", null, false]),
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
    harness.retain_conversation(&run_id, &document);

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

/// The freshness preflight never needs usernames, so a note whose author has
/// none, before or after capture, does not stop publication.
#[test]
fn a_note_author_without_a_username_does_not_stop_publication() {
    let harness = Harness::new();
    let nameless = |harness: &Harness, discussion: &str| {
        let mut state = harness.http.state.lock().unwrap();
        let mut note = state.note("A note whose author has no username.", 93);
        note["author"].as_object_mut().unwrap().remove("username");
        state
            .discussions
            .push(json!({"id": discussion, "individual_note": true, "notes": [note]}));
    };
    nameless(&harness, "before");
    let run_id = harness.accepted_with_finding();
    nameless(&harness, "after");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let document = read_json(&harness.file(&run_id, "review-conversation.json"));
    assert_eq!(accounts(&document), [json!(["93", null, false])]);
}

/// A conversation that a newer rkc wrote, with a higher version and a field
/// this build does not know, is reported as newer instead of damaged.
#[test]
fn a_conversation_from_a_newer_rkc_is_reported_as_newer_not_damaged() {
    let harness = Harness::new();
    harness.add_comment("question", "Is the cause kept?");
    let run_id = harness.start();
    let original = read_json(&harness.file(&run_id, "review-conversation.json"));
    let mut newer = original.clone();
    newer["version"] = 3.into();
    newer["discussions"][0]["notes"][0]["author_avatar"] = "avatar.png".into();
    harness.retain_conversation(&run_id, &newer);

    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "newer", "{status:#}");
    assert_eq!(
        status["retained_files"]["problems"][0]["code"],
        "newer_run_document"
    );
    assert!(
        status["next"].as_str().unwrap().contains("newer rkc"),
        "{status:#}"
    );
    for args in [
        vec!["check"],
        vec!["publish", "--effects", "review", "--plan"],
    ] {
        let refused = harness.fails(&args, 2, "newer_run_document");
        let diagnostic = &refused["diagnostics"][0];
        assert_eq!(diagnostic["file"], "review-conversation.json", "{args:?}");
        assert_eq!(diagnostic["version"], 3);
        assert_eq!(diagnostic["supported"], 2);
    }
    assert_eq!(harness.writes(), 0);
    harness.ok(&["run", "stop", "--reason", "Written by a newer rkc."]);

    // A malformed document of a version this build reads stays damaged.
    let damaged = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut malformed = original;
    malformed["discussions"][0]["notes"][0]["author_avatar"] = "avatar.png".into();
    harness.retain_conversation(&damaged, &malformed);
    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "damaged", "{status:#}");
    harness.fails(&["check"], 2, "invalid_run_document");
}

/// An owner of unfinished publication whose conversation a newer rkc wrote is
/// told to recover with that rkc, not to restore files.
#[test]
fn an_owner_whose_conversation_a_newer_rkc_wrote_is_told_to_use_that_rkc() {
    let harness = Harness::lost_responses();
    let owner = harness.ready_run(false);
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
    let (output, uncertain) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{uncertain:#}");
    let mut newer = read_json(&harness.file(&owner, "review-conversation.json"));
    newer["version"] = 3.into();
    harness.retain_conversation(&owner, &newer);
    let status = harness.ok(&["status"]);
    assert_eq!(
        status["journal"]["publication_status"], "pending",
        "{status:#}"
    );
    assert_eq!(status["retained_files"]["state"], "newer");
    let next = status["next"].as_str().unwrap();
    assert!(
        next.contains("a newer rkc wrote its retained files"),
        "{next}"
    );
    assert!(!next.contains("Restore"), "{next}");
}

/// Only an intact conversation file of the documented shape counts as newer:
/// a changed file, a non-object and a file of another kind stay damaged, and
/// a newer file next to other damage is damage.
#[test]
fn only_an_intact_conversation_counts_as_newer() {
    let harness = Harness::new();
    harness.add_comment("question", "Is the cause kept?");
    let run_id = harness.start();
    let path = harness.file(&run_id, "review-conversation.json");
    let original = read_json(&path);
    let mut newer = original.clone();
    newer["version"] = 3.into();

    fs::write(&path, serde_json::to_vec_pretty(&newer).unwrap()).unwrap();
    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "damaged", "{status:#}");
    let next = status["next"].as_str().unwrap();
    assert!(next.contains("restore the run's exact files"), "{status:#}");
    assert!(
        next.contains(
            "stop it with rkc run stop --reason TEXT and start a new run with rkc run start."
        ),
        "{status:#}"
    );
    harness.fails(&["check"], 2, "hash_mismatch");

    for shape in [json!(["review_conversation", 3]), json!({"version": 3}), {
        let mut other = newer.clone();
        other["kind"] = "review_context".into();
        other
    }] {
        harness.retain_conversation(&run_id, &shape);
        let status = harness.ok(&["status"]);
        assert_eq!(
            status["retained_files"]["state"], "damaged",
            "{shape}: {status:#}"
        );
    }

    harness.retain_conversation(&run_id, &newer);
    assert_eq!(harness.ok(&["status"])["retained_files"]["state"], "newer");
    fs::remove_file(harness.file(&run_id, "merge-request.diff")).unwrap();
    let status = harness.ok(&["status"]);
    let retained = &status["retained_files"];
    assert_eq!(retained["state"], "damaged", "{status:#}");
    assert_eq!(
        retained["files"],
        json!(["merge-request.diff", "review-conversation.json"])
    );
    let codes: Vec<_> = retained["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| problem["code"].clone())
        .collect();
    assert!(codes.contains(&json!("newer_run_document")), "{codes:?}");
    assert!(
        codes.iter().any(|code| code != "newer_run_document"),
        "{codes:?}"
    );
}
