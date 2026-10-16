use std::fs::{File, TryLockError};

use review_kit_application::review_run::RunId;
use review_kit_observation::Fields;
use serde::Serialize;

use super::{
    RunStorageError, RunWriter, RuntimeStore, StagedRun,
    durable::{directory_exists, file_exists, open_file, read, remove_temporaries, replace},
    environment::validate,
    layout::{GITIGNORE, GITIGNORE_CONTENT, LOCK_FILE, WORKSPACE_FILE},
    observation::completion,
};

/// Exclusive local mutation rights for one runtime directory.
///
/// The operating system lock on `review-workspace.lock` lasts until this guard
/// drops or the process ends. The lock file itself persists.
#[derive(Debug)]
#[must_use = "retain the lock for every runtime mutation of the command"]
pub struct WorkspaceLock {
    store: RuntimeStore,
    _file: File,
}

#[review_kit_observation::operations]
impl WorkspaceLock {
    /// Lock, revalidate the environment, restore `.gitignore` and remove
    /// temporary files left directly in `.review-kit/`.
    #[helper]
    pub(super) fn acquire(store: RuntimeStore) -> Result<Self, RunStorageError> {
        let runtime = store.path();
        if !directory_exists(runtime)? {
            return Err(RunStorageError::Uninitialized);
        }
        let path = runtime.join(LOCK_FILE);
        file_exists(&path)?;
        let file = open_file(&path, |options| {
            options.read(true).write(true).create(true).truncate(false);
        })?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(RunStorageError::WorkspaceBusy),
            Err(TryLockError::Error(error)) => return Err(RunStorageError::io(&path, error)),
        }
        let lock = Self { store, _file: file };
        let runtime = lock.store.path();
        validate(lock.store.worktree(), runtime)?;
        if read(&runtime.join(GITIGNORE))?.as_deref() != Some(GITIGNORE_CONTENT) {
            replace(runtime, GITIGNORE, GITIGNORE_CONTENT)?;
        }
        remove_temporaries(runtime);
        Ok(lock)
    }

    #[helper]
    pub fn store(&self) -> &RuntimeStore {
        &self.store
    }

    /// Replace `review-workspace.json` atomically and durably.
    #[operation(
        name = "filesystem.runs.write_workspace",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn write_workspace<T: Serialize>(&self, document: &T) -> Result<(), RunStorageError> {
        let runtime = self.store.path();
        let path = runtime.join(WORKSPACE_FILE);
        let bytes = review_kit_json_adapter::encode(document)
            .map_err(|error| RunStorageError::json(&path, error))?;
        replace(runtime, WORKSPACE_FILE, &bytes)
    }

    /// Create `staging/<run-id>/` for a run that is not installed.
    #[operation(
        name = "filesystem.runs.stage_run",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn stage_run(&self, run_id: RunId) -> Result<StagedRun<'_>, RunStorageError> {
        StagedRun::create(self, run_id)
    }

    /// Open an installed run for mutation and remove its leftover temporary files.
    #[operation(
        name = "filesystem.runs.open_run_writer",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn run(&self, run_id: &RunId) -> Result<RunWriter<'_>, RunStorageError> {
        RunWriter::open(self, run_id)
    }
}
