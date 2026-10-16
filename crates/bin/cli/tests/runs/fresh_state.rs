//! Follow-up reviews from a new environment: an empty publication state root
//! and a worktree without review runs, while GitLab keeps every earlier round.

use std::fs;

use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness, PROOF_POLICY, read_json, ready, write_json},
    server::{Fault, MR_PATH},
};

/// Move the publication state root and the worktree's runs aside, as a new
/// container starts without them. The provider keeps everything published.
fn fresh_instance(harness: &Harness, label: &str) {
    let previous = harness.root.path().join(label);
    fs::create_dir_all(&previous).unwrap();
    fs::rename(harness.state(), previous.join("state")).unwrap();
    fs::rename(harness.runtime(), previous.join("review-kit")).unwrap();
}

/// Reassess the carried finding 1 as fixed, with its transition reply.
fn accept_fix(harness: &Harness, run_id: &str) {
    let path = harness.file(run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    assert_eq!(
        draft["findings"][0]["id"], 1,
        "the published finding keeps its ID"
    );
    ready(&mut draft);
    let mut fixed = draft["findings"][0]["reassessment"].clone();
    fixed["status"] = json!("fixed");
    fixed["evidence"] = json!("The failure branch now returns the cause.");
    draft["findings"][0]["reassessment"] = fixed;
    draft["transitions"] = json!([{"finding":1,"from":"open","to":"fixed",
        "body":"The failure path now keeps its cause; the regression passes."}]);
    write_json(&path, &draft);
    harness.ok(&["accept"]);
}

#[test]
fn a_fresh_state_root_publishes_the_next_round_after_a_finished_round() {
    let harness = Harness::new();
    let first = harness.accepted_with_finding();
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let root = harness.http.state.lock().unwrap().discussions[0]["notes"][0]["id"].clone();
    fresh_instance(&harness, "first-instance");

    let follow_up = harness.draft_with_policies(&[ERROR_POLICY]);
    assert_ne!(follow_up, first);
    accept_fix(&harness, &follow_up);
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        planned["effects"]["round_ceiling"],
        Value::Null,
        "{planned:#}"
    );
    assert_eq!(planned["effects"]["outside"], json!([]), "{planned:#}");
    assert!(
        harness.journal_dir().is_none(),
        "planning creates no journal"
    );

    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(published["journal_origin"], "bootstrapped");
    assert_eq!(published["settlement"]["outcome"], "published");
    assert_eq!(published["settlement"]["round"], 2);
    assert_eq!(harness.manifest(&follow_up)["completion"], "published");
    assert_eq!(harness.summaries(), 1, "the reviewer keeps one summary");
    assert_eq!(
        harness.writes_to("POST", &format!("{MR_PATH}/notes")),
        1,
        "the summary is updated, never created again"
    );
    let discussions = harness.http.state.lock().unwrap().discussions.clone();
    assert_eq!(discussions.len(), 1, "finding 1 keeps its thread");
    assert_eq!(discussions[0]["notes"][0]["id"], root);
    assert_eq!(discussions[0]["notes"].as_array().unwrap().len(), 2);
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], follow_up);
    assert_eq!(journal["coverage_binding"]["round"], 2);
    assert_eq!(journal["settled_runs"].as_array().unwrap().len(), 1);

    fresh_instance(&harness, "second-instance");
    let next = harness.start();
    let captured = read_json(&harness.file(&next, "review-context.json"));
    let summary = &captured["snapshot"]["summaries"][0];
    assert_eq!(summary["current"]["number"], 2, "{summary:#}");
    let history: Vec<&Value> = summary["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|round| &round["number"])
        .collect();
    assert_eq!(history, [&json!(1)], "round 1 stays in the summary history");
}

