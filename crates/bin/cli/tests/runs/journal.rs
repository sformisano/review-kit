use std::fs;

use review_kit_application::use_cases::publishing::PublicationProgress;
use review_kit_filesystem_adapter::FileProgressStore;
use review_kit_json_adapter::ProgressDocument;
use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness},
    server::{Fault, MR_PATH, Response},
};

fn store(harness: &Harness) -> FileProgressStore {
    let client = harness.http.connect();
    FileProgressStore::new(harness.state(), client.subject(), client.reviewer())
}

#[test]
fn conservative_recovery_never_resends_an_absent_ambiguous_creation_across_restarts() {
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
    let pending = harness.journal();
    assert_eq!(pending["unresolved"]["phase"], "dispatch_possible");
    assert_eq!(pending["coverage_binding"]["run_id"], owner);
    assert_eq!(
        pending["retried_creations"].as_array().map_or(0, Vec::len),
        0
    );
    assert!(pending["authorization"].is_object());
    assert_eq!(harness.summaries(), 0);
    let writes = harness.writes();
    let creations = harness.writes_to("POST", "/notes");
    assert_eq!(creations, 1, "one ambiguous initial request");

    for _ in 0..2 {
        let (output, conservative) = harness.rkc(&[
            "publish",
            "--effects",
            "review",
            "--retry-uncertain",
            "never",
        ]);
        assert_eq!(output.status.code(), Some(6), "{conservative:#}");
        assert_eq!(conservative["status"], "unfinished");
        assert_eq!(harness.journal(), pending, "intent, grant and budget kept");
        assert_eq!(harness.writes(), writes, "no retry mutations");
        assert_eq!(harness.writes_to("POST", "/notes"), creations);
        assert_eq!(harness.summaries(), 0);
        assert_eq!(harness.manifest(&owner)["status"], "open");
    }

    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled");
    assert_eq!(harness.writes_to("POST", "/notes"), creations + 1);
    assert_eq!(harness.summaries(), 1, "one default controlled resend");
    let settled = harness.journal();
    assert_eq!(settled["unresolved"], Value::Null);
    assert_eq!(settled["retried_creations"].as_array().unwrap().len(), 1);
    assert_eq!(settled["authorization"], pending["authorization"]);
}

#[test]
fn fresh_runs_continue_locally_while_attempted_runs_need_their_journal() {
    let harness = Harness::new();
    let attempted = harness.ready_run(false);
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response(
            "POST",
            &format!("{MR_PATH}/notes"),
            Response::status(422),
        ));
    let (output, rejected) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(7), "{rejected:#}");
    assert_eq!(rejected["status"], "unfinished");
    let manifest = harness.manifest(&attempted);
    assert_eq!(manifest["status"], "open");
    assert_eq!(manifest["publication_attempted"], true);
    assert_eq!(
        harness.journal()["coverage_binding"],
        json!(null),
        "rejected first write rolls back"
    );

    let journal = harness
        .journal_dir()
        .unwrap()
        .join("publication-progress.json");
    let saved = fs::read(&journal).unwrap();
    fs::write(&journal, b"{").unwrap();
    for args in [
        vec!["revise", "--reason", "Reconsider."],
        vec!["accept"],
        vec!["run", "finish"],
        vec!["run", "stop", "--reason", "Stop it."],
    ] {
        harness.fails(&args, 5, "journal_unavailable");
    }
    harness.fails(
        &["publish", "--effects", "review"],
        5,
        "journal_unavailable",
    );
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["state"], "unavailable");
    let next = status["next"].as_str().unwrap();
    assert!(next.contains("unavailable"), "{next}");
    assert!(next.contains("restore"), "{next}");
    assert!(next.contains("publication-progress.json"), "{next}");
    assert!(next.contains("authoritative"), "{next}");
    assert!(!next.contains("rkc run finish"), "{next}");
    assert!(!next.contains("rkc run stop"), "{next}");
    harness.ok(&["check"]);
    harness.ok(&["run", "list"]);

    let fresh = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.declare_policies(&fresh, &[ERROR_POLICY]);

    harness.ok(&["revise", "--reason", "Local work needs no journal."]);
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        5,
        "journal_unavailable",
    );
    harness.ok(&[
        "run",
        "stop",
        "--reason",
        "Superseded by the attempted run.",
    ]);

    fs::write(&journal, saved).unwrap();
    harness.ok(&["run", "select", &attempted]);
    harness.ok(&["revise", "--reason", "The journal is available again."]);
}

