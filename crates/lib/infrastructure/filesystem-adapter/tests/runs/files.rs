use std::{ffi::OsString, fs};

use review_kit_filesystem_adapter::{
    RenderedFile, RunFile, RunStorageError, SnapshotDifference, SnapshotFile, WorkingFile,
    content_ref,
};
use serde_json::{Value, json};

use super::harness::{Repository, expected_snapshot, install_run, run_id};

// Run files have fixed names; callers never supply paths.
#[test]
fn run_files_have_the_fixed_layout_names() {
    let names: Vec<&str> = [
        RunFile::Working(WorkingFile::Manifest),
        SnapshotFile::Context.into(),
        SnapshotFile::Source.into(),
        SnapshotFile::Diff.into(),
        SnapshotFile::Conversation.into(),
        WorkingFile::Draft.into(),
        WorkingFile::CheckResult.into(),
        WorkingFile::RenderProfile.into(),
        WorkingFile::PublicationEvents.into(),
        WorkingFile::PublicationResult.into(),
        WorkingFile::Rendered(RenderedFile::Summary).into(),
        WorkingFile::Rendered(RenderedFile::TerminalSummary).into(),
    ]
    .into_iter()
    .map(RunFile::name)
    .collect();
    assert_eq!(
        names,
        [
            "review-run.json",
            "review-context.json",
            "reviewed-source.json",
            "merge-request.diff",
            "review-conversation.json",
            "review-assessment.draft.json",
            "review-check-result.json",
            "review-render-profile.json",
            "review-publication-events.json",
            "review-publication-result.json",
            "rendered/review-summary.md",
            "rendered/terminal-review-summary.md",
        ]
    );
}

// Retained context or source bytes that change at the same revision fail hash
// verification; a missing retained file is reported without a hash.
#[test]
fn snapshot_verification_detects_changed_and_missing_bytes() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let expected = expected_snapshot();
    run.verify_snapshot(&expected).unwrap();

    let changed = b"{\"context\":\"revision-1\"}\n";
    fs::write(run.path().join("review-context.json"), changed).unwrap();
    fs::remove_file(run.path().join("merge-request.diff")).unwrap();

    match run.verify_snapshot(&expected) {
        Err(RunStorageError::SnapshotMismatch { differences }) => assert_eq!(
            differences,
            vec![
                SnapshotDifference {
                    file: SnapshotFile::Context,
                    actual: Some(content_ref(changed).unwrap()),
                },
                SnapshotDifference {
                    file: SnapshotFile::Diff,
                    actual: None,
                },
            ]
        ),
        other => panic!("expected SnapshotMismatch, got {other:?}"),
    }
}

// Creating a draft preserves existing bytes; explicit replacement changes them.
#[test]
fn working_files_are_replaced_or_created_without_overwriting() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let draft = WorkingFile::Draft;

    assert!(run.create_json(draft, &json!({"rows": []})).unwrap());
    assert!(!run.create(draft, b"replacement").unwrap());
    assert_eq!(
        run.read_json::<Value>(draft.into()).unwrap(),
        json!({"rows": []})
    );

    run.replace(draft, b"edited").unwrap();
    assert_eq!(run.read_bytes(draft.into()).unwrap(), b"edited");
    assert_eq!(
        run.hash(draft.into()).unwrap(),
        content_ref(b"edited").unwrap()
    );
    run.replace_json(WorkingFile::CheckResult, &json!({"ok": true}))
        .unwrap();
    assert_eq!(
        fs::read(run.path().join("review-check-result.json")).unwrap(),
        b"{\n  \"ok\": true\n}\n"
    );
}

// A missing required file is a typed failure; optional reads report absence.
#[test]
fn missing_run_files_are_reported_by_name() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let file = RunFile::Working(WorkingFile::Draft);

    assert_eq!(run.read_optional_bytes(file).unwrap(), None);
    for result in [run.read_bytes(file).map(|_| ()), run.hash(file).map(|_| ())] {
        match result {
            Err(RunStorageError::MissingFile {
                file: missing,
                path,
            }) => {
                assert_eq!(missing, file);
                assert_eq!(path, run.path().join("review-assessment.draft.json"));
            }
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }
    fs::write(run.path().join("review-assessment.draft.json"), b"{").unwrap();
    assert!(matches!(
        run.read_json::<Value>(file),
        Err(RunStorageError::Json { .. })
    ));
}

