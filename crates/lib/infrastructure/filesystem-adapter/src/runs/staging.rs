use std::{
    fs, io,
    path::{Path, PathBuf},
};

use review_kit_application::review_run::RunId;
use review_kit_observation::Fields;
use serde::Serialize;

use super::{
    RunFile, RunStorageError, RunWriter, SnapshotFile, WorkingFile, WorkspaceLock,
    durable::{
        Entry, directory_exists, ensure_directory, file_exists, inspect, replace, sync_directory,
    },
    layout::{RUNS_DIR, STAGING_DIR},
    observation::{completion, file_fields},
};

/// Files a staged run must contain before installation.
const REQUIRED: [RunFile; 5] = [
    RunFile::Working(WorkingFile::Manifest),
    RunFile::Snapshot(SnapshotFile::Context),
    RunFile::Snapshot(SnapshotFile::Source),
    RunFile::Snapshot(SnapshotFile::Diff),
    RunFile::Snapshot(SnapshotFile::Conversation),
];

/// A run being written under `staging/<run-id>/`.
///
/// Staged files are never visible as a run. Dropping the handle without
/// [`Self::install`] leaves the directory for diagnosis by
/// [`RuntimeStore::staging_leftovers`](super::RuntimeStore::staging_leftovers).
#[derive(Debug)]
pub struct StagedRun<'lock> {
    lock: &'lock WorkspaceLock,
    run_id: RunId,
    path: PathBuf,
}

#[review_kit_observation::operations]
impl<'lock> StagedRun<'lock> {
    /// Fails with `RunExists` for an installed ID and `StagingExists` for a leftover.
    #[helper]
    pub(super) fn create(
        lock: &'lock WorkspaceLock,
        run_id: RunId,
    ) -> Result<Self, RunStorageError> {
        let runtime = lock.store().path();
        if installed(runtime, &run_id)? {
            return Err(RunStorageError::RunExists { run_id });
        }
        let staging = ensure_directory(runtime, STAGING_DIR)?;
        let path = staging.join(run_id.as_str());
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(RunStorageError::StagingExists { run_id });
            }
            Err(error) => return Err(RunStorageError::io(&path, error)),
        }
        sync_directory(&staging).map_err(|error| RunStorageError::unsynchronized(&path, error))?;
        Ok(Self { lock, run_id, path })
    }

    #[helper]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Write any run file, including retained snapshot files, durably.
    #[operation(
        name = "filesystem.runs.write_staged",
        input = file_fields(file).count("bytes", bytes.len()),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn write(&self, file: RunFile, bytes: &[u8]) -> Result<(), RunStorageError> {
        let directory = match file.directory() {
            None => self.path.clone(),
            Some(name) => ensure_directory(&self.path, name)?,
        };
        replace(&directory, file.file_name(), bytes)
    }

    #[helper]
    pub fn write_json(
        &self,
        file: RunFile,
        document: &impl Serialize,
    ) -> Result<(), RunStorageError> {
        let bytes = review_kit_json_adapter::encode(document)
            .map_err(|error| RunStorageError::json(self.path.join(file.name()), error))?;
        self.write(file, &bytes)
    }

    /// Rename the staged directory to `review-runs/<run-id>/` and synchronize
    /// both parents.
    ///
    /// Requires the manifest and the four retained snapshot files. An existing
    /// run directory is never replaced: the target is checked under the
    /// workspace lock, and the rename refuses a non-empty directory. Failure
    /// before the rename leaves the staging directory intact; after it, the run
    /// is installed and the workspace file is unchanged.
    #[operation(
        name = "filesystem.runs.install_run",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn install(self) -> Result<RunWriter<'lock>, RunStorageError> {
        let mut missing = Vec::new();
        for file in REQUIRED {
            if !file_exists(&self.path.join(file.name()))? {
                missing.push(file);
            }
        }
        if !missing.is_empty() {
            return Err(RunStorageError::IncompleteStaging {
                run_id: self.run_id,
                missing,
            });
        }
        let runtime = self.lock.store().path();
        let runs = ensure_directory(runtime, RUNS_DIR)?;
        let target = runs.join(self.run_id.as_str());
        if inspect(&target)? != Entry::Absent {
            return Err(RunStorageError::RunExists {
                run_id: self.run_id,
            });
        }
        match fs::rename(&self.path, &target) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::AlreadyExists
                ) =>
            {
                return Err(RunStorageError::RunExists {
                    run_id: self.run_id,
                });
            }
            Err(error) => return Err(RunStorageError::io(&target, error)),
        }
        for parent in [runs, runtime.join(STAGING_DIR)] {
            sync_directory(&parent)
                .map_err(|error| RunStorageError::unsynchronized(&target, error))?;
        }
        let lock: &'lock WorkspaceLock = self.lock;
        lock.run(&self.run_id)
    }
}

#[review_kit_observation::operations]
#[helper]
fn installed(runtime: &Path, run_id: &RunId) -> Result<bool, RunStorageError> {
    let runs = runtime.join(RUNS_DIR);
    Ok(directory_exists(&runs)? && inspect(&runs.join(run_id.as_str()))? != Entry::Absent)
}