#[test]
fn another_runs_pending_publication_blocks_replacement_until_it_settles() {
    let harness = Harness::lost_responses();
    let owner = harness.accepted_with_finding();
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
    let (output, uncertain) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{uncertain:#}");
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert_eq!(journal["dispatch_may_have_started"], true);
    assert_eq!(journal["unresolved"]["phase"], "dispatch_possible");
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["publication_owner"], owner);
    assert_eq!(status["journal"]["publication_status"], "pending");
    assert_eq!(
        status["journal"]["unresolved_effect"]["kind"],
        "create_summary"
    );
    for args in [
        vec!["accept"],
        vec!["revise", "--reason", "Change it."],
        vec!["run", "finish"],
        vec!["run", "stop", "--reason", "Give up."],
    ] {
        let refused = harness.fails(&args, 5, "publication_pending_owner");
        assert_eq!(refused["diagnostics"][0]["owner_run_id"], owner);
    }

    let replacement = harness.ready_run(true);
    let writes = harness.writes();
    let blocked = harness.fails(
        &["publish", "--effects", "review", "--plan"],
        5,
        "publication_pending_owner",
    );
    assert_eq!(blocked["diagnostics"][0]["owner_run_id"], owner);
    let refused = harness.fails(
        &["publish", "--effects", "review"],
        5,
        "publication_pending_owner",
    );
    assert_eq!(refused["diagnostics"][0]["owner_run_id"], owner);
    assert_eq!(harness.writes(), writes, "no replacement writes");
    assert_eq!(harness.journal()["coverage_binding"]["run_id"], owner);
    assert_eq!(harness.journal()["unresolved"], journal["unresolved"]);
    let replacement_status = harness.ok(&["status"]);
    assert_eq!(replacement_status["journal"]["publication_owner"], owner);

    harness.ok(&["run", "select", &owner]);
    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(recovered["recovery"], true);
    assert_eq!(harness.manifest(&owner)["completion"], "published");
    assert_eq!(harness.summaries(), 1, "one summary after the resend");
    assert_eq!(harness.discussions(), 1);

    harness.ok(&["run", "select", &replacement]);
    harness.fails(
        &["publish", "--effects", "review"],
        3,
        "conversation_changed",
    );
    harness.ok(&[
        "run",
        "stop",
        "--reason",
        "Captured before the earlier publication settled.",
    ]);

    let follow_up = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.declare_policies(&follow_up, &[ERROR_POLICY]);

    harness.fill_carried_open(&follow_up);
    harness.ok(&["accept"]);
    let discussions = harness.discussions();
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(
        harness.discussions(),
        discussions,
        "the open finding keeps its thread"
    );
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], follow_up);
    let settled: Vec<&Value> = journal["settled_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["run_id"])
        .collect();
    assert_eq!(settled, [&json!(owner), &json!(follow_up)]);
}

#[test]
fn unrelated_files_cannot_replace_or_block_the_current_journal() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let store = store(&harness);
    fs::create_dir_all(store.journal_path().parent().unwrap()).unwrap();
    let unrelated = b"unrelated private bytes, not a journal";
    fs::write(store.path(), unrelated).unwrap();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        harness.fails(&args, 5, "journal_missing");
        assert_eq!(harness.writes(), 0);
        assert!(!store.journal_path().exists());
        assert_eq!(fs::read(store.path()).unwrap(), unrelated);
    }
    let client = harness.http.connect();
    let journal = ProgressDocument::journal(
        client.subject(),
        client.reviewer(),
        &PublicationProgress::default(),
    );
    fs::write(
        store.journal_path(),
        review_kit_json_adapter::encode(&journal).unwrap(),
    )
    .unwrap();
    harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(harness.writes(), 0);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled");
    assert_eq!(published["journal_origin"], "loaded");
    assert_eq!(harness.journal()["settled_runs"][0]["run_id"], run_id);
    assert_eq!(fs::read(store.path()).unwrap(), unrelated);
}

#[test]
fn preflight_rejects_changed_source_before_first_dispatch() {
    let harness = Harness::new();
    let first = harness.ready_run(false);
    harness.push_commit();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let moved = harness.fails(&args, 3, "source_changed");
        assert_eq!(moved["diagnostics"][0]["current_head"], "4".repeat(40));
    }
    assert_eq!(harness.writes(), 0);
    assert_eq!(harness.manifest(&first)["publication_attempted"], false);
    assert!(harness.journal_dir().is_none());
    harness.ok(&[
        "run",
        "stop",
        "--reason",
        "Source changed before publication.",
    ]);
    let second = harness.ready_run(true);
    assert_ne!(second, first);
    assert_eq!(
        harness.manifest(&second)["revision"]["head"],
        "4".repeat(40)
    );
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
}

/// A run accepted before another run's partial publication is refused by that
/// unfinished publication, not by the conversation it changed, and is told how
/// the owner recovers.
#[test]
fn a_pending_owner_is_reported_before_the_freshness_preflight() {
    let harness = Harness::new();
    let early = harness.ready_run(false);
    let owner = harness.accepted_follow_up();
    let (output, partial) = harness.rkc(&["publish", "--effects", "create_summary,update_summary"]);
    assert_eq!(output.status.code(), Some(4), "{partial:#}");
    assert_eq!(partial["status"], "unfinished");
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert_eq!(journal["unresolved"], json!(null));

    harness.ok(&["run", "select", &early]);
    let writes = harness.writes();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let refused = harness.fails(&args, 5, "publication_pending_owner");
        let diagnostic = &refused["diagnostics"][0];
        assert_eq!(diagnostic["owner_run_id"], owner, "{args:?}");
        let next = diagnostic["next"].as_str().unwrap();
        assert!(next.contains(&format!("rkc run select {owner}")), "{next}");
        assert!(next.contains("rkc publish --abandon"), "{next}");
        assert!(!next.contains("publish again"), "{next}");
    }
    assert_eq!(harness.writes(), writes, "the refused run writes nothing");
    assert_eq!(harness.manifest(&early)["publication_attempted"], false);
    assert_eq!(harness.journal(), journal, "the journal is unchanged");

    harness.ok(&["run", "select", &owner]);
    let abandoned = harness.ok(&[
        "publish",
        "--abandon",
        "--reason",
        "Published with a narrow grant.",
    ]);
    assert_eq!(abandoned["status"], "settled", "{abandoned:#}");
    harness.ok(&["run", "select", &early]);
    harness.fails(
        &["publish", "--effects", "review"],
        3,
        "conversation_changed",
    );
}
