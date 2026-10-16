use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use review_kit_application::review_run::RunId;
use review_kit_observation::Fields;
use serde::de::DeserializeOwned;

use super::{
    GitWorktree, RunDir, RunStorageError, WorkspaceLock,
    durable::{directory_exists, read, sync_directory},
    environment::validate,
    layout::{RUNS_DIR, RUNTIME_DIR, STAGING_DIR, WORKSPACE_FILE},
    observation::completion,
};

/// The validated `.review-kit/` directory of one Git worktree.
///
/// Reads need no lock. Mutations go through [`WorkspaceLock`].
#[derive(Debug, Clone)]
pub struct RuntimeStore {
    worktree: GitWorktree,
    runtime: PathBuf,
}

/// Entries of `review-runs/` or `staging/`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunListing {
    /// Real directories named by a valid run ID, in ascending order.
    pub runs: Vec<RunId>,
    /// Every other entry, including symbolic links, in ascending order.
    pub unrecognized: Vec<OsString>,
}

#[review_kit_observation::operations]
impl RuntimeStore {
    /// Validate the worktree's runtime directory, which may be absent.
    #[operation(
        name = "filesystem.runs.open_store",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn open(worktree: GitWorktree) -> Result<Self, RunStorageError> {
        let runtime = worktree.root().join(RUNTIME_DIR);
        validate(&worktree, &runtime)?;
        Ok(Self { worktree, runtime })
    }

    #[helper]
    pub fn worktree(&self) -> &GitWorktree {
        &self.worktree
    }

    /// The `.review-kit` directory path, whether or not it exists.
    #[helper]
    pub fn path(&self) -> &Path {
        &self.runtime
    }

    /// Decode `review-workspace.json`, or `None` when it is absent.
    #[operation(
        name = "filesystem.runs.read_workspace",
        input = Fields::new(),
        output = |result| completion(result, |document: &Option<T>| Fields::new().flag("present", document.is_some())),
    )]
    pub fn read_workspace<T: DeserializeOwned>(&self) -> Result<Option<T>, RunStorageError> {
        if !directory_exists(&self.runtime)? {
            return Ok(None);
        }
        let path = self.runtime.join(WORKSPACE_FILE);
        read(&path)?
            .map(|bytes| {
                review_kit_json_adapter::decode(&bytes)
                    .map_err(|error| RunStorageError::json(&path, error))
            })
            .transpose()
    }

    /// Installed runs. Staging directories never appear here.
    #[operation(
        name = "filesystem.runs.list_runs",
        input = Fields::new(),
        output = |result| completion(result, listing_fields),
    )]
    pub fn list_runs(&self) -> Result<RunListing, RunStorageError> {
        listing(&self.runtime.join(RUNS_DIR))
    }

    /// Staging directories left by interrupted run creation, kept for diagnosis.
    #[operation(
        name = "filesystem.runs.list_staging",
        input = Fields::new(),
        output = |result| completion(result, listing_fields),
    )]
    pub fn staging_leftovers(&self) -> Result<RunListing, RunStorageError> {
        listing(&self.runtime.join(STAGING_DIR))
    }

    /// Open an installed run for reading.
    #[operation(
        name = "filesystem.runs.open_run",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn open_run(&self, run_id: &RunId) -> Result<RunDir, RunStorageError> {
        RunDir::open(&self.runtime, run_id)
    }

    /// Acquire the workspace lock of an existing runtime directory without waiting.
    ///
    /// The runtime directory is validated again while the lock is held.
    #[operation(
        name = "filesystem.runs.lock_workspace",
        input = Fields::new().flag("create", false),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn lock(&self) -> Result<WorkspaceLock, RunStorageError> {
        WorkspaceLock::acquire(self.clone())
    }

    /// Create the runtime directory when absent, then lock it.
    ///
    /// `.gitignore` is written only after the lock is held.
    #[operation(
        name = "filesystem.runs.lock_workspace",
        input = Fields::new().flag("create", true),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn create_and_lock(&self) -> Result<WorkspaceLock, RunStorageError> {
        match fs::create_dir(&self.runtime) {
            Ok(()) => sync_directory(self.worktree.root())
                .map_err(|error| RunStorageError::unsynchronized(&self.runtime, error))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(RunStorageError::io(&self.runtime, error)),
        }
        WorkspaceLock::acquire(self.clone())
    }
}

#[review_kit_observation::operations]
#[helper]
fn listing(directory: &Path) -> Result<RunListing, RunStorageError> {
    let mut listing = RunListing::default();
    if !directory_exists(directory)? {
        return Ok(listing);
    }
    for entry in fs::read_dir(directory).map_err(|error| RunStorageError::io(directory, error))? {
        let entry = entry.map_err(|error| RunStorageError::io(directory, error))?;
        let name = entry.file_name();
        let real_directory = entry
            .file_type()
            .map_err(|error| RunStorageError::io(entry.path(), error))?
            .is_dir();
        match name.to_str().map(|text| RunId::try_from(text.to_owned())) {
            Some(Ok(run_id)) if real_directory => listing.runs.push(run_id),
            _ => listing.unrecognized.push(name),
        }
    }
    listing.runs.sort();
    listing.unrecognized.sort();
    Ok(listing)
}

#[review_kit_observation::operations]
#[helper]
fn listing_fields(listing: &RunListing) -> Fields {
    Fields::new()
        .count("runs", listing.runs.len())
        .count("unrecognized", listing.unrecognized.len())
}
