use std::fs;

use serde_json::json;

use super::harness::{ERROR_POLICY, Harness, PROOF_POLICY, read_json, sha256, write_json};

#[test]
fn edited_assessment_blocks_planning_and_completion_until_restored() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let accepted = fs::read(&path).unwrap();
    let mut edited = read_json(&path);
    edited["reason"] = json!("An unaccepted edit.");
    write_json(&path, &edited);
    let dirty = harness.fails(
        &["publish", "--effects", "review", "--plan"],
        2,
        "dirty_draft",
    );
    assert_eq!(
        dirty["diagnostics"][0]["draft"],
        "review-assessment.draft.json"
    );
    harness.fails(&["run", "finish"], 2, "dirty_draft");
    assert_eq!(harness.ok(&["status"])["assessment"]["state"], "dirty");
    fs::write(&path, accepted).unwrap();
    harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(harness.writes(), 0);
}

#[test]
fn revision_preserves_declarations_and_archives_bytes_but_resets_readiness() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    let previous = harness.manifest(&run_id)["review_assessment"].clone();
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let before = fs::read(&path).unwrap();
    let original = read_json(&path);
    let revised = harness.ok(&["revise", "--reason", "Reassess the selected policies."]);
    assert_eq!(revised["supersedes"], previous);
    assert_eq!(
        fs::read(harness.file(&run_id, revised["archived_draft"].as_str().unwrap())).unwrap(),
        before
    );
    assert!(
        revised["reset"]
            .as_array()
            .unwrap()
            .contains(&json!("confirmations")),
        "{revised:#}"
    );
    assert_eq!(original["confirmations"].as_array().unwrap().len(), 1);
    let draft = read_json(&path);
    assert_eq!(draft["confirmations"], json!([]));
    assert_eq!(draft["supersedes"], previous);
    assert_eq!(draft["decision"], json!(null));
    assert_eq!(draft["new_findings"][0]["validated"], false);
    assert_eq!(
        draft["new_findings"][0]["title"],
        original["new_findings"][0]["title"]
    );
    assert_eq!(draft["local_context"], original["local_context"]);
    assert!(
        draft["policies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["outcome"].is_null())
    );
    assert_eq!(harness.manifest(&run_id)["review_assessment"], previous);
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        2,
        "dirty_draft",
    );
    harness.fill_finding(&run_id);
    let accepted = harness.ok(&["accept"]);
    assert_eq!(accepted["status"], "stored");
    assert_ne!(accepted["reference"], previous);
    assert_eq!(harness.ok(&["accept"])["status"], "existing");
    harness.ok(&["publish", "--effects", "review", "--plan"]);
}

#[test]
fn malformed_draft_requires_regeneration_and_retains_the_exact_invalid_bytes() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    fs::write(&path, b"{ broken").unwrap();
    let malformed = harness.fails(&["revise", "--reason", "Repair it."], 2, "malformed_draft");
    assert_eq!(malformed["status"], "malformed_draft");
    assert_eq!(fs::read(&path).unwrap(), b"{ broken");
    assert!(!harness.run_dir(&run_id).join("draft-history").exists());
    let regenerated = harness.ok(&[
        "revise",
        "--reason",
        "Recover the assessment.",
        "--regenerate",
    ]);
    let archived = harness.file(&run_id, regenerated["archived_draft"].as_str().unwrap());
    assert_eq!(fs::read(archived).unwrap(), b"{ broken");
    assert!(
        regenerated["archived_draft"]
            .as_str()
            .unwrap()
            .contains(&sha256(b"{ broken")[7..])
    );
    let draft = read_json(&path);
    assert_eq!(draft["reason"], "Recover the assessment.");
    assert_eq!(draft["policies"][0]["policy_id"], ERROR_POLICY);
    assert!(draft["policies"][0]["outcome"].is_null());
    harness.fill_ready(&run_id);
    harness.ok(&["accept"]);
}

#[test]
fn copied_assessment_or_manifest_cannot_rebind_a_different_run() {
    let harness = Harness::new();
    let first = harness.ready_run(false);
    let second = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::copy(
        harness.file(&first, "review-assessment.draft.json"),
        harness.file(&second, "review-assessment.draft.json"),
    )
    .unwrap();
    let rejected = harness.rkc(&["accept"]).1;
    assert!(
        rejected["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "run_binding")
    );
    assert!(harness.manifest(&second)["review_assessment"].is_null());
    let manifest = harness.file(&second, "review-run.json");
    let original = fs::read(&manifest).unwrap();
    fs::copy(harness.file(&first, "review-run.json"), &manifest).unwrap();
    harness.fails(&["status"], 2, "run_identity_mismatch");
    harness.fails(&["accept"], 2, "run_identity_mismatch");
    fs::write(manifest, original).unwrap();
    harness.ok(&["status"]);
}

#[test]
fn captured_and_accepted_bytes_are_verified_before_publication_or_selection() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let context = harness.file(&run_id, "review-context.json");
    let original = fs::read(&context).unwrap();
    let mut changed = original.clone();
    changed.push(b'\n');
    fs::write(&context, changed).unwrap();
    harness.fails(&["accept"], 2, "hash_mismatch");
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        2,
        "hash_mismatch",
    );
    fs::write(context, original).unwrap();
    let reference = harness.manifest(&run_id)["review_assessment"]
        .as_str()
        .unwrap()
        .to_owned();
    let snapshot = harness.file(
        &run_id,
        &format!("accepted/{}/review-assessment.json", &reference[7..]),
    );
    let accepted = fs::read(&snapshot).unwrap();
    fs::write(&snapshot, b"{}").unwrap();
    harness.fails(&["accept"], 2, "hash_mismatch");
    harness.fails(&["run", "select", &run_id], 2, "hash_mismatch");
    fs::write(snapshot, accepted).unwrap();
    harness.ok(&["accept"]);
}

#[test]
fn required_unresolved_and_excluded_outcomes_keep_their_selection_constraints() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY]);
    harness.fill_ready(&run_id);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["policies"][1]["outcome"] = json!({"kind":"excluded"});
    write_json(&path, &draft);
    assert!(!harness.rkc(&["accept"]).0.status.success());
    draft["policies"][1]["outcome"] =
        json!({"kind":"unresolved","limitation":"Required definition is unavailable."});
    draft["decision"] = json!("blocked");
    draft["limitation"] = json!("Required definition is unavailable.");
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    harness.ok(&[
        "revise",
        "--reason",
        "Check the unavailable definition again.",
    ]);
    let revised = read_json(&path);
    assert_eq!(
        revised["policies"][1]["outcome"],
        draft["policies"][1]["outcome"]
    );
    assert!(revised["policies"][0]["outcome"].is_null());
    assert!(revised["decision"].is_null());
}
