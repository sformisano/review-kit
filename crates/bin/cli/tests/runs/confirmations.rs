use std::fs;

use serde_json::{Value, json};

use super::harness::{ERROR_POLICY, Harness, PROOF_POLICY, read_json, ready};

/// Write exact draft bytes without the fixture's simulated checker confirmations.
fn write_exact(path: &std::path::Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn checker(finding: &Value) -> Value {
    json!({
        "source": "checker",
        "finding": finding["id"],
        "result": "confirmed",
        "checked": finding,
        "basis": "An isolated checker reproduced the defect against the reviewed source.",
    })
}

/// A follow-up run whose published blocking finding F1 is carried, with a complete
/// changes-required draft that leaves F1's decision to the caller.
fn follow_up(harness: &Harness) -> (String, Value, Value) {
    follow_up_after(harness, |_| {})
}

/// [`follow_up`] with `between` applied to the provider after the first publication.
fn follow_up_after(harness: &Harness, between: impl FnOnce(&Harness)) -> (String, Value, Value) {
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    between(harness);
    let started = harness.ok(&["run", "start"]);
    assert_eq!(started["carried_findings"], 1, "{started:#}");
    let run_id = started["run_id"].as_str().unwrap().to_owned();
    harness.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
    let mut draft = read_json(&harness.file(&run_id, "review-assessment.draft.json"));
    ready(&mut draft);
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind":"violated","evidence":"The failure path still discards the actionable error."});
        }
    }
    let captured = read_json(&harness.file(&run_id, "review-context.json"))["snapshot"]["findings"]
        [0]["finding"]
        .clone();
    assert_eq!(captured["validated"], true, "{captured:#}");
    (run_id, draft, captured)
}

fn retained() -> Value {
    json!({
        "source": "retained",
        "finding": 1,
        "basis": "The confirmation recorded for this unchanged version still applies.",
    })
}

#[test]
fn an_unchanged_carried_blocker_needs_a_current_or_retained_confirmation() {
    let harness = Harness::new();
    let (run_id, mut draft, captured) = follow_up(&harness);
    assert_eq!(draft["findings"][0]["reusable_confirmation"], true);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    draft["findings"][0]["reassessment"] = json!("unchanged");
    draft["confirmations"] = json!([]);
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "incomplete_decision");

    draft["confirmations"] = json!([checker(&captured)]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["check"])["status"], "valid");

    draft["confirmations"] = json!([retained()]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
    let accepted = harness.manifest(&run_id)["review_assessment"]
        .as_str()
        .unwrap()
        .to_owned();
    let snapshot = read_json(&harness.file(
        &run_id,
        &format!("accepted/{}/review-assessment.json", &accepted[7..]),
    ));
    assert_eq!(snapshot["findings"][0]["reassessment"], "unchanged");
    let plan = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        plan["effects"]["actions"],
        json!([]),
        "an unchanged confirmed finding keeps its existing thread: {plan:#}"
    );
}

#[test]
fn only_a_confirmation_recorded_under_enforcement_at_this_revision_is_reusable() {
    for between in [
        Harness::forget_confirmation_records as fn(&Harness) -> usize,
        |harness: &Harness| {
            harness.push_commit();
            1
        },
    ] {
        let harness = Harness::new();
        let (run_id, mut draft, captured) =
            follow_up_after(&harness, |harness| assert!(between(harness) > 0));
        assert_eq!(draft["findings"][0]["reusable_confirmation"], false);
        let path = harness.file(&run_id, "review-assessment.draft.json");
        draft["findings"][0]["reassessment"] = json!("unchanged");
        draft["confirmations"] = json!([retained()]);
        write_exact(&path, &draft);
        harness.fails(&["accept"], 2, "invalid_retained_confirmation");

        draft["findings"][0]["reusable_confirmation"] = json!(true);
        write_exact(&path, &draft);
        harness.fails(&["accept"], 2, "changed_reusable_confirmation");

        draft["findings"][0]["reusable_confirmation"] = json!(false);
        draft["confirmations"] = json!([checker(&captured)]);
        write_exact(&path, &draft);
        assert_eq!(harness.ok(&["accept"])["status"], "stored");
    }
}

#[test]
fn a_blocking_validation_claim_needs_an_exact_checker_confirmation() {
    let harness = Harness::new();
    let (run_id, mut draft, captured) = follow_up(&harness);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut current = captured.clone();
    current["evidence"] = json!("The current source still discards the cause.");
    draft["findings"][0]["reassessment"] = current.clone();
    draft["confirmations"] = json!([]);
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "unconfirmed_blocking_finding");

    draft["confirmations"] = json!([checker(&captured)]);
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "stale_confirmation");

    draft["confirmations"] = json!([checker(&current)]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
}

#[test]
fn private_context_beyond_the_limit_is_rejected_before_acceptance() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    draft["local_context"] = json!("x".repeat(2001));
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "local_context_too_long");
    draft["local_context"] = json!("Primary result: /tmp/review-results/primary.json");
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
}

#[test]
fn an_unchanged_fixed_finding_keeps_its_validation_without_a_new_check() {
    let harness = Harness::new();
    let (run_id, mut draft, captured) = follow_up(&harness);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let explanation = "The failure branch now returns the cause.";
    let mut fixed = captured.clone();
    fixed["status"] = json!("fixed");
    fixed["evidence"] = json!(explanation);
    ready(&mut draft);
    draft["findings"][0]["reassessment"] = fixed;
    draft["transitions"] =
        json!([{"finding": 1, "from": "open", "to": "fixed", "body": explanation}]);
    draft["confirmations"] = json!([]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
    harness.ok(&["publish", "--effects", "review"]);

    let started = harness.ok(&["run", "start"]);
    assert_eq!(started["carried_findings"], 1, "{started:#}");
    let third = started["run_id"].as_str().unwrap().to_owned();
    harness.declare_policies(&third, &[ERROR_POLICY, PROOF_POLICY]);
    let path = harness.file(&third, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    draft["findings"][0]["reassessment"] = json!("unchanged");
    draft["confirmations"] = json!([]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
    let plan = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(plan["effects"]["actions"], json!([]), "{plan:#}");
}

#[test]
fn an_update_reply_needs_a_changed_finding() {
    let harness = Harness::new();
    let (run_id, mut draft, _) = follow_up(&harness);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    draft["findings"][0]["reassessment"] = json!("unchanged");
    draft["confirmations"] = json!([retained()]);
    draft["transitions"] = json!([{
        "finding": 1, "from": "open", "to": "open",
        "body": "The finding still applies.",
    }]);
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "invalid_transition");
    draft["transitions"] = json!([]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
}

#[test]
fn an_unconfirmed_carried_blocker_needs_a_withheld_same_state_transition() {
    let harness = Harness::new();
    let (run_id, mut draft, _) = follow_up_after(&harness, |harness| harness.push_commit());
    let path = harness.file(&run_id, "review-assessment.draft.json");
    draft["findings"][0]["reassessment"] = json!("unchanged");
    draft["confirmations"] = json!([]);
    draft["decision"] = json!("blocked");
    draft["limitation"] = json!("No independent check of F1 is available at this revision.");
    write_exact(&path, &draft);
    harness.fails(&["accept"], 2, "missing_transition");

    draft["transitions"] = json!([{
        "finding": 1, "from": "open", "to": "open",
        "body": "F1 awaits an independent check at this revision.",
    }]);
    write_exact(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["status"], "stored");
    let replies = harness.writes_to("POST", "/notes");
    harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(
        harness.writes_to("POST", "/notes"),
        replies,
        "a blocked review withholds the unconfirmed finding's reply"
    );
}
