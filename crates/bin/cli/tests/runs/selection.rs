use std::{fs, sync::mpsc::sync_channel};

use serde_json::json;

use super::{
    harness::{Harness, read_json, write_json},
    server::Fault,
};

#[test]
fn runs_with_unaccepted_edits_remain_selectable_and_report_dirty_state() {
    let harness = Harness::new();
    let dirty = harness.ready_run(false);
    harness.ok(&["revise", "--reason", "Reconsider the evidence."]);
    let other = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(harness.selected(), other);
    let selected = harness.ok(&["run", "select", &dirty]);
    assert_eq!(selected["run_status"], "open");
    assert_eq!(selected["assessment"], "dirty");
    assert_eq!(harness.selected(), dirty);
    assert_eq!(harness.ok(&["status"])["assessment"]["state"], "dirty");
    harness.ok(&["run", "stop", "--reason", "Superseded by a later capture."]);
    harness.ok(&["run", "select", &other]);
    let stopped = harness.ok(&["run", "select", &dirty]);
    assert_eq!(stopped["run_status"], "stopped");
    assert_eq!(stopped["assessment"], "dirty");
    harness.fails(&["accept"], 5, "run_terminal");
}

#[test]
fn run_start_derives_the_repository_binding_from_runs_left_without_a_workspace_file() {
    let harness = Harness::new();
    let run_id = harness.start();
    let manifest_path = harness.file(&run_id, "review-run.json");
    let manifest = read_json(&manifest_path);
    let mut foreign = manifest.clone();
    foreign["subject"]["repository"] = json!("group/other");
    write_json(&manifest_path, &foreign);
    let workspace = harness.runtime().join("review-workspace.json");
    fs::remove_file(&workspace).unwrap();

    harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        2,
        "repository_mismatch",
    );
    assert!(!workspace.exists(), "no binding is written");
    assert_eq!(
        harness.ok(&["run", "list"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "no run is installed"
    );

    write_json(&manifest_path, &manifest);
    let started = harness.ok(&["run", "start", "--new", "--mr", &harness.mr()]);
    assert_eq!(started["status"], "created");
    assert_eq!(
        read_json(&workspace)["repository"]["repository"],
        "group/sub/repo"
    );
}

#[test]
fn run_start_checks_the_repository_binding_under_the_workspace_lock() {
    let harness = Harness::new();
    let (ready, paused) = sync_channel(0);
    let (release, released) = sync_channel(0);
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::pause("/user", ready, released));
    let workspace = harness.runtime().join("review-workspace.json");
    let (output, value) = std::thread::scope(|scope| {
        let start = scope.spawn(|| harness.rkc(&["run", "start", "--mr", &harness.mr()]));
        paused.recv().unwrap();
        // Another start binds the worktree to another repository while this
        // one connects before taking the workspace lock.
        fs::create_dir_all(harness.runtime()).unwrap();
        fs::write(harness.runtime().join(".gitignore"), "*\n").unwrap();
        write_json(
            &workspace,
            &json!({
                "version": 1,
                "kind": "review_workspace",
                "repository": {
                    "provider": "gitlab",
                    "host": harness.server,
                    "repository": "group/other",
                },
                "selected_run_id": "00000000-0000-4000-8000-000000000001",
            }),
        );
        release.send(()).unwrap();
        start.join().unwrap()
    });
    assert_eq!(output.status.code(), Some(2), "{value:#}");
    assert_eq!(value["diagnostics"][0]["code"], "repository_mismatch");
    assert_eq!(
        read_json(&workspace)["repository"]["repository"],
        "group/other",
        "the binding is not rewritten"
    );
    assert!(!harness.runtime().join("review-runs").exists());
}
