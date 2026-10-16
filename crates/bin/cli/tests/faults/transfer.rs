//! A run that attempted publication never starts over without its journal.

use std::fs;

use review_kit_application::publication::RoundStatus;
use review_kit_json_adapter::rounds::State;
use serde_json::json;

use super::{
    crash,
    harness::{Harness, write_json},
};

const PUBLISH: [&str; 3] = ["publish", "--effects", "review"];

/// The number and status of the current round in the reviewer's summary.
fn provider_round(harness: &Harness) -> (u64, RoundStatus) {
    let body = harness.http.state.lock().unwrap().notes[0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, tail) = body.split_once("\n\n<!-- review-kit:state:v3 ").unwrap();
    let (payload, _) = tail.split_once(" -->\n\n<!-- review-kit:v1 ").unwrap();
    let (current, _) = State::decode(payload.as_bytes())
        .unwrap()
        .into_rounds()
        .unwrap();
    (current.number, current.status)
}

/// Every publication command of the run refuses with `journal_missing` and writes nothing.
fn refused_without_a_journal(harness: &Harness, case: &str) {
    for args in [
        PUBLISH.to_vec(),
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--abandon", "--reason", "Moved to another host."],
    ] {
        let refused = harness.fails(&args, 5, "journal_missing");
        let next = refused["diagnostics"][0]["next"]
            .as_str()
            .unwrap_or_default();
        assert!(next.contains("restore"), "{case} {args:?}: {refused:#}");
        assert!(!next.contains("publish again"), "{case}: {refused:#}");
    }
    assert_eq!(harness.writes(), 0, "{case}: no provider writes");
    assert!(
        harness.journal_dir().is_none(),
        "{case}: no journal is recreated"
    );
}

#[test]
fn a_run_that_reached_its_dispatch_checkpoint_refuses_a_moved_state_root() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(&harness, "publish.after_dispatch_flag:1", &PUBLISH);
    assert_eq!(harness.writes(), 0, "the abort precedes the HTTP request");
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], true);
    let journal = harness.journal();
    assert_eq!(journal["unresolved"]["phase"], "dispatch_possible");

    let state = harness.state();
    let disabled = state.with_file_name("state.disabled");
    fs::rename(&state, &disabled).unwrap();
    refused_without_a_journal(&harness, "attempted manifest");

    // An older manifest copy without the attempt marker: the run's own events
    // still record the dispatch checkpoint.
    let manifest = harness.file(&run_id, "review-run.json");
    let mut older = harness.manifest(&run_id);
    older["publication_attempted"] = json!(false);
    write_json(&manifest, &older);
    refused_without_a_journal(&harness, "events record the attempt");
    harness.fails(
        &["run", "stop", "--reason", "Moved to another host."],
        5,
        "journal_unavailable",
    );
    let status = harness.ok(&["status"]);
    assert!(
        status["next"].as_str().unwrap().contains("restore"),
        "{status:#}"
    );

    // Restoring the authoritative state lets the same run recover.
    fs::remove_dir_all(&state).unwrap();
    fs::rename(&disabled, &state).unwrap();
    let recovered = harness.ok(&PUBLISH);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(recovered["journal_origin"], "loaded");
    assert_eq!(recovered["publication"]["retried_creations"], 1);
    assert_eq!(harness.writes_to("POST", "/merge_requests/7/notes"), 1);
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        1
    );
}

/// The provider's finished round admits an empty journal only for a run that
/// never attempted publication; the attempted run still needs its own journal.
#[test]
fn an_attempted_run_refuses_a_moved_state_root_even_after_its_round_finished() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(&harness, "publish.before_settlement", &PUBLISH);
    assert_eq!(provider_round(&harness), (1, RoundStatus::Completed));
    assert_eq!(harness.manifest(&run_id)["status"], "open");
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], true);
    assert_eq!(harness.journal()["settled_runs"], json!([]));
    let writes = harness.writes();

    let state = harness.state();
    fs::rename(&state, state.with_file_name("state.disabled")).unwrap();
    for args in [
        PUBLISH.to_vec(),
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--abandon", "--reason", "Moved to another host."],
    ] {
        let refused = harness.fails(&args, 5, "journal_missing");
        let next = refused["diagnostics"][0]["next"].as_str().unwrap();
        assert!(next.contains("restore"), "{args:?}: {refused:#}");
    }
    assert_eq!(harness.writes(), writes, "no provider writes");
    assert!(harness.journal_dir().is_none(), "no journal is recreated");
    assert_eq!(harness.manifest(&run_id)["status"], "open");
}

#[test]
fn a_run_that_never_attempted_publication_still_bootstraps_its_journal() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["journal"], "absent");
    assert!(
        harness.journal_dir().is_none(),
        "planning creates no journal"
    );
    let published = harness.ok(&PUBLISH);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(published["journal_origin"], "bootstrapped");
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
}
