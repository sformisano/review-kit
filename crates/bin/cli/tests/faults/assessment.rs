use super::{
    crash,
    harness::{ERROR_POLICY, Harness, read_json, sha256, write_json},
};
use serde_json::json;
use std::fs;

#[test]
fn acceptance_crashes_preserve_exact_bytes_and_retry_the_single_reference() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let draft = fs::read(&path).unwrap();
    let reference = sha256(&draft);
    let snapshot = harness.file(
        &run_id,
        &format!("accepted/{}/review-assessment.json", &reference[7..]),
    );
    crash(&harness, "accept.before_install", &["accept"]);
    assert!(!snapshot.exists());
    assert!(harness.manifest(&run_id)["review_assessment"].is_null());
    crash(&harness, "accept.after_install", &["accept"]);
    assert_eq!(fs::read(&snapshot).unwrap(), draft);
    assert!(harness.manifest(&run_id)["review_assessment"].is_null());
    assert_eq!(harness.ok(&["status"])["assessment"]["state"], "draft");
    crash(&harness, "accept.after_manifest", &["accept"]);
    assert_eq!(harness.manifest(&run_id)["review_assessment"], reference);
    assert_eq!(harness.ok(&["accept"])["status"], "existing");
    assert_eq!(fs::read(&path).unwrap(), draft);
    assert_eq!(fs::read(&snapshot).unwrap(), draft);
    assert_eq!(harness.writes(), 0);
}

#[test]
fn acceptance_crash_after_manifest_recreates_previews_on_retry() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    crash(&harness, "accept.after_manifest", &["accept"]);
    assert!(harness.manifest(&run_id)["review_assessment"].is_string());
    assert!(!harness.file(&run_id, "review-render-profile.json").exists());
    assert_eq!(harness.ok(&["accept"])["status"], "existing");
    assert!(
        harness
            .file(&run_id, "review-render-profile.json")
            .is_file()
    );
    assert!(
        harness
            .file(&run_id, "rendered/review-summary.md")
            .is_file()
    );
}

#[test]
fn revision_crash_after_archive_preserves_edited_draft_until_retry() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let accepted = harness.manifest(&run_id)["review_assessment"].clone();
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut edited = read_json(&path);
    edited["policy_set"] = json!("Edited review policies");
    write_json(&path, &edited);
    let bytes = fs::read(&path).unwrap();
    let archive = harness.file(
        &run_id,
        &format!(
            "draft-history/{}/review-assessment.draft.json",
            &sha256(&bytes)[7..]
        ),
    );
    crash(
        &harness,
        "revise.after_archive",
        &["revise", "--reason", "Rename the policy set."],
    );
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let revised = harness.ok(&["revise", "--reason", "Rename the policy set."]);
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    let draft = read_json(&path);
    assert_eq!(draft["supersedes"], accepted);
    assert_eq!(draft["reason"], "Rename the policy set.");
    assert_eq!(draft["policy_set"], edited["policy_set"]);
    assert!(draft["decision"].is_null());
    assert!(draft["policies"][0]["outcome"].is_null());
    assert!(
        revised["archived_draft"]
            .as_str()
            .unwrap()
            .ends_with("review-assessment.draft.json")
    );
}

#[test]
fn run_start_crash_before_selection_keeps_the_previous_selection() {
    let harness = Harness::new();
    crash(
        &harness,
        "run_start.after_install",
        &["run", "start", "--mr", &harness.mr()],
    );
    assert!(!harness.runtime().join("review-workspace.json").exists());
    harness.fails(&["status"], 2, "no_selected_run");
    let listed = harness.ok(&["run", "list"]);
    let installed = listed["runs"][0]["run_id"].as_str().unwrap().to_owned();
    assert_eq!(listed["runs"][0]["selected"], false);
    let refused = harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        5,
        "open_run_exists",
    );
    assert_eq!(refused["diagnostics"][0]["run_ids"], json!([installed]));
    harness.ok(&["run", "select", &installed]);
    assert_eq!(harness.selected(), installed);

    crash(
        &harness,
        "run_start.after_install",
        &["run", "start", "--new"],
    );
    assert_eq!(
        harness.selected(),
        installed,
        "the previous selection is kept"
    );
    let listed = harness.ok(&["run", "list"]);
    assert_eq!(listed["runs"].as_array().unwrap().len(), 2);
    assert_eq!(listed["incomplete_staging"], json!([]));
    let other = listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["run_id"] != installed)
        .unwrap()["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.ok(&["run", "select", &other]);
    assert_eq!(harness.ok(&["status"])["assessment"]["state"], "draft");
}
