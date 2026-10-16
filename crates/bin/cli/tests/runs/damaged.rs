use std::fs;

use serde_json::json;

use super::harness::{ERROR_POLICY, Harness, read_json, write_json};

#[test]
fn stop_and_status_work_with_a_missing_or_undecodable_retained_file() {
    for (file, damage) in [
        ("review-conversation.json", None),
        ("review-context.json", Some(&b"{not json"[..])),
        ("reviewed-source.json", Some(&b"[]"[..])),
    ] {
        let harness = Harness::new();
        let run_id = harness.start();
        let path = harness.file(&run_id, file);
        match damage {
            None => fs::remove_file(&path).unwrap(),
            Some(bytes) => fs::write(&path, bytes).unwrap(),
        }
        harness.fails(&["accept"], 2, "hash_mismatch");

        let status = harness.ok(&["status"]);
        assert_eq!(status["status"], "open", "{file}");
        assert_eq!(status["retained_files"]["state"], "damaged", "{status:#}");
        assert_eq!(
            status["retained_files"]["files"],
            json!([file]),
            "{status:#}"
        );
        assert!(
            status["next"].as_str().unwrap().contains("rkc run stop"),
            "{status:#}"
        );

        let stopped = harness.ok(&["run", "stop", "--reason", "The capture is damaged."]);
        assert_eq!(stopped["status"], "stopped", "{file}");
        assert_eq!(harness.manifest(&run_id)["status"], "stopped");
        assert_eq!(harness.writes(), 0);
    }
}

#[test]
fn checking_a_terminal_run_reports_diagnostics_without_writing_files() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    harness.ok(&["accept"]);
    harness.ok(&["run", "finish"]);
    let files = [
        "rendered/review-summary.md",
        "rendered/terminal-review-summary.md",
        "review-check-result.json",
    ];
    let before = files.map(|name| fs::read(harness.file(&run_id, name)).unwrap());
    let checked = harness.ok(&["check"]);
    assert_eq!(checked["status"], "valid", "{checked:#}");
    assert_eq!(checked["recorded"], false);
    assert_eq!(checked["previews"], json!([]));
    let draft = harness.file(&run_id, "review-assessment.draft.json");
    let mut edited = read_json(&draft);
    edited["scope"] = json!("Edited after completion.");
    write_json(&draft, &edited);
    let invalid = harness.fails(&["check"], 2, "stale_successor");
    assert_eq!(invalid["recorded"], false);
    harness.fails(&["check"], 2, "stale_successor");

    let after = files.map(|name| fs::read(harness.file(&run_id, name)).unwrap());
    assert_eq!(after, before, "previews and the check result are unchanged");
    assert_eq!(harness.manifest(&run_id)["completion"], "local_only");
}

#[test]
fn a_symlinked_publication_events_file_is_refused_and_nothing_is_written_through_it() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review", "--plan"]);
    let events = harness.file(&run_id, "review-publication-events.json");
    let outside = harness.root.path().join("outside-events.json");
    fs::copy(&events, &outside).unwrap();
    fs::remove_file(&events).unwrap();
    std::os::unix::fs::symlink(&outside, &events).unwrap();
    let target = fs::read(&outside).unwrap();
    let result = fs::read(harness.file(&run_id, "review-publication-result.json")).unwrap();

    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        harness.fails(&args, 2, "runtime_redirected");
    }
    assert!(fs::symlink_metadata(&events).unwrap().is_symlink());
    assert_eq!(
        fs::read(&outside).unwrap(),
        target,
        "the link target is unchanged"
    );
    assert_eq!(
        fs::read(harness.file(&run_id, "review-publication-result.json")).unwrap(),
        result
    );
    assert_eq!(harness.writes(), 0);
    assert!(harness.journal_dir().is_none(), "no journal was created");
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);
}

#[test]
fn publication_needs_a_recorded_preview_profile() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    let profile = harness.file(&run_id, "review-render-profile.json");
    fs::remove_file(&profile).unwrap();
    let requests = harness.http.state.lock().unwrap().requests.len();
    let refused = harness.fails(
        &["publish", "--effects", "review"],
        2,
        "render_profile_changed",
    );
    assert!(
        refused["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("rkc publish --plan"),
        "{refused:#}"
    );
    assert_eq!(harness.http.state.lock().unwrap().requests.len(), requests);
    harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert!(profile.is_file(), "a successful plan records the profile");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
}

#[test]
fn status_reports_a_missing_accepted_assessment_file_or_directory() {
    for whole_directory in [true, false] {
        let harness = Harness::new();
        let run_id = harness.accepted_with_finding();
        let reference = harness.manifest(&run_id)["review_assessment"]
            .as_str()
            .unwrap()
            .to_owned();
        let directory = harness
            .run_dir(&run_id)
            .join("accepted")
            .join(&reference[7..]);
        if whole_directory {
            fs::remove_dir_all(&directory).unwrap();
        } else {
            fs::remove_file(directory.join("review-assessment.json")).unwrap();
        }
        let status = harness.ok(&["status"]);
        assert_eq!(status["assessment"]["state"], "missing_snapshot");
        assert!(status["next"].as_str().unwrap().contains("restore"));
        let listed = harness.ok(&["run", "list"]);
        assert_eq!(listed["runs"][0]["assessment"], "missing_snapshot");
        harness.fails(
            &["publish", "--effects", "review", "--plan"],
            2,
            "accepted_snapshot_missing",
        );
    }
}
