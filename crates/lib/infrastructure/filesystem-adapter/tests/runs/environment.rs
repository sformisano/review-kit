use std::{ffi::OsString, fs, path::PathBuf};

use review_kit_filesystem_adapter::{GitWorktree, RunStorageError, RuntimeStore};

use super::harness::{Repository, Selection, git_in, git_output, install_run, run_id};

// Paths resolve from the worktree root even when invoked in a subdirectory.
#[test]
fn subdirectory_invocation_uses_the_worktree_root() {
    let repository = Repository::new();
    let nested = repository.root().join("src/module");
    fs::create_dir_all(&nested).unwrap();

    let worktree = GitWorktree::locate(&nested).unwrap();
    assert_eq!(worktree.root(), repository.root());
    let store = RuntimeStore::open(worktree).unwrap();
    assert_eq!(store.path(), repository.runtime());
    drop(store.create_and_lock().unwrap());

    assert!(repository.runtime().is_dir());
    assert!(!nested.join(".review-kit").exists());
}

// Linked worktrees have separate runtime directories.
#[test]
fn linked_worktrees_keep_separate_runtime_directories() {
    let repository = Repository::new();
    let linked = repository.linked_worktree("linked");
    let linked_nested = linked.join("docs");
    fs::create_dir_all(&linked_nested).unwrap();

    let linked_worktree = GitWorktree::locate(&linked_nested).unwrap();
    assert_eq!(linked_worktree.root(), linked);
    let main = repository.store();
    let other = RuntimeStore::open(linked_worktree).unwrap();
    assert_ne!(main.path(), other.path());

    let main_lock = main.create_and_lock().unwrap();
    let other_lock = other.create_and_lock().unwrap();
    main_lock
        .write_workspace(&Selection::of(&run_id(1)))
        .unwrap();
    other_lock
        .write_workspace(&Selection::of(&run_id(2)))
        .unwrap();
    install_run(&main_lock, &run_id(1));

    assert_eq!(
        main.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(1)))
    );
    assert_eq!(
        other.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(2)))
    );
    assert_eq!(main.list_runs().unwrap().runs, vec![run_id(1)]);
    assert!(other.list_runs().unwrap().runs.is_empty());
}

// Outside a worktree, locating the root fails with a typed error and creates nothing.
#[test]
fn directories_outside_a_worktree_are_rejected() {
    let repository = Repository::new();
    let bare = repository.outside("bare.git");
    git_in(&bare, &["init", "-q", "--bare"]);
    let plain = repository.outside("plain");
    assert!(
        !git_output(&plain, &["rev-parse", "--show-toplevel"])
            .status
            .success(),
        "the temporary directory must lie outside every Git worktree"
    );

    for directory in [repository.root().join(".git"), bare, plain.clone()] {
        match GitWorktree::locate(&directory) {
            Err(RunStorageError::NotInWorktree {
                directory: reported,
                status,
                ..
            }) => {
                assert_eq!(reported, directory);
                assert!(!status.success());
            }
            other => panic!("{directory:?}: expected NotInWorktree, got {other:?}"),
        }
    }
    assert!(!plain.join(".review-kit").exists());
}

// A tracked runtime path is rejected before any runtime artifact is consumed,
// including when it becomes tracked after the store was opened.
#[test]
fn tracked_runtime_files_are_rejected() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    lock.write_workspace(&Selection::of(&run_id(1))).unwrap();
    drop(lock);
    repository.git(&["add", "-f", ".review-kit/review-workspace.json"]);

    let expected = vec![PathBuf::from(".review-kit/review-workspace.json")];
    match RuntimeStore::open(GitWorktree::locate(repository.root()).unwrap()) {
        Err(RunStorageError::Tracked { paths }) => assert_eq!(paths, expected),
        other => panic!("expected Tracked, got {other:?}"),
    }
    match store.lock() {
        Err(RunStorageError::Tracked { paths }) => assert_eq!(paths, expected),
        other => panic!("expected Tracked under the lock, got {other:?}"),
    }
}

