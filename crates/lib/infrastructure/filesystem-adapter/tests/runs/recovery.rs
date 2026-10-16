use std::fs;

use review_kit_filesystem_adapter::{WorkingFile, content_ref};

use super::harness::{
    MANIFEST, Repository, Selection, entries, install_run, run_id, stage_without_installing,
};

// A crash can leave temporary files from an atomic write. They are never read or
// listed, they do not make `.review-kit` unrecognized, and the next lock holder
// removes them.
#[test]
fn leftover_temporary_files_are_ignored_and_removed_under_the_lock() {
    let repository = Repository::new();
    let runtime = repository.runtime();
    fs::create_dir(&runtime).unwrap();
    fs::write(runtime.join(".rkc-run-77-0.tmp"), b"partial").unwrap();
    fs::write(runtime.join(".rkc-77-1.tmp"), b"partial").unwrap();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    assert_eq!(
        entries(&runtime),
        vec![".gitignore", "review-workspace.lock"]
    );

    let run = install_run(&lock, &run_id(1));
    run.install_accepted(b"{}").unwrap();
    run.archive_draft(b"{").unwrap();
    run.replace(
        WorkingFile::Rendered(review_kit_filesystem_adapter::RenderedFile::Summary),
        b"#",
    )
    .unwrap();
    let directories = [
        run.path().to_owned(),
        run.path().join("accepted"),
        run.path().join("draft-history"),
        run.path().join("rendered"),
    ];
    let before: Vec<Vec<String>> = directories.iter().map(|path| entries(path)).collect();
    drop(run);
    for directory in &directories {
        fs::write(directory.join(".rkc-run-77-2.tmp"), b"partial").unwrap();
    }

    let reader = store.open_run(&run_id(1)).unwrap();
    assert_eq!(
        reader.read_bytes(WorkingFile::Manifest.into()).unwrap(),
        MANIFEST
    );
    assert_eq!(store.list_runs().unwrap().runs, vec![run_id(1)]);
    assert!(store.list_runs().unwrap().unrecognized.is_empty());
    assert!(directories[0].join(".rkc-run-77-2.tmp").exists());

    lock.run(&run_id(1)).unwrap();
    let after: Vec<Vec<String>> = directories.iter().map(|path| entries(path)).collect();
    assert_eq!(after, before);
}

// A crash after staging but before installation keeps the previous selection, and
// the leftover never becomes a run.
#[test]
fn interrupted_run_start_keeps_the_previous_selection() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    install_run(&lock, &run_id(1));
    lock.write_workspace(&Selection::of(&run_id(1))).unwrap();
    stage_without_installing(&lock, &run_id(2));
    drop(lock);

    let store = repository.store();
    assert_eq!(
        store.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(1)))
    );
    assert_eq!(store.list_runs().unwrap().runs, vec![run_id(1)]);
    assert_eq!(store.staging_leftovers().unwrap().runs, vec![run_id(2)]);

    let lock = store.lock().unwrap();
    install_run(&lock, &run_id(3));
    assert_eq!(store.list_runs().unwrap().runs, vec![run_id(1), run_id(3)]);
    assert_eq!(store.staging_leftovers().unwrap().runs, vec![run_id(2)]);
}

// A crash after installation but before the selection changes leaves a complete,
// listed run and the previous selection; installing and selecting are recoverable.
#[test]
fn installed_run_without_selection_change_is_listed_but_not_selected() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    install_run(&lock, &run_id(1));
    lock.write_workspace(&Selection::of(&run_id(1))).unwrap();
    install_run(&lock, &run_id(2));
    drop(lock);

    let store = repository.store();
    assert_eq!(
        store.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(1)))
    );
    assert_eq!(store.list_runs().unwrap().runs, vec![run_id(1), run_id(2)]);
    assert_eq!(
        store
            .open_run(&run_id(2))
            .unwrap()
            .read_bytes(WorkingFile::Manifest.into())
            .unwrap(),
        MANIFEST
    );
    let lock = store.lock().unwrap();
    lock.write_workspace(&Selection::of(&run_id(2))).unwrap();
    assert_eq!(
        store.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(2)))
    );
}

// An acceptance interrupted after creating its digest directory but before
// linking the snapshot leaves no snapshot; the retry installs it.
#[test]
fn interrupted_accepted_installation_completes_on_retry() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let bytes = b"{\"policies\": []}";
    let reference = content_ref(bytes).unwrap();
    let accepted = run.path().join("accepted");
    fs::create_dir_all(accepted.join(reference.hex())).unwrap();
    fs::write(accepted.join(".rkc-run-77-0.tmp"), bytes).unwrap();
    assert!(run.read_accepted(&reference).is_err());

    let stored = run.install_accepted(bytes).unwrap();
    assert!(stored.created);
    assert_eq!(stored.reference, reference);
    assert_eq!(run.read_accepted(&reference).unwrap(), bytes);
}
