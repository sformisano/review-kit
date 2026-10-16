//! Real command outputs carry every JSON path that the CLI guide's result
//! inspection examples read (`crates/bin/cli/README.md#result-inspection`).
//!
//! Each test produces one family of results against the controlled GitLab
//! fixture. Every listed path must select at least one value of the documented
//! JSON type, and each documented status must be the one the command reports.
//! A renamed, moved or retyped field fails here instead of leaving the guide's
//! examples silently wrong.

use std::{fs, os::unix::fs::PermissionsExt, sync::mpsc::sync_channel};

use serde_json::{Value, json};

use super::{
    harness::{
        ERROR_POLICY, Harness, POLICY_SET, PROOF_POLICY, finding, read_json, ready, write_json,
    },
    server::{Fault, MR_PATH, Response},
};

/// A guide path and the JSON types its values may have, separated by `|`.
type Paths = &'static [(&'static str, &'static str)];

/// Outer errors: a document without `command`.
const OUTER_ERROR: Paths = &[
    (".status", "string"),
    (".diagnostics", "array"),
    (".diagnostics[].code", "string"),
    (".diagnostics[].message", "string"),
];

/// Every command result names its command and status.
const COMMAND: Paths = &[(".command", "string"), (".status", "string")];

/// Assessment validity and diagnostics of `check` and `accept`.
const STAGE: Paths = &[
    (".command", "string"),
    (".status", "string"),
    (".diagnostics", "array"),
];

/// Fields of each assessment diagnostic.
const STAGE_DIAGNOSTIC: Paths = &[
    (".diagnostics[].code", "string"),
    (".diagnostics[].message", "string"),
    (".diagnostics[].finding", "number|null"),
    (".diagnostics[].policy", "string|null"),
    (".diagnostics[].input", "string|null"),
];

/// The plan overview and summary bodies.
const PLAN: Paths = &[
    (".run.outcome.kind", "string"),
    (".run.outcome.plan.round.number", "number"),
    (".run.outcome.plan.unchanged", "boolean"),
    (".run.outcome.plan.unresolved_findings", "array"),
    (".run.outcome.plan.actions", "array"),
    (".run.outcome.plan.actions[0]", "object"),
    (".run.outcome.plan.actions[].kind", "string"),
    (".run.outcome.plan.summary_body", "string"),
    (".run.outcome.plan.terminal_summary_preview", "string"),
];

/// A `finding` action, relative to the action.
const FINDING_ACTION: Paths = &[
    (".body", "string"),
    (".finding.id", "number"),
    (".finding.location.path", "string"),
    (".finding.location.line", "number"),
    (".finding.suggestion.start_line", "number"),
    (".finding.suggestion.original[]", "string"),
    (".finding.suggestion.replacement[]", "string"),
    (".finding.suggestion.revision.head", "string"),
];

/// A `transition` reply action, relative to the action.
const TRANSITION_ACTION: Paths = &[
    (".body", "string"),
    (".finding", "number"),
    (".reply.kind", "string"),
    (".reply.status", "string"),
];

/// An `answer` reply action, relative to the action.
const ANSWER_ACTION: Paths = &[
    (".body", "string"),
    (".finding", "number"),
    (".reply.kind", "string"),
    (".reply.note", "string"),
];

/// A `set_resolved` action, relative to the action.
const RESOLUTION_ACTION: Paths = &[(".finding", "number"), (".resolved", "boolean")];

/// The publication outcome; `settlement` is null until the run settles.
const OUTCOME: Paths = &[
    (".command", "string"),
    (".status", "string"),
    (".settlement", "object|null"),
    (".publication.status", "string"),
    (".run.outcome.kind", "string"),
];

/// Failure homes of a publication that executed.
const EXECUTED: Paths = &[
    (".run.outcome.result.stop", "array"),
    (".run.outcome.result.finalization_stop", "array"),
    (".run.outcome.result.skipped", "array"),
    (".run.evidence_failure", "object|null"),
    (".storage_failures", "array"),
];

/// Attempted and verified writes.
const WRITES: Paths = &[
    (".run.outcome.result.attempted", "array"),
    (".run.outcome.result.attempted[].kind", "string"),
    (".run.outcome.result.verified", "array"),
    (".run.outcome.result.verified[].write", "object"),
    (".run.outcome.result.verified[].write.kind", "string"),
    (".run.outcome.result.verified[].readback", "object"),
];