// A redirected runtime directory is rejected and its target is never written.
#[cfg(unix)]
#[test]
fn symlinked_runtime_directory_is_rejected() {
    let repository = Repository::new();
    let target = repository.outside("elsewhere");
    std::os::unix::fs::symlink(&target, repository.runtime()).unwrap();

    match RuntimeStore::open(GitWorktree::locate(repository.root()).unwrap()) {
        Err(RunStorageError::Redirected { path }) => assert_eq!(path, repository.runtime()),
        other => panic!("expected Redirected, got {other:?}"),
    }
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

// Every managed component below `.review-kit` must be a real entry.
#[cfg(unix)]
#[test]
fn symlinked_managed_components_are_rejected() {
    use std::os::unix::fs::symlink;

    for component in [
        "review-runs",
        "staging",
        "review-workspace.json",
        "review-workspace.lock",
    ] {
        let repository = Repository::new();
        let lock = repository.store().create_and_lock().unwrap();
        lock.write_workspace(&Selection::of(&run_id(1))).unwrap();
        drop(lock);
        let target = repository.outside("elsewhere");
        let path = repository.runtime().join(component);
        let _ = fs::remove_file(&path);
        symlink(&target, &path).unwrap();

        match RuntimeStore::open(GitWorktree::locate(repository.root()).unwrap()) {
            Err(RunStorageError::Redirected { path: reported }) => assert_eq!(reported, path),
            other => panic!("{component}: expected Redirected, got {other:?}"),
        }
    }
}

// An unrecognized pre-existing `.review-kit` is not adopted.
#[test]
fn unrecognized_runtime_directory_is_rejected() {
    let repository = Repository::new();
    fs::create_dir(repository.runtime()).unwrap();
    fs::write(repository.runtime().join("notes.txt"), b"mine").unwrap();
    fs::create_dir(repository.runtime().join("cache")).unwrap();

    match RuntimeStore::open(GitWorktree::locate(repository.root()).unwrap()) {
        Err(RunStorageError::Unrecognized { entries }) => assert_eq!(
            entries,
            vec![OsString::from("cache"), OsString::from("notes.txt")]
        ),
        other => panic!("expected Unrecognized, got {other:?}"),
    }
    assert_eq!(
        fs::read(repository.runtime().join("notes.txt")).unwrap(),
        b"mine"
    );
}

// Recognized names, including leftovers of an interrupted first run start, are accepted
// without a workspace file. Once the workspace file exists, other entries are tolerated.
#[test]
fn recognized_runtime_entries_are_accepted() {
    let repository = Repository::new();
    let runtime = repository.runtime();
    fs::create_dir_all(runtime.join("staging").join(run_id(1).as_str())).unwrap();
    fs::create_dir(runtime.join("review-runs")).unwrap();
    fs::write(runtime.join(".gitignore"), b"*\n").unwrap();
    fs::write(runtime.join("review-workspace.lock"), b"").unwrap();
    fs::write(runtime.join(".rkc-run-1-0.tmp"), b"partial").unwrap();
    repository.store();

    fs::write(runtime.join("review-workspace.json"), b"{}").unwrap();
    fs::write(runtime.join("notes.txt"), b"mine").unwrap();
    repository.store();
}

// Creation writes `.gitignore` with `*`, so runtime files stay out of Git staging;
// a changed ignore file is restored under the lock.
#[test]
fn runtime_creation_keeps_runtime_files_out_of_git() {
    let repository = Repository::new();
    let store = repository.store();
    let lock = store.create_and_lock().unwrap();
    lock.write_workspace(&Selection::of(&run_id(1))).unwrap();
    install_run(&lock, &run_id(1));
    drop(lock);

    let gitignore = repository.runtime().join(".gitignore");
    assert_eq!(fs::read(&gitignore).unwrap(), b"*\n");
    let status = repository.git(&["status", "--porcelain", "--untracked-files=all"]);
    assert_eq!(status, "");

    fs::write(&gitignore, b"!*\n").unwrap();
    drop(store.lock().unwrap());
    assert_eq!(fs::read(&gitignore).unwrap(), b"*\n");
}

// Locking without creation leaves a worktree without runtime state untouched.
#[test]
fn locking_requires_an_existing_runtime_directory() {
    let repository = Repository::new();
    let store = repository.store();
    assert!(matches!(store.lock(), Err(RunStorageError::Uninitialized)));
    assert!(!repository.runtime().exists());
    assert_eq!(store.read_workspace::<Selection>().unwrap(), None);
    assert!(store.list_runs().unwrap().runs.is_empty());
}