#[test]
fn a_fresh_state_root_with_an_unchanged_assessment_writes_nothing() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let provider = {
        let state = harness.http.state.lock().unwrap();
        (state.notes.clone(), state.discussions.clone())
    };
    let writes = harness.writes();
    fresh_instance(&harness, "first-instance");

    let repeat = harness.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY]);
    let path = harness.file(&repeat, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind":"violated","evidence":"The failure path discards the actionable error."});
        }
    }
    write_json(&path, &draft);
    harness.ok(&["accept"]);

    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        planned["run"]["outcome"]["plan"]["unchanged"], true,
        "{planned:#}"
    );
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(published["journal_origin"], "bootstrapped");
    assert_eq!(published["settlement"]["outcome"], "no_changes");
    assert_eq!(published["settlement"]["round"], 1, "no new round");
    assert_eq!(harness.writes(), writes, "no provider writes");
    let state = harness.http.state.lock().unwrap();
    assert_eq!((state.notes.clone(), state.discussions.clone()), provider);
}

/// Two environments whose first publication left round 1 unfinished on
/// GitLab: `pending` after a lost finding response, `partial` after a denied
/// finding effect. Each still holds the journal and the run that started it.
fn unfinished_rounds() -> [(Harness, &'static str); 2] {
    let pending = Harness::lost_responses();
    pending.accepted_with_finding();
    pending
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let (output, uncertain) = pending.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{uncertain:#}");

    let partial = Harness::new();
    partial.accepted_with_finding();
    let (output, denied) = partial.rkc(&["publish", "--effects", "create_summary,update_summary"]);
    assert_eq!(output.status.code(), Some(4), "{denied:#}");
    [(pending, "pending"), (partial, "partial")]
}

/// An unfinished round on GitLab keeps its possible unresolved write and
/// effect ceiling in the state root that started it, so a new environment
/// refuses before any write and says what to restore.
#[test]
fn a_fresh_state_root_refuses_a_pending_or_partial_round_with_guidance() {
    for (harness, status) in unfinished_rounds() {
        let writes = harness.writes();
        fresh_instance(&harness, "first-instance");
        // Acceptance already requires the unfinished round's exact assessment.
        let run_id = harness.accepted_with_finding();
        let captured = read_json(&harness.file(&run_id, "review-context.json"));
        let round = &captured["snapshot"]["summaries"][0]["current"];
        assert_eq!(round["number"], 1);
        assert_eq!(round["status"], status, "{round:#}");
        for args in [
            vec!["publish", "--effects", "review", "--plan"],
            vec!["publish", "--effects", "review"],
        ] {
            let refused = harness.fails(&args, 5, "journal_missing");
            let diagnostic = &refused["diagnostics"][0];
            assert_eq!(diagnostic["round"], 1, "{status} {args:?}: {refused:#}");
            let message = diagnostic["message"].as_str().unwrap();
            assert!(message.contains("round 1"), "{status}: {message}");
            assert!(message.contains("unfinished"), "{status}: {message}");
            let next = diagnostic["next"].as_str().unwrap();
            for expected in [
                "latest round finished",
                "publication_state",
                "publication-progress.json",
                "rkc run start --new",
                "rkc publish",
            ] {
                assert!(next.contains(expected), "{status} {args:?}: {next}");
            }
            assert!(!next.contains("publish again"), "{status}: {next}");
        }
        assert_eq!(harness.writes(), writes, "{status}: no provider writes");
        assert!(harness.journal_dir().is_none(), "{status}: no journal");
        assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);
    }
}

/// The draft commands with what each asks to do after fixing the draft.
const DRAFT_COMMANDS: [(&str, &str); 2] = [
    ("check", "check it again"),
    ("accept", "run rkc accept again"),
];

/// A changed assessment of an unfinished round is still refused, but in a new
/// environment the `next` of `check` and `accept` names the missing publication
/// state: accepting the round's exact assessment would only lead publication to
/// `journal_missing`.
#[test]
fn a_fresh_state_root_explains_a_changed_assessment_of_an_unfinished_round() {
    for (harness, status) in unfinished_rounds() {
        let retained = harness.ok(&["run", "start", "--new"])["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        harness.declare_policies(&retained, &[ERROR_POLICY, PROOF_POLICY]);
        harness.fill_ready(&retained);
        for (command, retry) in DRAFT_COMMANDS {
            let refused = harness.fails(&[command], 2, "changed_pending_assessment");
            assert_eq!(
                refused["next"],
                format!("Fix review-assessment.draft.json and {retry}."),
                "{status} {command}: with the round's journal here, the draft is what to fix: {refused:#}"
            );
        }

        let writes = harness.writes();
        fresh_instance(&harness, "first-instance");
        let run_id = harness.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY]);
        harness.fill_ready(&run_id);
        for (command, _) in DRAFT_COMMANDS {
            let refused = harness.fails(&[command], 2, "changed_pending_assessment");
            assert_eq!(
                refused["accepted"],
                Value::Null,
                "{status} {command}: {refused:#}"
            );
            let next = refused["next"].as_str().unwrap();
            for expected in [
                "GitLab shows round 1 of this reviewer's summary unfinished",
                "journal_missing",
                "publication_state",
                "publication-progress.json",
                "the review run that started round 1",
                "let it finish, then start a fresh run here with rkc run start --new",
            ] {
                assert!(next.contains(expected), "{status} {command}: {next}");
            }
            assert!(
                !next.contains("Fix review-assessment"),
                "{status} {command}: {next}"
            );
        }
        assert_eq!(harness.writes(), writes, "{status}: no provider writes");
        assert!(harness.journal_dir().is_none(), "{status}: no journal");
    }
}

