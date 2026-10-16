//! A prepared answer that was never sent cannot block publication after its question changes.

use serde_json::{Value, json};

use super::{
    crash,
    harness::{ERROR_POLICY, Harness, carried_open, read_json, write_json},
};

const PUBLISH: [&str; 3] = ["publish", "--effects", "review"];
const ABANDON: [&str; 4] = [
    "publish",
    "--abandon",
    "--reason",
    "The question changed before the answer was sent.",
];

/// Start and accept a run that keeps finding 1 open and answers `question`.
fn answering_run(harness: &Harness, question: u64, body: &str) -> String {
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    carried_open(&mut draft);
    draft["answers"] = json!([{"finding": 1, "in_reply_to": question.to_string(), "body": body}]);
    write_json(&path, &draft);
    let accepted = harness.ok(&["accept"]);
    assert_eq!(accepted["status"], "stored", "{accepted:#}");
    run_id
}

/// Answers the reviewer posted in finding 1's thread.
fn answers(harness: &Harness) -> Vec<String> {
    let state = harness.http.state.lock().unwrap();
    state.discussions[0]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|note| note["author"]["id"] == 42)
        .skip(1)
        .map(|note| note["body"].as_str().unwrap().to_owned())
        .collect()
}

fn edit_question(harness: &Harness, question: u64, body: &str) {
    let mut state = harness.http.state.lock().unwrap();
    let note = state.discussions[0]["notes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|note| note["id"] == question)
        .unwrap();
    note["body"] = json!(body);
}

/// Event kinds recorded in the run's publication events.
fn event_kinds(harness: &Harness, run_id: &str) -> Vec<String> {
    read_json(&harness.file(run_id, "review-publication-events.json"))["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn stale_prepared_answer_settles(recover_first: bool) {
    let harness = Harness::new();
    harness.accepted_with_finding();
    let first = harness.ok(&PUBLISH);
    assert_eq!(first["status"], "settled", "{first:#}");
    let question = harness
        .http
        .state
        .lock()
        .unwrap()
        .question("discussion-1", "Does the caller now receive the cause?");
    let owner = answering_run(&harness, question, "Yes: the caller receives the cause.");
    crash(&harness, "publish.after_intent:2", &PUBLISH);
    let pending = harness.journal();
    assert_eq!(pending["coverage_binding"]["run_id"], owner);
    assert_eq!(pending["dispatch_may_have_started"], true);
    assert_eq!(pending["unresolved"]["phase"], "prepared");
    assert_eq!(pending["unresolved"]["write"]["kind"], "reply");
    assert!(answers(&harness).is_empty(), "the answer was never sent");

    edit_question(
        &harness,
        question,
        "Does the caller receive the cause and its context?",
    );
    let replies = harness.writes_to("POST", "/discussions/discussion-1/notes");
    if recover_first {
        let (output, recovered) = harness.rkc(&PUBLISH);
        assert_ne!(output.status.code(), Some(6), "{recovered:#}");
        assert_ne!(output.status.code(), Some(0), "{recovered:#}");
        assert_eq!(recovered["status"], "unfinished", "{recovered:#}");
        let next = recovered["next"].as_str().unwrap();
        assert!(next.contains("rkc publish --abandon"), "{recovered:#}");
        assert!(!next.contains("publish again"), "{recovered:#}");
    }
    let abandoned = harness.ok(&ABANDON);
    assert_eq!(abandoned["status"], "settled", "{abandoned:#}");
    assert_eq!(harness.manifest(&owner)["completion"], "abandoned");
    assert_eq!(
        harness.writes_to("POST", "/discussions/discussion-1/notes"),
        replies,
        "the stale answer is never posted"
    );
    let journal = harness.journal();
    assert_eq!(journal["unresolved"], Value::Null);
    assert_eq!(journal["dispatch_may_have_started"], true);
    assert_eq!(journal["coverage_binding"], pending["coverage_binding"]);
    assert_eq!(journal["authorization"], pending["authorization"]);
    assert_eq!(journal["retried_creations"], pending["retried_creations"]);
    assert!(
        event_kinds(&harness, &owner).contains(&"discarded_unsent".to_owned()),
        "the run's events record the discarded intent"
    );

    let fresh = answering_run(&harness, question, "Yes, with its context.");
    let published = harness.ok(&PUBLISH);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.manifest(&fresh)["completion"], "published");
    let posted = answers(&harness);
    assert_eq!(posted.len(), 1, "one answer: {posted:?}");
    assert!(posted[0].contains("Yes, with its context."), "{posted:?}");
}

#[test]
fn a_stale_prepared_answer_is_discarded_by_recovery_and_the_round_abandoned() {
    stale_prepared_answer_settles(true);
}

#[test]
fn a_stale_prepared_answer_is_discarded_by_abandonment() {
    stale_prepared_answer_settles(false);
}