/// The final evaluations and the finding list they are kept apart from.
const EVALUATIONS: Paths = &[
    (".assessment", "string"),
    (".run.final_assessment.decision", "string"),
    (".run.final_assessment.evaluations", "array"),
    (".run.final_assessment.evaluations[].policy_id", "string"),
    (
        ".run.final_assessment.evaluations[].judgment.kind",
        "string",
    ),
    (".run.final_assessment.findings", "array"),
    (".run.final_assessment.findings[].status", "string"),
];

/// Values a guide path selects: `.key` descends, `[]` selects every element,
/// `[N]` one element. A missing key or index selects nothing.
fn select<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut selected = vec![value];
    for segment in path.split('.').skip(1) {
        let (key, indexes) = segment.split_once('[').unwrap_or((segment, ""));
        if !key.is_empty() {
            selected = selected
                .into_iter()
                .filter_map(|value| value.get(key))
                .collect();
        }
        for index in indexes.split('[').filter(|index| !index.is_empty()) {
            selected = match index.trim_end_matches(']') {
                "" => selected
                    .into_iter()
                    .filter_map(Value::as_array)
                    .flatten()
                    .collect(),
                number => {
                    let number: usize = number.parse().unwrap();
                    selected
                        .into_iter()
                        .filter_map(|value| value.get(number))
                        .collect()
                }
            };
        }
    }
    selected
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Require each path to select at least one value, every one of a listed type.
#[track_caller]
fn assert_paths(case: &str, value: &Value, paths: Paths) {
    for (path, types) in paths {
        let selected = select(value, path);
        assert!(
            !selected.is_empty(),
            "{case}: {path} selects nothing in {value:#}"
        );
        for found in selected {
            assert!(
                types.split('|').any(|kind| kind == type_name(found)),
                "{case}: {path} is {} instead of {types}",
                type_name(found)
            );
        }
    }
}

/// Run `args` in the harness repository, require `exit` and return the document.
fn run(harness: &Harness, args: &[&str], exit: i32) -> Value {
    let (output, value) = harness.rkc(args);
    assert_eq!(output.status.code(), Some(exit), "{args:?}: {value:#}");
    value
}

/// The first plan action of `kind`, and of reply `reply` when given.
#[track_caller]
#[allow(clippy::panic, reason = "A missing planned action is a test failure.")]
fn action<'a>(plan: &'a Value, kind: &str, reply: Option<&str>) -> &'a Value {
    plan["run"]["outcome"]["plan"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| {
            action["kind"] == kind && reply.is_none_or(|reply| action["reply"]["kind"] == reply)
        })
        .unwrap_or_else(|| panic!("no {kind} {reply:?} action in {plan:#}"))
}

#[test]
fn guide_outer_error_and_stage_check_paths_exist() {
    let harness = Harness::new();
    let arguments = run(&harness, &["check", "nonsense"], 2);
    let no_run = run(&harness, &["publish", "--effects", "review", "--plan"], 2);
    for (error, code) in [(&arguments, "arguments"), (&no_run, "no_selected_run")] {
        assert_paths(code, error, OUTER_ERROR);
        assert_eq!(error["status"], "error", "{code}");
        assert_eq!(error["diagnostics"][0]["code"], code);
        assert!(error.get("command").is_none(), "{code}: {error:#}");
    }

    let run_id = harness.start();
    harness.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
    let checked = run(&harness, &["check"], 2);
    assert_paths("invalid assessment", &checked, STAGE);
    assert_paths("invalid assessment", &checked, STAGE_DIAGNOSTIC);
    assert_eq!(checked["status"], "invalid");
    let rejected = run(&harness, &["accept"], 2);
    assert_paths("rejected assessment", &rejected, STAGE);
    assert_paths("rejected assessment", &rejected, STAGE_DIAGNOSTIC);
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(
        read_json(&harness.file(&run_id, "review-check-result.json")),
        rejected,
        "the saved check result is the complete stdout document"
    );

    harness.fill_finding(&run_id);
    let checked = run(&harness, &["check"], 0);
    assert_paths("valid assessment", &checked, STAGE);
    assert_paths("valid assessment", &checked, &[(".check.plan", "object")]);
    assert_eq!(checked["status"], "valid");
    assert_eq!(checked["diagnostics"], json!([]));
    let accepted = run(&harness, &["accept"], 0);
    assert_paths("stored assessment", &accepted, STAGE);
    assert_eq!(accepted["status"], "stored");
    assert_eq!(
        read_json(&harness.file(&run_id, "review-check-result.json")),
        accepted
    );

    harness.add_comment("late-question", "Is the error cause kept?");
    let changed = run(&harness, &["publish", "--effects", "review", "--plan"], 3);
    assert_paths("conversation changed", &changed, OUTER_ERROR);
    assert_eq!(changed["diagnostics"][0]["code"], "conversation_changed");
    assert!(changed.get("command").is_none(), "{changed:#}");
}

