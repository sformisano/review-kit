use std::{ffi::OsString, fs, path::Path};

use super::{
    GitWorktree, RunStorageError,
    durable::{directory_exists, file_exists},
    layout::{
        GITIGNORE, LOCK_FILE, RUNS_DIR, STAGING_DIR, WORKSPACE_FILE, is_recognized_runtime_entry,
    },
};

/// Check `runtime` before any runtime artifact is read or written.
///
/// In order: the runtime directory and its top-level managed entries are real
/// files or directories; Git tracks nothing under it; and, while no workspace
/// file exists, it holds only recognized names. Entries inside run directories
/// are checked when a run is opened or accessed.
#[review_kit_observation::operations]
#[helper]
pub(super) fn validate(worktree: &GitWorktree, runtime: &Path) -> Result<(), RunStorageError> {
    let present = directory_exists(runtime)?;
    let mut workspace = false;
    if present {
        for name in [GITIGNORE, LOCK_FILE] {
            file_exists(&runtime.join(name))?;
        }
        workspace = file_exists(&runtime.join(WORKSPACE_FILE))?;
        for name in [STAGING_DIR, RUNS_DIR] {
            directory_exists(&runtime.join(name))?;
        }
    }
    let tracked = worktree.tracked_runtime_paths()?;
    if !tracked.is_empty() {
        return Err(RunStorageError::Tracked { paths: tracked });
    }
    if present && !workspace {
        let entries = unrecognized_entries(runtime)?;
        if !entries.is_empty() {
            return Err(RunStorageError::Unrecognized { entries });
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn unrecognized_entries(runtime: &Path) -> Result<Vec<OsString>, RunStorageError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(runtime).map_err(|error| RunStorageError::io(runtime, error))? {
        let name = entry
            .map_err(|error| RunStorageError::io(runtime, error))?
            .file_name();
        if !is_recognized_runtime_entry(&name) {
            entries.push(name);
        }
    }
    entries.sort();
    Ok(entries)
}
