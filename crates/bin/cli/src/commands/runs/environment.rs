use review_kit_application::review_run::RunId;
use review_kit_filesystem_adapter::{
    GitWorktree, RunStorageError, RuntimeStore, WorkingFile, WorkspaceLock,
};
use review_kit_json_adapter::checkpoints::{
    RepositoryBinding, ReviewRunDocument, ReviewWorkspaceDocument,
};

use crate::error::CliError;

/// The validated runtime store of the Git worktree containing the invocation directory.
///
/// Every runtime path resolves from the worktree root, whatever subdirectory the
/// command runs in. Environment checks run before any runtime file is read.
#[review_kit_observation::operations]
#[helper]
pub fn locate() -> Result<RuntimeStore, CliError> {
    let directory = std::env::current_dir()
        .map_err(|error| CliError::adapter("read invocation directory", "filesystem", 7, error))?;
    let worktree = GitWorktree::locate(&directory)
        .map_err(|error| CliError::storage("locate Git worktree", error))?;
    RuntimeStore::open(worktree).map_err(|error| CliError::storage("open review runtime", error))
}

/// `review-workspace.json`, or `None` before the first run.
#[review_kit_observation::operations]
#[helper]
pub fn workspace(store: &RuntimeStore) -> Result<Option<ReviewWorkspaceDocument>, CliError> {
    store
        .read_workspace()
        .map_err(|error| CliError::storage("read review workspace", error))
}

/// The workspace document and its selected run, read once and pinned by the caller.
#[review_kit_observation::operations]
#[helper]
pub fn selected(store: &RuntimeStore) -> Result<(ReviewWorkspaceDocument, RunId), CliError> {
    let document = workspace(store)?.ok_or_else(no_selected_run)?;
    let run_id = document.selected_run_id.clone();
    Ok((document, run_id))
}

/// The repository this worktree's runs are bound to, read under the workspace lock.
///
/// `review-workspace.json` names it. Without that file, such as after a crash
/// before the first selection, the binding is derived from every readable run
/// manifest; manifests naming different repositories are `repository_mismatch`.
/// `None` means no run exists yet, so the first run's subject binds the worktree.
#[review_kit_observation::operations]
#[helper]
pub fn repository_binding(
    lock: &WorkspaceLock,
    workspace: Option<&ReviewWorkspaceDocument>,
) -> Result<Option<RepositoryBinding>, CliError> {
    if let Some(workspace) = workspace {
        return Ok(Some(workspace.repository.clone()));
    }
    let store = lock.store();
    let listing = store
        .list_runs()
        .map_err(|error| CliError::storage("list review runs", error))?;
    let mut derived: Option<RepositoryBinding> = None;
    for run_id in listing.runs {
        let Ok(manifest) = store
            .open_run(&run_id)
            .and_then(|dir| dir.read_json::<ReviewRunDocument>(WorkingFile::Manifest.into()))
        else {
            continue;
        };
        let binding = RepositoryBinding::of(&manifest.subject);
        match &derived {
            Some(existing) if existing != &binding => {
                return Err(super::loading::repository_mismatch());
            }
            Some(_) => {}
            None => derived = Some(binding),
        }
    }
    Ok(derived)
}

/// Lock an existing runtime directory for a local mutation.
#[review_kit_observation::operations]
#[helper]
pub fn lock(store: &RuntimeStore) -> Result<WorkspaceLock, CliError> {
    store.lock().map_err(|error| match error {
        RunStorageError::Uninitialized => no_selected_run(),
        error => CliError::storage("lock review workspace", error),
    })
}

#[review_kit_observation::operations]
#[helper]
pub fn no_selected_run() -> CliError {
    CliError::state(
        "no_selected_run",
        2,
        "this worktree has no selected review run; start one with rkc run start --mr URL",
    )
}

/// The run directory relative to the worktree root, for command output.
#[review_kit_observation::operations]
#[helper]
pub fn run_directory(run_id: &RunId) -> String {
    format!(".review-kit/review-runs/{run_id}")
}