/// Publish one blocking finding, then accept a follow-up run that fixes it,
/// answers a question in its thread and adds a finding with a suggestion.
///
/// Returns the follow-up run ID and its assessment acceptance.
fn follow_up_with_every_action(harness: &Harness) -> (String, Value) {
    harness.accepted_with_finding();
    run(harness, &["publish", "--effects", "review"], 0);
    let question = harness
        .http
        .state
        .lock()
        .unwrap()
        .question("discussion-1", "Does the caller now receive the cause?");
    let run_id = harness.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY, "rust-test-design"]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["policy_set"] = json!(POLICY_SET);
    draft["policies"] = json!(
        [ERROR_POLICY, PROOF_POLICY, "rust-test-design"]
            .iter()
            .map(|policy| json!({
                "policy_id": policy,
                "name": format!("Policy {policy}"),
                "required": *policy == PROOF_POLICY,
                "reason": "The changed source is governed by this policy.", "outcome": null,
            }))
            .collect::<Vec<_>>()
    );
    write_json(&path, &draft);

    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind": "violated", "evidence": "The new reader drops the first line's error context."});
        }
    }
    let slot = &mut draft["findings"][0];
    let mut reassessment = slot["reassessment"].clone();
    reassessment["status"] = json!("fixed");
    reassessment["evidence"] = json!("The failure branch now returns the cause.");
    slot["reassessment"] = reassessment;
    draft["transitions"] = json!([{"finding": 1, "from": "open", "to": "fixed",
        "body": "The failure branch now returns its cause; the regression passes."}]);
    draft["answers"] = json!([{"finding": 1, "in_reply_to": question.to_string(),
        "body": "Yes: the caller receives the original cause."}]);
    let mut suggested = finding();
    suggested["id"] = json!(2);
    suggested["title"] = json!("Label the first line");
    suggested["body"] = json!("The first line has no label, so a failure report cannot name it.");
    suggested["location"] = json!({"path": "src/new.rs", "line": 1});
    suggested["suggestion"] = json!({
        "location": {"path": "src/new.rs", "line": 1},
        "revision": draft["revision"].clone(),
        "start_line": 1,
        "original": ["first"],
        "replacement": ["first: labelled"],
        "evidence": "The label test passes with the replacement applied.",
    });
    draft["new_findings"] = json!([suggested]);
    write_json(&path, &draft);
    let accepted = run(harness, &["accept"], 0);
    assert_eq!(accepted["status"], "stored", "{accepted:#}");
    (run_id, accepted)
}

