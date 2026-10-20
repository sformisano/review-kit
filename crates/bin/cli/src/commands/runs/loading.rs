use review_kit_application::{conversation::Conversation, review_run::RunId};
use review_kit_filesystem_adapter::{
    RunDir, RunFile, RunStorageError, RuntimeStore, SnapshotFile, WorkingFile,
};
use review_kit_gitlab_adapter::RetainedSource;
use review_kit_json_adapter::{
    CONVERSATION_VERSION, ContextDocument, ConversationDocument,
    checkpoints::{RepositoryBinding, ReviewRunDocument, RunLifecycle},
};
use serde::Deserialize;
use serde_json::json;

use crate::error::CliError;

/// One run's manifest with its retained context and conversation decoded and bound to it.
#[derive(Debug)]
pub struct LoadedRun {
    pub dir: RunDir,
    pub manifest: ReviewRunDocument,
    pub context: ContextDocument,
    pub conversation: Conversation,
}

/// One run's manifest, bound to its directory and the workspace repository.
///
/// Retained files are not read; `run stop` and `status` work from this alone.
#[derive(Debug)]
pub struct StoredRun {
    pub dir: RunDir,
    pub manifest: ReviewRunDocument,
}

#[review_kit_observation::operations]
impl LoadedRun {
    #[helper]
    pub fn run_id(&self) -> &RunId {
        &self.manifest.run_id
    }
}

/// Open a run's manifest and check its identity and repository binding.
///
/// The manifest must name the directory's run ID, and its subject must belong to
/// `repository` when the workspace has one.
#[review_kit_observation::operations]
#[helper]
pub fn load_manifest(
    store: &RuntimeStore,
    repository: Option<&RepositoryBinding>,
    run_id: &RunId,
) -> Result<StoredRun, CliError> {
    let dir = store
        .open_run(run_id)
        .map_err(|error| CliError::storage("open review run", error))?;
    let manifest: ReviewRunDocument = dir
        .read_json(WorkingFile::Manifest.into())
        .map_err(|error| CliError::storage("read run manifest", error))?;
    if &manifest.run_id != run_id {
        return Err(CliError::state(
            "run_identity_mismatch",
            2,
            format!(
                "the manifest in run directory {run_id} belongs to run {}",
                manifest.run_id
            ),
        ));
    }
    if let Some(repository) = repository
        && !repository.binds(&manifest.subject)
    {
        return Err(repository_mismatch());
    }
    Ok(StoredRun { dir, manifest })
}

/// Open a run and check its identity, repository binding and retained files.
///
/// Beyond [`load_manifest`], the four retained files must match the manifest's
/// hashes, and context, source and conversation must describe the manifest's
/// subject, reviewer and revision.
#[review_kit_observation::operations]
#[helper]
pub fn load_run(
    store: &RuntimeStore,
    repository: Option<&RepositoryBinding>,
    run_id: &RunId,
) -> Result<LoadedRun, CliError> {
    let StoredRun { dir, manifest } = load_manifest(store, repository, run_id)?;
    verify_retained(&dir, &manifest)?;
    let context = retained_context(&dir, &manifest)?;
    let source = retained_source(&dir)?;
    if source.subject() != &manifest.subject || source.revision() != &manifest.revision {
        return Err(identity_mismatch(SnapshotFile::Source));
    }
    let conversation = retained_conversation(&dir, &manifest)?;
    Ok(LoadedRun {
        dir,
        manifest,
        context,
        conversation,
    })
}

/// `review-context.json`, decoded and bound to the manifest's subject, reviewer and revision.
#[review_kit_observation::operations]
#[helper]
fn retained_context(
    dir: &RunDir,
    manifest: &ReviewRunDocument,
) -> Result<ContextDocument, CliError> {
    let context: ContextDocument = dir
        .read_json(SnapshotFile::Context.into())
        .map_err(|error| CliError::storage("read retained context", error))?;
    let snapshot = context
        .snapshot
        .clone()
        .into_snapshot()
        .map_err(CliError::from)?;
    if snapshot.subject != manifest.subject
        || snapshot.reviewer != manifest.reviewer
        || snapshot.revision != manifest.revision
    {
        return Err(identity_mismatch(SnapshotFile::Context));
    }
    Ok(context)
}

/// `review-conversation.json`, decoded and bound to the manifest's subject.
///
/// A version newer than this build reads is `newer_run_document`, not damage.
#[review_kit_observation::operations]
#[helper]
fn retained_conversation(
    dir: &RunDir,
    manifest: &ReviewRunDocument,
) -> Result<Conversation, CliError> {
    if let Some(error) = newer_document(dir, SnapshotFile::Conversation, CONVERSATION_VERSION) {
        return Err(error);
    }
    let conversation = dir
        .read_json::<ConversationDocument>(SnapshotFile::Conversation.into())
        .map_err(|error| CliError::storage("read retained conversation", error))?
        .into_conversation()
        .map_err(CliError::from)?;
    if conversation.subject() != &manifest.subject {
        return Err(identity_mismatch(SnapshotFile::Conversation));
    }
    Ok(conversation)
}

