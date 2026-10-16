use std::fs;

use review_kit_filesystem_adapter::{RunFile, RunStorageError, SnapshotFile, WorkingFile};

use super::harness::{
    ASSESSMENT_DRAFT, MANIFEST, Repository, SNAPSHOT, Selection, entries, install_run, run_id,
    stage_without_installing,
};

// Incomplete staging is never listed, opened or selected as a run, and it
// stays on disk for diagnosis.
#[test]
fn staged_runs_are_never_runs() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    stage_without_installing(&lock, &run_id(1));

    assert!(store.list_runs().unwrap().runs.is_empty());
    assert_eq!(store.staging_leftovers().unwrap().runs, vec![run_id(1)]);
    assert!(matches!(
        store.open_run(&run_id(1)),
        Err(RunStorageError::RunMissing { run_id: missing }) if missing == run_id(1)
    ));
    assert!(matches!(
        lock.run(&run_id(1)),
        Err(RunStorageError::RunMissing { .. })
    ));
    assert!(matches!(
        lock.stage_run(run_id(1)),
        Err(RunStorageError::StagingExists { .. })
    ));
    let staged = repository
        .runtime()
        .join("staging")
        .join(run_id(1).as_str());
    assert_eq!(
        fs::read(staged.join("review-assessment.draft.json")).unwrap(),
        ASSESSMENT_DRAFT
    );
}

// Installation requires the manifest and the four retained files.
#[test]
fn incomplete_staging_is_not_installed() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    let staged = lock.stage_run(run_id(1)).unwrap();
    staged
        .write(WorkingFile::Manifest.into(), MANIFEST)
        .unwrap();
    staged
        .write(SnapshotFile::Context.into(), SNAPSHOT[0].1)
        .unwrap();

    match staged.install() {
        Err(RunStorageError::IncompleteStaging {
            run_id: id,
            missing,
        }) => {
            assert_eq!(id, run_id(1));
            assert_eq!(
                missing,
                vec![
                    RunFile::Snapshot(SnapshotFile::Source),
                    RunFile::Snapshot(SnapshotFile::Diff),
                    RunFile::Snapshot(SnapshotFile::Conversation),
                ]
            );
        }
        other => panic!("expected IncompleteStaging, got {other:?}"),
    }
    assert!(store.list_runs().unwrap().runs.is_empty());
    assert_eq!(store.staging_leftovers().unwrap().runs, vec![run_id(1)]);
}

// Installation moves the complete directory and leaves the workspace file unchanged;
// selecting the new run is a separate later write.
#[test]
fn installation_moves_the_staged_directory() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    lock.write_workspace(&Selection::of(&run_id(9))).unwrap();
    let staged = lock.stage_run(run_id(1)).unwrap();
    for (file, bytes) in SNAPSHOT {
        staged.write(file.into(), bytes).unwrap();
    }
    staged
        .write(WorkingFile::Manifest.into(), MANIFEST)
        .unwrap();
    staged
        .write(RunFile::Working(WorkingFile::Draft), ASSESSMENT_DRAFT)
        .unwrap();
    let run = staged.install().unwrap();

    assert_eq!(run.run_id(), &run_id(1));
    assert_eq!(run.path(), repository.run_path(&run_id(1)));
    assert_eq!(
        entries(run.path()),
        vec![
            "merge-request.diff",
            "review-assessment.draft.json",
            "review-context.json",
            "review-conversation.json",
            "review-run.json",
            "reviewed-source.json",
        ]
    );
    assert_eq!(
        run.read_bytes(WorkingFile::Manifest.into()).unwrap(),
        MANIFEST
    );
    assert_eq!(store.list_runs().unwrap().runs, vec![run_id(1)]);
    assert!(store.staging_leftovers().unwrap().runs.is_empty());
    assert_eq!(
        store.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(9)))
    );
}

// An existing run directory is never replaced, whether it appears before staging
// or between staging and installation, and whether or not it is empty.
#[test]
fn installation_never_replaces_an_existing_run() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    install_run(&lock, &run_id(1));
    assert!(matches!(
        lock.stage_run(run_id(1)),
        Err(RunStorageError::RunExists { .. })
    ));

    for (number, marker) in [(2, Some(b"existing".as_slice())), (3, None)] {
        let staged = lock.stage_run(run_id(number)).unwrap();
        for (file, bytes) in SNAPSHOT {
            staged.write(file.into(), bytes).unwrap();
        }
        staged
            .write(WorkingFile::Manifest.into(), MANIFEST)
            .unwrap();
        let existing = repository.run_path(&run_id(number));
        fs::create_dir(&existing).unwrap();
        if let Some(marker) = marker {
            fs::write(existing.join("marker"), marker).unwrap();
        }

        assert!(matches!(
            staged.install(),
            Err(RunStorageError::RunExists { run_id: id }) if id == run_id(number)
        ));
        let expected: Vec<String> = marker.iter().map(|_| "marker".to_owned()).collect();
        assert_eq!(entries(&existing), expected);
        if let Some(marker) = marker {
            assert_eq!(fs::read(existing.join("marker")).unwrap(), marker);
        }
        let staging = repository
            .runtime()
            .join("staging")
            .join(run_id(number).as_str());
        assert_eq!(fs::read(staging.join("review-run.json")).unwrap(), MANIFEST);
    }
    assert_eq!(
        repository
            .store()
            .open_run(&run_id(1))
            .unwrap()
            .read_bytes(WorkingFile::Manifest.into())
            .unwrap(),
        MANIFEST
    );
}