#[test]
fn guide_online_plan_completed_publication_and_evaluation_paths_exist() {
    let harness = Harness::new();
    let (run_id, accepted) = follow_up_with_every_action(&harness);
    let previews = [
        "rendered/review-summary.md",
        "rendered/terminal-review-summary.md",
    ];
    let before = previews.map(|file| fs::read(harness.file(&run_id, file)).unwrap());

    let planned = run(&harness, &["publish", "--effects", "review", "--plan"], 0);
    assert_paths("plan", &planned, COMMAND);
    assert_paths("plan", &planned, PLAN);
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["run"]["outcome"]["kind"], "planned");
    for (file, before) in previews.iter().zip(&before) {
        assert_eq!(
            &fs::read(harness.file(&run_id, file)).unwrap(),
            before,
            "{file} is not refreshed by an online plan"
        );
    }
    assert_paths(
        "finding action",
        action(&planned, "finding", None),
        FINDING_ACTION,
    );
    assert_paths(
        "transition action",
        action(&planned, "reply", Some("transition")),
        TRANSITION_ACTION,
    );
    assert_paths(
        "answer action",
        action(&planned, "reply", Some("answer")),
        ANSWER_ACTION,
    );
    let resolution = action(&planned, "set_resolved", None);
    assert_paths("resolution action", resolution, RESOLUTION_ACTION);
    assert!(resolution.get("body").is_none(), "{resolution:#}");
    assert_paths("plan", &planned, EVALUATIONS);
    let saved = read_json(&harness.file(&run_id, "review-publication-result.json"));
    assert_eq!(
        saved, planned["run"],
        "the saved result is stdout's run report"
    );
    assert_paths(
        "saved plan",
        &saved,
        &[
            (".outcome.plan.actions", "array"),
            (".final_assessment.evaluations", "array"),
        ],
    );

    // The final evaluations are the accepted assessment's judgments.
    let status = run(&harness, &["status"], 0);
    assert_paths("status", &status, &[(".assessment.accepted", "string")]);
    assert_eq!(planned["assessment"], status["assessment"]["accepted"]);
    let stored = read_json(&harness.file(&run_id, accepted["path"].as_str().unwrap()));
    let judgments: Vec<Value> = stored["policies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|policy| json!({"policy_id": policy["policy_id"], "judgment": policy["outcome"]}))
        .collect();
    assert_eq!(
        planned["run"]["final_assessment"]["evaluations"],
        json!(judgments)
    );

    let published = run(&harness, &["publish", "--effects", "review"], 0);
    for paths in [OUTCOME, EXECUTED, WRITES, EVALUATIONS] {
        assert_paths("completed", &published, paths);
    }
    assert_eq!(published["status"], "settled");
    assert_eq!(published["publication"]["status"], "completed");
    assert_eq!(published["settlement"]["outcome"], "published");
    assert_eq!(published["run"]["outcome"]["kind"], "published");
    assert_eq!(
        read_json(&harness.file(&run_id, "review-publication-result.json")),
        published["run"],
        "the saved result is stdout's run report"
    );

    let settled = run(&harness, &["publish", "--effects", "review"], 0);
    assert_paths(
        "already settled",
        &settled,
        &[
            (".status", "string"),
            (".settlement.outcome", "string"),
            (".repaired", "boolean"),
        ],
    );
    assert_eq!(settled["status"], "already_settled");
    assert!(
        settled.get("run").is_none() && settled.get("publication").is_none(),
        "{settled:#}"
    );
    assert_eq!(
        read_json(&harness.file(&run_id, "review-publication-result.json")),
        published["run"],
        "already_settled leaves the saved result unchanged"
    );
}