/// `newer_run_document` when `file` declares a version above `supported`.
///
/// Only the `version` field is read, so a newer layout with unknown fields is
/// still recognised. `None` when the file is unreadable, has no numeric
/// version or is not newer; the ordinary decoder then reports it.
#[review_kit_observation::operations]
#[helper]
fn newer_document(dir: &RunDir, file: SnapshotFile, supported: u64) -> Option<CliError> {
    #[derive(Deserialize)]
    struct Declared {
        version: u64,
    }
    let bytes = dir.read_bytes(file.into()).ok()?;
    let version = serde_json::from_slice::<Declared>(&bytes).ok()?.version;
    (version > supported).then(|| {
        let name = file.name();
        CliError::state(
            "newer_run_document",
            2,
            format!("{name} version {version} was written by a newer rkc; use that rkc version or a later one for this run"),
        )
        .with_details(json!({"file": name, "version": version, "supported": supported}))
    })
}

/// Retained files that are missing, do not decode or describe another run, with their codes.
///
/// Hash differences alone are not listed; [`verify_retained`] reports them.
#[review_kit_observation::operations]
#[helper]
pub fn damaged_retained(stored: &StoredRun) -> Vec<(SnapshotFile, CliError)> {
    let StoredRun { dir, manifest } = stored;
    let mut damaged = Vec::new();
    if let Err(error) = retained_context(dir, manifest) {
        damaged.push((SnapshotFile::Context, error));
    }
    match retained_source(dir) {
        Ok(source)
            if source.subject() != &manifest.subject || source.revision() != &manifest.revision =>
        {
            damaged.push((
                SnapshotFile::Source,
                identity_mismatch(SnapshotFile::Source),
            ));
        }
        Ok(_) => {}
        Err(error) => damaged.push((SnapshotFile::Source, error)),
    }
    if let Err(error) = dir.read_bytes(SnapshotFile::Diff.into()) {
        damaged.push((
            SnapshotFile::Diff,
            CliError::storage("read retained diff", error),
        ));
    }
    if let Err(error) = retained_conversation(dir, manifest) {
        damaged.push((SnapshotFile::Conversation, error));
    }
    damaged
}

/// Compare the four retained files with the manifest's hashes.
#[review_kit_observation::operations]
#[helper]
pub fn verify_retained(dir: &RunDir, manifest: &ReviewRunDocument) -> Result<(), CliError> {
    dir.verify_snapshot(&manifest.snapshot_files)
        .map_err(|error| match error {
            RunStorageError::SnapshotMismatch { differences } => CliError::state(
                "hash_mismatch",
                2,
                "retained run files differ from the hashes recorded when the run started; restore the exact files or start a new run",
            )
            .with_details(json!({
                "files": differences.iter().map(|difference| difference.file.name()).collect::<Vec<_>>()
            })),
            error => CliError::storage("verify retained run files", error),
        })
}

/// The run's retained committed source, `reviewed-source.json`, read from the run root.
#[review_kit_observation::operations]
#[helper]
pub fn retained_source(dir: &RunDir) -> Result<RetainedSource, CliError> {
    let bytes = dir
        .read_bytes(RunFile::Snapshot(SnapshotFile::Source))
        .map_err(|error| CliError::storage("read retained source", error))?;
    RetainedSource::decode(&bytes).map_err(CliError::GitLab)
}

#[review_kit_observation::operations]
#[helper]
pub fn repository_mismatch() -> CliError {
    CliError::state(
        "repository_mismatch",
        2,
        "the merge request belongs to a different repository than this worktree's review runs",
    )
}

#[review_kit_observation::operations]
#[helper]
fn identity_mismatch(file: SnapshotFile) -> CliError {
    let file = file.name();
    CliError::state(
        "identity_mismatch",
        2,
        format!("{file} describes a different subject, reviewer or revision than the run manifest"),
    )
    .with_details(json!({"file": file}))
}

/// Reject edits and new publication of a completed or stopped run.
#[review_kit_observation::operations]
#[helper]
pub fn require_open(manifest: &ReviewRunDocument) -> Result<(), CliError> {
    match &manifest.lifecycle {
        RunLifecycle::Open => Ok(()),
        lifecycle => Err(CliError::state(
            "run_terminal",
            5,
            format!(
                "run {} is {}; terminal runs stay inspectable but cannot change; start a fresh run with rkc run start",
                manifest.run_id,
                lifecycle_label(lifecycle).0
            ),
        )),
    }
}

/// `status`, `completion` and `reason` as written in the manifest.
#[review_kit_observation::operations]
#[helper]
pub fn lifecycle_label(
    lifecycle: &RunLifecycle,
) -> (&'static str, Option<&'static str>, Option<&str>) {
    use review_kit_application::review_run::RunCompletion;
    match lifecycle {
        RunLifecycle::Open => ("open", None, None),
        RunLifecycle::Completed { completion, reason } => (
            "completed",
            Some(match completion {
                RunCompletion::Published => "published",
                RunCompletion::NoChanges => "no_changes",
                RunCompletion::LocalOnly => "local_only",
                RunCompletion::Abandoned => "abandoned",
            }),
            reason.as_deref(),
        ),
        RunLifecycle::Stopped { reason } => ("stopped", None, Some(reason)),
    }
}
