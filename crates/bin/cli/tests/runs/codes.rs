use std::fs;

use serde_json::json;

use super::{
    harness::{ERROR_POLICY, Harness, copy_dir, read_json, sha256, write_json},
    server::{Fault, MR_PATH},
};

fn diagnostic_codes(value: &serde_json::Value) -> Vec<String> {
    value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["code"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn incomplete_or_missing_local_state_has_stable_codes_and_repairs() {
    let harness = Harness::new();
    let run_id = harness.start();
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        2,
        "assessment_not_accepted",
    );
    harness.fails(&["run", "finish"], 2, "assessment_not_accepted");
    harness.fails(
        &["publish", "--abandon", "--reason", "Nothing to close."],
        5,
        "no_unfinished_publication",
    );

    let draft = harness.file(&run_id, "review-assessment.draft.json");
    fs::remove_file(&draft).unwrap();
    harness.fails(&["accept"], 2, "missing_draft");
    let prepared = harness.ok(&["revise", "--reason", "Prepare the missing draft."]);
    assert_eq!(prepared["archived_draft"], json!(null));
    harness.declare_policies(&run_id, &[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    harness.ok(&["accept"]);
    let reference = harness.manifest(&run_id)["review_assessment"]
        .as_str()
        .unwrap()
        .to_owned();
    let accepted = harness
        .run_dir(&run_id)
        .join("accepted")
        .join(&reference[7..]);
    let moved = harness.root.path().join("moved-snapshot");
    fs::rename(&accepted, &moved).unwrap();
    harness.fails(&["accept"], 2, "accepted_snapshot_missing");
    fs::rename(&moved, &accepted).unwrap();
    let manifest = harness.file(&run_id, "review-run.json");
    let saved = fs::read(&manifest).unwrap();
    fs::write(&manifest, b"{").unwrap();
    harness.fails(&["status"], 2, "invalid_run_document");
    fs::write(&manifest, saved).unwrap();
    harness.ok(&["status"]);
}

#[test]
fn retained_files_copied_from_another_revision_fail_identity_checks() {
    let harness = Harness::new();
    let first = harness.start();
    harness.push_commit();
    let second = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.ok(&["run", "select", &first]);
    let context = harness.file(&first, "review-context.json");
    let original = fs::read(&context).unwrap();
    let copied = fs::read(harness.file(&second, "review-context.json")).unwrap();
    fs::write(&context, &copied).unwrap();
    harness.fails(&["accept"], 2, "hash_mismatch");
    let manifest_path = harness.file(&first, "review-run.json");
    let manifest = read_json(&manifest_path);
    let mut forged = manifest.clone();
    forged["snapshot_files"]["review-context.json"] = json!(sha256(&copied));
    write_json(&manifest_path, &forged);
    let mismatch = harness.fails(&["accept"], 2, "identity_mismatch");
    assert_eq!(mismatch["diagnostics"][0]["file"], "review-context.json");
    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "damaged", "{status:#}");
    assert_eq!(
        status["retained_files"]["files"],
        json!(["review-context.json"])
    );
    assert_eq!(
        status["retained_files"]["problems"][0]["code"],
        "identity_mismatch"
    );
    fs::write(&context, original).unwrap();
    write_json(&manifest_path, &manifest);
    harness.ok(&["status"]);
}

#[test]
fn render_profile_changes_after_acceptance_need_a_fresh_plan() {
    let harness = Harness::new();
    let profile = harness.root.path().join("profile");
    harness.ok(&[
        "render-profile",
        "export",
        "--dir",
        profile.to_str().unwrap(),
    ]);
    let mut config = read_json(&harness.config);
    config["render_profile"] = json!(profile.join("profile.json"));
    write_json(&harness.config, &config);
    harness.accepted_with_finding();
    let manifest = read_json(&profile.join("profile.json"));
    let template = profile.join(
        manifest["artifacts"]["finding"]["template"]
            .as_str()
            .unwrap(),
    );
    let body = fs::read_to_string(&template).unwrap();
    fs::write(&template, format!("REVISED PREVIEW\n{body}")).unwrap();
    let requests = harness.http.state.lock().unwrap().requests.len();
    harness.fails(
        &["publish", "--effects", "review"],
        2,
        "render_profile_changed",
    );
    assert_eq!(harness.http.state.lock().unwrap().requests.len(), requests);
    harness.ok(&["publish", "--effects", "review", "--plan"]);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let finding = harness.http.state.lock().unwrap().discussions[0]["notes"][0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(finding.starts_with("REVISED PREVIEW\n"), "{finding}");
}

#[test]
fn a_restored_run_with_other_accepted_inputs_cannot_recover_the_journal_binding() {
    let harness = Harness::lost_responses();
    let run_id = harness.accepted_with_finding();
    let backup = harness.root.path().join("run-backup");
    copy_dir(&harness.run_dir(&run_id), &backup);
    harness.ok(&["revise", "--reason", "Tighten the evidence."]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["new_findings"][0]["validated"] = json!(true);
    draft["new_findings"][0]["evidence"] = json!("The failure branch drops the cause.");
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        policy["outcome"] = if policy["policy_id"] == ERROR_POLICY {
            json!({"kind":"violated","evidence":"The failure path discards the actionable error."})
        } else {
            json!({"kind":"satisfied"})
        };
    }
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
    let (output, _) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6));
    fs::remove_dir_all(harness.run_dir(&run_id)).unwrap();
    copy_dir(&backup, &harness.run_dir(&run_id));
    harness.fails(&["run", "select", &run_id], 5, "binding_mismatch");
    harness.fails(&["publish", "--effects", "review"], 5, "binding_mismatch");
    harness.fails(&["accept"], 5, "publication_pending_owner");
}

#[test]
fn a_follow_up_must_inventory_active_finding_policies_and_can_resolve_them() {
    let harness = Harness::new();
    let first = harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let second = harness.ok(&["run", "start"]);
    assert_eq!(second["carried_findings"], 1);
    let second = second["run_id"].as_str().unwrap().to_owned();
    assert_ne!(second, first);
    harness.declare_policies(&second, &[super::harness::PROOF_POLICY]);
    let missing = harness.rkc(&["accept"]).1;
    assert_eq!(missing["status"], "rejected");
    assert!(
        diagnostic_codes(&missing).contains(&"missing_active_finding_policy".to_owned()),
        "{missing}"
    );
    let path = harness.file(&second, "review-assessment.draft.json");
    let mut inventory = read_json(&path);
    inventory["policies"].as_array_mut().unwrap().push(json!({
        "policy_id": ERROR_POLICY,
        "name": format!("Policy {ERROR_POLICY}"),
        "required": false,
        "reason": "The active finding is reassessed under its original policy.",
        "outcome": null,
    }));
    write_json(&path, &inventory);

    let path = harness.file(&second, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    assert_eq!(draft["findings"][0]["id"], 1);
    assert!(draft["findings"][0].get("previous").is_none());
    super::harness::ready(&mut draft);
    let mut fixed = draft["findings"][0]["reassessment"].clone();
    fixed["status"] = json!("fixed");
    fixed["evidence"] = json!("The failure branch now returns the cause.");
    draft["findings"][0]["reassessment"] = fixed;
    draft["transitions"] = json!([{"finding":1,"from":"open","to":"fixed",
        "body":"The failure path now keeps its cause; the regression passes."}]);
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    let discussions = harness.discussions();
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.discussions(), discussions, "no new finding thread");
    assert!(
        harness.writes_to("PUT", "/discussions/discussion-1") >= 1,
        "the thread is resolved"
    );
    let journal = harness.journal();
    assert_eq!(journal["settled_runs"].as_array().unwrap().len(), 2);
}