#[test]
fn guide_partial_denied_and_rejected_publication_paths_exist() {
    let partial = Harness::new();
    let run_id = partial.accepted_with_finding();
    let value = run(
        &partial,
        &["publish", "--effects", "create_summary,update_summary"],
        4,
    );
    for paths in [OUTCOME, EXECUTED, WRITES] {
        assert_paths("partial", &value, paths);
    }
    assert_eq!(value["status"], "unfinished");
    assert_eq!(value["publication"]["status"], "partial");
    assert_eq!(value["run"]["outcome"]["kind"], "published");
    assert_eq!(
        value["run"]["outcome"]["result"]["stop"][0]["code"],
        "authority_denied"
    );
    assert_eq!(
        value["run"]["outcome"]["result"]["skipped"][0]["kind"],
        "finding"
    );
    let abandoned = run(
        &partial,
        &[
            "publish",
            "--abandon",
            "--reason",
            "Close the denied round.",
        ],
        0,
    );
    assert_eq!(abandoned["status"], "settled", "{abandoned:#}");
    assert_eq!(
        read_json(&partial.file(&run_id, "review-publication-result.json")),
        abandoned["abandon"],
        "an abandonment saves its abandon document"
    );

    let finalization = Harness::new();
    let finalization_run = finalization.accepted_with_finding();
    let preview = finalization.file(&finalization_run, "rendered/review-summary.md");
    let accepted_preview = fs::read_to_string(&preview).unwrap();
    finalization
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response(
            "PUT",
            &format!("{MR_PATH}/notes/101"),
            Response::status(422),
        ));
    let value = run(&finalization, &["publish", "--effects", "review"], 7);
    assert_paths("finalization", &value, OUTCOME);
    assert_paths("finalization", &value, EXECUTED);
    assert_eq!(value["status"], "unfinished");
    assert_eq!(value["publication"]["status"], "partial");
    assert_eq!(value["run"]["outcome"]["result"]["stop"], json!([]));
    assert_eq!(
        value["run"]["outcome"]["result"]["finalization_stop"][0]["code"],
        "write_rejected"
    );
    // A recovery plan reads the published finding from GitLab; the preview keeps the accepted view.
    let replanned = run(
        &finalization,
        &["publish", "--effects", "review", "--plan"],
        0,
    );
    assert_paths(
        "recovery plan",
        &replanned,
        &[(".run.outcome.plan.summary_body", "string")],
    );
    assert_eq!(fs::read_to_string(&preview).unwrap(), accepted_preview);
    assert_ne!(
        replanned["run"]["outcome"]["plan"]["summary_body"], accepted_preview,
        "the online plan, not the preview, shows the current round"
    );

    let denied = Harness::new();
    denied.accepted_with_finding();
    let value = run(&denied, &["publish", "--effects", "finding"], 4);
    assert_paths("denied", &value, OUTCOME);
    assert_paths("denied", &value, EXECUTED);
    assert_eq!(value["status"], "unfinished");
    assert_eq!(value["publication"]["status"], "denied");
    assert_eq!(value["run"]["outcome"]["result"]["attempted"], json!([]));

    let rejected = Harness::new();
    rejected.ready_run(false);
    rejected
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
    let value = run(&rejected, &["publish", "--effects", "review"], 7);
    assert_paths("rejected write", &value, OUTCOME);
    assert_paths("rejected write", &value, EXECUTED);
    assert_eq!(value["status"], "unfinished");
    assert_eq!(value["publication"]["status"], "not_started");
    assert_eq!(
        value["run"]["outcome"]["result"]["stop"][0]["code"],
        "write_rejected"
    );

    // Recovery refused before execution: a partial round whose head then moved.
    let moved = Harness::new();
    moved.accepted_with_finding();
    run(
        &moved,
        &["publish", "--effects", "create_summary,update_summary"],
        4,
    );
    moved.push_commit();
    let value = run(&moved, &["publish", "--effects", "review"], 3);
    assert_paths("refused recovery", &value, OUTCOME);
    assert_paths(
        "refused recovery",
        &value,
        &[
            (".run.outcome.diagnostics[].code", "string"),
            (".diagnostics[].code", "string"),
            (".diagnostics[].next", "string"),
        ],
    );
    assert_eq!(value["status"], "unfinished");
    assert_eq!(value["publication"]["status"], "rejected");
    assert_eq!(value["run"]["outcome"]["kind"], "failed");
    assert_eq!(value["diagnostics"][0]["code"], "changed_revision");
}

