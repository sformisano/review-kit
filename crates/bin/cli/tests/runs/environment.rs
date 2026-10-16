use std::fs;

use serde_json::json;

use super::harness::{ERROR_POLICY, Harness, git, read_json, write_json};

#[test]
fn commands_outside_a_git_worktree_fail_without_runtime_files() {
    let harness = Harness::new();
    let outside = harness.root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    for args in [
        vec!["status"],
        vec!["run", "list"],
        vec!["accept"],
        vec!["run", "start", "--mr", &harness.mr()],
        vec!["publish", "--effects", "review"],
    ] {
        harness.fails_in(&outside, &args, 2, "not_in_git_worktree");
    }
    assert!(!outside.join(".review-kit").exists());
    assert_eq!(harness.http.state.lock().unwrap().requests.len(), 0);
}

#[test]
fn subdirectories_share_the_worktree_runtime_and_linked_worktrees_are_separate() {
    let harness = Harness::new();
    let nested = harness.repo.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    let started = harness.ok_in(&nested, &["run", "start", "--mr", &harness.mr()]);
    let run_id = started["run_id"].as_str().unwrap().to_owned();
    assert!(harness.run_dir(&run_id).is_dir());
    assert!(!nested.join(".review-kit").exists());
    harness.declare_policies(&run_id, &[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    harness.ok_in(&nested, &["accept"]);
    assert_eq!(harness.ok_in(&nested, &["status"])["run_id"], run_id);

    let linked = harness.root.path().join("linked");
    git(
        &harness.repo,
        &["worktree", "add", "-q", linked.to_str().unwrap()],
    );
    harness.fails_in(&linked, &["status"], 2, "no_selected_run");
    assert_eq!(harness.ok_in(&linked, &["run", "list"])["runs"], json!([]));
    let other = harness.ok_in(&linked, &["run", "start", "--mr", &harness.mr()]);
    assert_ne!(other["run_id"], run_id);
    assert!(
        linked
            .join(".review-kit/review-runs")
            .join(other["run_id"].as_str().unwrap())
            .is_dir()
    );
    assert_eq!(harness.selected(), run_id);
    assert_eq!(
        harness.ok(&["run", "list"])["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn tracked_redirected_and_unrecognized_runtime_paths_are_rejected_before_use() {
    let harness = Harness::new();
    fs::create_dir_all(harness.runtime()).unwrap();
    fs::write(harness.runtime().join("notes.txt"), "unrelated").unwrap();
    harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        2,
        "runtime_unrecognized",
    );
    assert_eq!(harness.writes(), 0);
    fs::remove_file(harness.runtime().join("notes.txt")).unwrap();

    let run_id = harness.start();
    git(
        &harness.repo,
        &["add", "-f", ".review-kit/review-workspace.json"],
    );
    harness.fails(&["status"], 2, "runtime_tracked");
    harness.fails(&["accept"], 2, "runtime_tracked");
    git(
        &harness.repo,
        &["rm", "-q", "--cached", ".review-kit/review-workspace.json"],
    );
    assert_eq!(harness.ok(&["status"])["run_id"], run_id);

    let moved = harness.root.path().join("redirected-runtime");
    fs::rename(harness.runtime(), &moved).unwrap();
    std::os::unix::fs::symlink(&moved, harness.runtime()).unwrap();
    harness.fails(&["status"], 2, "runtime_redirected");
    harness.fails(&["run", "list"], 2, "runtime_redirected");
    fs::remove_file(harness.runtime()).unwrap();
    fs::rename(&moved, harness.runtime()).unwrap();
    assert_eq!(harness.ok(&["status"])["run_id"], run_id);
}

#[test]
fn open_runs_require_new_and_selection_never_recaptures() {
    let harness = Harness::new();
    let first = harness.start();
    let refused = harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        5,
        "open_run_exists",
    );
    assert_eq!(refused["diagnostics"][0]["run_ids"], json!([first]));
    harness.fails(&["run", "start"], 5, "open_run_exists");
    let second = harness.ok(&["run", "start", "--new"]);
    let second = second["run_id"].as_str().unwrap().to_owned();
    let listed = harness.ok(&["run", "list"]);
    let runs = listed["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(
        runs.iter()
            .any(|run| run["run_id"] == first && run["selected"] == false)
    );
    assert!(
        runs.iter()
            .any(|run| run["run_id"] == second && run["selected"] == true)
    );
    let requests = harness.http.state.lock().unwrap().requests.len();
    let selected = harness.ok(&["run", "select", &first]);
    assert_eq!(selected["selected_run_id"], first);
    assert_eq!(selected["run_status"], "open");
    assert_eq!(harness.selected(), first);
    assert_eq!(harness.ok(&["status"])["run_id"], first);
    assert_eq!(harness.http.state.lock().unwrap().requests.len(), requests);
    harness.fails(&["run", "select", "not-a-run"], 2, "invalid_run_id");
    harness.fails(
        &["run", "select", "00000000-0000-4000-8000-000000000009"],
        2,
        "run_missing",
    );
    assert_eq!(harness.selected(), first);
}

#[test]
fn repository_binding_and_reviewer_identity_bind_every_run() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    harness.ok(&["accept"]);
    let workspace = harness.runtime().join("review-workspace.json");
    let original = read_json(&workspace);
    let mut other = original.clone();
    other["repository"]["repository"] = json!("group/other");
    write_json(&workspace, &other);
    harness.fails(&["status"], 2, "repository_mismatch");
    harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        2,
        "repository_mismatch",
    );
    harness.fails(&["run", "select", &run_id], 2, "repository_mismatch");
    write_json(&workspace, &original);

    harness.http.state.lock().unwrap().authenticated_actor = 43;
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        4,
        "wrong_reviewer",
    );
    harness.fails(&["publish", "--effects", "review"], 4, "wrong_reviewer");
    assert_eq!(harness.writes(), 0);
    let other = harness.ok(&["run", "start"]);
    assert_eq!(
        other["reviewer"], "43",
        "the open run belongs to another reviewer"
    );
    harness.ok(&["run", "select", &run_id]);
    harness.http.state.lock().unwrap().authenticated_actor = 42;
    harness.ok(&["publish", "--effects", "review", "--plan"]);
}

#[test]
fn held_workspace_and_publication_locks_are_busy_conflicts() {
    let harness = Harness::new();
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&run_id);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(harness.runtime().join("review-workspace.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    harness.fails(&["accept"], 5, "workspace_busy");
    harness.fails(&["run", "start", "--new"], 5, "workspace_busy");
    assert_eq!(
        harness.ok(&["status"])["run_id"],
        run_id,
        "diagnostics need no lock"
    );
    drop(lock);
    harness.ok(&["accept"]);

    let client = harness.http.connect();
    let store = review_kit_filesystem_adapter::FileProgressStore::new(
        harness.state(),
        client.subject(),
        client.reviewer(),
    );
    let guard = store.try_lock().unwrap();
    harness.fails(&["publish", "--effects", "review"], 5, "publication_busy");
    harness.fails(
        &["publish", "--effects", "review", "--plan"],
        5,
        "publication_busy",
    );
    assert_eq!(harness.writes(), 0);
    drop(guard);
    harness.ok(&["publish", "--effects", "review"]);
}
