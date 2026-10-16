use std::fs;

use serde_json::json;

use super::harness::{ERROR_POLICY, Harness, copy_dir, read_json, write_json};

#[test]
fn status_names_a_command_that_repairs_a_settled_runs_open_manifest() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let path = harness.file(&run_id, "review-run.json");
    let open = fs::read(&path).unwrap();
    harness.ok(&["publish", "--effects", "review"]);
    fs::write(&path, &open).unwrap();

    let writes = harness.writes();
    let status = harness.ok(&["status"]);
    assert_eq!(status["status"], "open");
    assert_eq!(status["journal"]["settlement"]["outcome"], "published");
    assert!(
        status["next"]
            .as_str()
            .unwrap()
            .contains("rkc publish --effects review"),
        "{status:#}"
    );
    assert_eq!(fs::read(&path).unwrap(), open, "status is read-only");

    let planned = harness.ok(&["publish"]);
    assert_eq!(planned["status"], "already_settled");
    assert_eq!(fs::read(&path).unwrap(), open, "planning never repairs");
    let repaired = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(repaired["status"], "already_settled");
    assert_eq!(repaired["repaired"], true);
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
    assert_eq!(harness.writes(), writes, "repair makes no provider writes");
}

/// Revise and accept a second assessment of the selected run.
fn accept_revised_assessment(harness: &Harness, run_id: &str) {
    harness.ok(&["revise", "--reason", "Tighten the evidence."]);
    let path = harness.file(run_id, "review-assessment.draft.json");
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
}

#[test]
fn a_restored_run_whose_inputs_differ_from_its_settlement_is_never_relabelled() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    let backup = harness.root.path().join("run-backup");
    copy_dir(&harness.run_dir(&run_id), &backup);
    accept_revised_assessment(&harness, &run_id);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let settled = harness.journal()["settled_runs"][0]["review_assessment"].clone();

    fs::remove_dir_all(harness.run_dir(&run_id)).unwrap();
    copy_dir(&backup, &harness.run_dir(&run_id));
    let manifest = harness.file(&run_id, "review-run.json");
    let restored = fs::read(&manifest).unwrap();
    assert_ne!(read_json(&manifest)["review_assessment"], settled);
    let journal = fs::read(
        harness
            .journal_dir()
            .unwrap()
            .join("publication-progress.json"),
    )
    .unwrap();
    let writes = harness.writes();
    for args in [
        vec!["publish", "--effects", "review"],
        vec!["publish", "--effects", "review", "--plan"],
        vec![
            "publish",
            "--abandon",
            "--reason",
            "Close the restored copy.",
        ],
        vec!["accept"],
        vec!["revise", "--reason", "Reconsider."],
        vec!["run", "finish"],
        vec!["run", "stop", "--reason", "Restored by mistake."],
    ] {
        harness.fails(&args, 5, "binding_mismatch");
        assert_eq!(fs::read(&manifest).unwrap(), restored, "{args:?}");
    }
    harness.fails(&["run", "select", &run_id], 5, "binding_mismatch");
    let status = harness.ok(&["status"]);
    assert_eq!(status["status"], "open", "status never repairs");
    let next = status["next"].as_str().unwrap();
    assert!(next.contains("different accepted inputs"), "{next}");
    assert!(next.contains("restore the exact run directory"), "{next}");
    assert!(!next.contains("rkc publish"), "{next}");
    assert_eq!(harness.writes(), writes);
    assert_eq!(
        fs::read(
            harness
                .journal_dir()
                .unwrap()
                .join("publication-progress.json")
        )
        .unwrap(),
        journal
    );
}