/// Verified writes of a publication result that no attempt of the same result equals.
fn observed_only(result: &Value) -> Vec<&Value> {
    let attempted = result["attempted"].as_array().unwrap();
    result["verified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| &effect["write"])
        .filter(|write| !attempted.contains(write))
        .collect()
}

#[test]
fn guide_uncertain_refused_and_recovered_publication_paths_exist() {
    let harness = Harness::lost_responses();
    let owner = harness.accepted_with_finding();
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
    let uncertain = run(&harness, &["publish", "--effects", "review"], 6);
    assert_paths("uncertain", &uncertain, OUTCOME);
    assert_paths("uncertain", &uncertain, EXECUTED);
    assert_eq!(uncertain["status"], "unfinished");
    assert_eq!(uncertain["publication"]["status"], "uncertain");
    let result = &uncertain["run"]["outcome"]["result"];
    assert_eq!(result["stop"][0]["code"], "write_unknown");
    assert_eq!(result["attempted"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["verified"], json!([]));

    harness.ready_run(true);
    let refused = run(&harness, &["publish", "--effects", "review"], 5);
    assert_paths("refused", &refused, OUTER_ERROR);
    assert_paths(
        "refused",
        &refused,
        &[
            (".diagnostics[].owner_run_id", "string"),
            (".diagnostics[].next", "string"),
        ],
    );
    assert_eq!(
        refused["diagnostics"][0]["code"],
        "publication_pending_owner"
    );
    assert!(refused.get("command").is_none(), "{refused:#}");

    harness.ok(&["run", "select", &owner]);
    let resent = run(&harness, &["publish", "--effects", "review"], 0);
    assert_paths("resent", &resent, OUTCOME);
    assert_paths("resent", &resent, WRITES);
    assert_paths(
        "resent",
        &resent,
        &[(".run.outcome.result.retried_creations[].kind", "string")],
    );
    assert_eq!(resent["status"], "settled");
    assert_eq!(resent["recovery"], true);

    // The summary creation commits, but neither its response nor its readback arrives.
    let committed = Harness::lost_responses();
    committed.accepted_with_finding();
    {
        let mut state = committed.http.state.lock().unwrap();
        state
            .faults
            .push(Fault::lost(&format!("{MR_PATH}/notes"), true));
        state.faults.push(Fault::response(
            "GET",
            &format!("{MR_PATH}/notes/101"),
            Response::status(500),
        ));
    }
    let unread = run(&committed, &["publish", "--effects", "review"], 7);
    assert_paths("unread", &unread, OUTCOME);
    assert_paths("unread", &unread, EXECUTED);
    assert_eq!(unread["publication"]["status"], "uncertain");
    let result = &unread["run"]["outcome"]["result"];
    assert_eq!(result["stop"][0]["code"], "provider_unavailable");
    assert_eq!(result["verified"], json!([]));
    let recovered = run(&committed, &["publish", "--effects", "review"], 0);
    assert_paths("recovered", &recovered, OUTCOME);
    assert_paths("recovered", &recovered, WRITES);
    assert_eq!(recovered["status"], "settled");
    assert_eq!(recovered["recovery"], true);
    let observed = observed_only(&recovered["run"]["outcome"]["result"]);
    assert_eq!(
        observed
            .iter()
            .map(|write| &write["kind"])
            .collect::<Vec<_>>(),
        [&json!("summary")],
        "the earlier summary creation is re-observed, not sent again: {recovered:#}"
    );
}

#[test]
fn guide_readback_evidence_and_storage_failure_paths_exist() {
    let readback = Harness::new();
    readback.accepted_with_finding();
    readback
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response(
            "GET",
            &format!("{MR_PATH}/discussions/discussion-1"),
            Response::status(500),
        ));
    let value = run(&readback, &["publish", "--effects", "review"], 7);
    assert_paths("readback", &value, OUTCOME);
    assert_paths("readback", &value, EXECUTED);
    assert_paths("readback", &value, WRITES);
    assert_eq!(value["publication"]["status"], "uncertain");
    let result = &value["run"]["outcome"]["result"];
    assert_eq!(result["stop"][0]["code"], "provider_unavailable");
    let finding_write = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .any(|write| write["kind"] == "finding" || write["write"]["kind"] == "finding")
    };
    assert!(finding_write(&result["attempted"]), "{result:#}");
    assert!(!finding_write(&result["verified"]), "{result:#}");

    let evidence = Harness::new();
    let run_id = evidence.accepted_with_finding();
    let (ready, paused) = sync_channel(0);
    let (release, released) = sync_channel(0);
    evidence
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::pause(
            &format!("{MR_PATH}/discussions/discussion-1"),
            ready,
            released,
        ));
    let run_dir = evidence.run_dir(&run_id);
    let (output, value) = std::thread::scope(|scope| {
        let publish = scope.spawn(|| evidence.rkc(&["publish", "--effects", "review"]));
        paused.recv().unwrap();
        // The finding is dispatched; its readback and every later run file cannot be saved.
        fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o555)).unwrap();
        release.send(()).unwrap();
        publish.join().unwrap()
    });
    fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(output.status.code(), Some(7), "{value:#}");
    assert_paths("evidence", &value, OUTCOME);
    assert_paths("evidence", &value, EXECUTED);
    assert_paths(
        "evidence",
        &value,
        &[
            (".run.evidence_failure.code", "string"),
            (".storage_failures[].code", "string"),
        ],
    );
    assert_eq!(value["publication"]["status"], "uncertain");
    assert_eq!(
        value["run"]["outcome"]["result"]["stop"][0]["code"],
        "evidence_unavailable"
    );
    assert_eq!(
        value["run"]["evidence_failure"]["code"],
        "evidence_unavailable"
    );
    assert_eq!(value["storage_failures"][0]["code"], "result_storage");
    assert!(!run_dir.join("review-publication-result.json").exists());

    let storage = Harness::new();
    let run_id = storage.accepted_with_finding();
    let result = storage.file(&run_id, "review-publication-result.json");
    fs::create_dir_all(result.join("occupied")).unwrap();
    let value = run(&storage, &["publish", "--effects", "review"], 7);
    assert_paths("storage", &value, OUTCOME);
    assert_paths("storage", &value, EXECUTED);
    assert_eq!(value["status"], "settled");
    assert_eq!(value["publication"]["status"], "completed");
    assert_eq!(value["storage_failures"][0]["code"], "result_storage");
}