/// Accept a ready assessment or, when `blocked`, one whose proof policy is
/// unavailable, with the same policy declarations either way.
fn accept_ready_or_blocked(harness: &Harness, blocked: bool) -> String {
    let run_id = harness.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    if blocked {
        let limitation = "The integration runner is unavailable.";
        draft["decision"] = json!("blocked");
        draft["limitation"] = json!(limitation);
        for row in draft["policies"].as_array_mut().unwrap() {
            if row["policy_id"] == PROOF_POLICY {
                row["outcome"] = json!({"kind":"blocked", "limitation": limitation});
            }
        }
    }
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    run_id
}

/// A blocked round without finding actions finishes in place. Its retained
/// ceiling caps that only while its journal exists; a new environment starts
/// the continuation with a new ceiling from its own first write.
#[test]
fn a_fresh_state_root_finishes_a_blocked_round_in_place_under_current_authority() {
    let harness = Harness::new();
    accept_ready_or_blocked(&harness, true);
    let blocked = harness.ok(&["publish", "--effects", "create_summary"]);
    assert_eq!(blocked["publication"]["status"], "blocked", "{blocked:#}");
    assert_eq!(blocked["settlement"]["round"], 1);

    accept_ready_or_blocked(&harness, false);
    let retained = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        retained["effects"]["round_ceiling"],
        json!({"round": 1, "effects": ["create_summary"], "retained": true}),
        "{retained:#}"
    );
    assert_eq!(retained["effects"]["outside"], json!(["update_summary"]));

    fresh_instance(&harness, "first-instance");
    accept_ready_or_blocked(&harness, false);
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        planned["run"]["outcome"]["plan"]["round"]["number"], 1,
        "{planned:#}"
    );
    assert_eq!(
        planned["effects"]["round_ceiling"],
        Value::Null,
        "{planned:#}"
    );
    assert_eq!(planned["effects"]["outside"], json!([]));
    let completed = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(completed["status"], "settled", "{completed:#}");
    assert_eq!(completed["publication"]["status"], "completed");
    assert_eq!(
        completed["settlement"]["round"], 1,
        "the blocked round finishes in place"
    );
    assert_eq!(harness.summaries(), 1);
    assert_eq!(harness.journal()["authorization"]["round"], 1);
}