// Stale previews can be removed; removal of an absent preview reports no change.
#[test]
fn rendered_previews_are_written_and_removed() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    assert!(!run.remove_rendered(RenderedFile::Summary).unwrap());

    run.replace(RenderedFile::Summary.into(), b"# Summary\n")
        .unwrap();
    let path = run.path().join("rendered/review-summary.md");
    assert_eq!(fs::read(&path).unwrap(), b"# Summary\n");
    assert!(run.remove_rendered(RenderedFile::Summary).unwrap());
    assert!(!path.exists());
    assert!(!run.remove_rendered(RenderedFile::Summary).unwrap());
    assert!(!run.remove_rendered(RenderedFile::TerminalSummary).unwrap());
}

// Listing returns valid installed runs and reports every other entry.
#[test]
fn run_listing_reports_unrecognized_entries() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    install_run(&lock, &run_id(2));
    install_run(&lock, &run_id(1));
    let runs = repository.runtime().join("review-runs");
    fs::create_dir(runs.join("not-a-run")).unwrap();
    fs::write(runs.join(run_id(3).as_str()), b"file").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(runs.join(run_id(1).as_str()), runs.join(run_id(4).as_str()))
        .unwrap();

    let listing = store.list_runs().unwrap();
    assert_eq!(listing.runs, vec![run_id(1), run_id(2)]);
    let mut unrecognized = vec![
        OsString::from(run_id(3).as_str()),
        OsString::from("not-a-run"),
    ];
    #[cfg(unix)]
    unrecognized.push(OsString::from(run_id(4).as_str()));
    unrecognized.sort();
    assert_eq!(listing.unrecognized, unrecognized);
    assert!(matches!(
        store.open_run(&run_id(3)),
        Err(RunStorageError::UnexpectedKind { .. })
    ));
}

// Redirected entries inside a run are rejected, and their targets stay untouched,
// including by temporary-file cleanup.
#[cfg(unix)]
#[test]
fn symlinked_run_entries_are_rejected() {
    use std::os::unix::fs::symlink;

    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    let elsewhere = repository.outside("elsewhere");
    let foreign = repository.outside("foreign");
    fs::write(foreign.join(".rkc-run-1-0.tmp"), b"not ours").unwrap();

    let run = install_run(&lock, &run_id(1));
    symlink(&foreign, run.path().join("draft-history")).unwrap();
    drop(run);
    let run = lock.run(&run_id(1)).unwrap();
    assert!(foreign.join(".rkc-run-1-0.tmp").exists());

    symlink(&elsewhere, run.path().join("accepted")).unwrap();
    let reference = content_ref(b"{}").unwrap();
    assert!(matches!(
        run.install_accepted(b"{}"),
        Err(RunStorageError::Redirected { path }) if path == run.path().join("accepted")
    ));
    assert!(matches!(
        run.read_accepted(&reference),
        Err(RunStorageError::Redirected { .. })
    ));
    symlink(&elsewhere, run.path().join("rendered")).unwrap();
    assert!(matches!(
        run.replace(RenderedFile::Summary.into(), b"# Summary"),
        Err(RunStorageError::Redirected { .. })
    ));
    let manifest = run.path().join("review-run.json");
    fs::remove_file(&manifest).unwrap();
    symlink(elsewhere.join("manifest.json"), &manifest).unwrap();
    assert!(matches!(
        run.read_bytes(WorkingFile::Manifest.into()),
        Err(RunStorageError::Redirected { .. })
    ));
    assert!(matches!(
        run.replace(WorkingFile::Manifest, b"{}"),
        Err(RunStorageError::Redirected { .. })
    ));
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);

    let redirected = repository
        .runtime()
        .join("review-runs")
        .join(run_id(2).as_str());
    symlink(run.path(), &redirected).unwrap();
    assert!(matches!(
        store.open_run(&run_id(2)),
        Err(RunStorageError::Redirected { path }) if path == redirected
    ));
}
