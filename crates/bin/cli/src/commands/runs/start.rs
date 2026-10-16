use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::Snapshot,
    review_run::{RunId, SnapshotFiles},
    use_cases::discovery_bundle::discover_bundle,
};
use review_kit_domain::{FindingId, ReviewerId, Subject};
use review_kit_filesystem_adapter::{RunFile, SnapshotFile, WorkingFile, WorkspaceLock};
use review_kit_gitlab_adapter::GitLab;
use review_kit_json_adapter::{
    ContextDocument, ConversationDocument,
    checkpoints::{
        AssessmentDocument, RepositoryBinding, ReviewRunDocument, ReviewWorkspaceDocument,
        RunLifecycle, UtcTimestamp,
    },
};
use serde_json::json;

use super::{
    chain::reference,
    environment::{self, run_directory},
    loading::{lifecycle_label, repository_mismatch},
};
use crate::{
    commands::{CommandOptions, CommandOutput, history, shared::merge_request_url},
    error::CliError,
};

/// Capture the merge request afresh, install a new run and select it.
///
/// The repository binding is checked under the workspace lock and written only
/// by the worktree's first run. The run directory is complete and durable before
/// `review-workspace.json` selects it. A failed capture leaves the selection
/// unchanged.
#[review_kit_observation::operations]
#[operation(name = "cli.run.start", input = crate::logging::command_input("cli.run.start", options).flag("explicit_mr", mr.is_some()).flag("new", new), output = crate::logging::command_result)]
pub fn execute(
    options: &CommandOptions,
    mr: Option<&str>,
    new: bool,
) -> Result<CommandOutput, CliError> {
    let config = options.load()?;
    let store = environment::locate()?;
    let workspace = environment::workspace(&store)?;
    let url = match (mr, &workspace) {
        (Some(url), _) => url.to_owned(),
        (None, Some(workspace)) => {
            let selected = store
                .open_run(&workspace.selected_run_id)
                .and_then(|dir| dir.read_json::<ReviewRunDocument>(WorkingFile::Manifest.into()))
                .map_err(|error| CliError::storage("read selected run", error))?;
            merge_request_url(&selected.subject)?
        }
        (None, None) => return Err(environment::no_selected_run()),
    };
    let gitlab = config.connect(&url)?;
    let lock = store
        .create_and_lock()
        .map_err(|error| CliError::storage("lock review workspace", error))?;
    let workspace = environment::workspace(lock.store())?;
    let repository = environment::repository_binding(&lock, workspace.as_ref())?;
    if let Some(repository) = &repository
        && !repository.binds(gitlab.subject())
    {
        return Err(repository_mismatch());
    }
    let open = open_runs(&lock, gitlab.subject(), gitlab.reviewer())?;
    if !open.is_empty() && !new {
        return Err(CliError::state(
            "open_run_exists",
            5,
            "an open run already exists for this merge request and reviewer; select and continue it, or pass --new to start another run",
        )
        .with_details(json!({"run_ids": open.iter().map(RunId::as_str).collect::<Vec<_>>()})));
    }
    let captured = capture(&gitlab)?;
    let run_id = install(&lock, &captured)?;
    fault_point!("run_start.after_install");
    lock.write_workspace(&ReviewWorkspaceDocument {
        repository: repository.unwrap_or_else(|| RepositoryBinding::of(&captured.manifest_subject)),
        selected_run_id: run_id.clone(),
    })
    .map_err(|error| CliError::storage("select new run", error))?;
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "run start",
        "status": "created",
        "run_id": run_id.as_str(),
        "subject": captured.context.snapshot.subject,
        "reviewer": captured.reviewer.as_str(),
        "revision": captured.context.snapshot.revision,
        "run_directory": run_directory(&run_id),
        "carried_findings": captured.carried_findings,
        "outdated_positions": captured.outdated_positions.iter().map(|id| id.get()).collect::<Vec<_>>(),
        "next_finding_id": captured.next_finding_id,
        "other_open_runs": open.iter().map(RunId::as_str).collect::<Vec<_>>(),
        "draft": super::chain::DRAFT_FILE,
        "next": "Complete review-assessment.draft.json, then run rkc accept.",
    })))
}

/// Open runs of the same subject and reviewer; unreadable manifests are skipped.
#[review_kit_observation::operations]
#[helper]
fn open_runs(
    lock: &WorkspaceLock,
    subject: &Subject,
    reviewer: &ReviewerId,
) -> Result<Vec<RunId>, CliError> {
    let listing = lock
        .store()
        .list_runs()
        .map_err(|error| CliError::storage("list review runs", error))?;
    let mut open = Vec::new();
    for run_id in listing.runs {
        let Ok(manifest) = lock
            .store()
            .open_run(&run_id)
            .and_then(|dir| dir.read_json::<ReviewRunDocument>(WorkingFile::Manifest.into()))
        else {
            continue;
        };
        if &manifest.subject == subject
            && &manifest.reviewer == reviewer
            && lifecycle_label(&manifest.lifecycle).0 == "open"
        {
            open.push(run_id);
        }
    }
    Ok(open)
}

/// Bytes captured for a new run, in `SnapshotFile::ALL` order.
struct Captured {
    manifest_subject: Subject,
    reviewer: ReviewerId,
    context: ContextDocument,
    revision: review_kit_domain::Revision,
    files: [(SnapshotFile, Vec<u8>); 4],
    carried_findings: usize,
    next_finding_id: u64,
    outdated_positions: Vec<FindingId>,
}

struct CaptureReader<'a> {
    gitlab: &'a GitLab,
    outdated: Vec<FindingId>,
}

#[review_kit_observation::operations]
impl ReviewReader for CaptureReader<'_> {
    #[helper]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
        if subject != self.gitlab.subject() {
            return Err(PortError::new(
                "GitLab discovery",
                review_kit_gitlab_adapter::GitLabError::Identity,
            ));
        }
        let (snapshot, outdated) = self
            .gitlab
            .discover_with_outdated_positions()
            .map_err(|error| PortError::new("GitLab discovery", error))?;
        self.outdated = outdated;
        Ok(snapshot)
    }
}

/// Read discovery, committed source, diff and supported conversation.
#[review_kit_observation::operations]
#[operation(name = "cli.discover", input = review_kit_observation::Fields::new(), output = |result| crate::logging::result(result, |captured| review_kit_observation::Fields::new().count("carried_findings", captured.carried_findings)))]
fn capture(gitlab: &GitLab) -> Result<Captured, CliError> {
    let mut reader = CaptureReader {
        gitlab,
        outdated: Vec::new(),
    };
    let mut changes = gitlab.clone();
    let bundle = discover_bundle(
        &mut reader,
        &mut changes,
        gitlab.subject(),
        gitlab.reviewer(),
    )
    .map_err(|error| history::bundle_error(error, gitlab))?;
    let revision = bundle.snapshot().revision.clone();
    let source = gitlab
        .capture_source(gitlab.subject(), &revision)
        .map_err(CliError::GitLab)?;
    let conversation = gitlab
        .capture_conversation(gitlab.subject())
        .map_err(CliError::GitLab)?;
    let context = ContextDocument::new(bundle.snapshot(), bundle.changes());
    let conversation = ConversationDocument::new(&conversation, utc_now()?.as_str().to_owned())?;
    Ok(Captured {
        manifest_subject: gitlab.subject().clone(),
        reviewer: gitlab.reviewer().clone(),
        files: [
            (
                SnapshotFile::Context,
                review_kit_json_adapter::encode(&context)?,
            ),
            (
                SnapshotFile::Source,
                source.encode().map_err(CliError::GitLab)?,
            ),
            (
                SnapshotFile::Diff,
                bundle.changes().unified_diff.as_bytes().to_vec(),
            ),
            (
                SnapshotFile::Conversation,
                review_kit_json_adapter::encode(&conversation)?,
            ),
        ],
        context,
        revision,
        carried_findings: bundle.findings().len(),
        next_finding_id: bundle.next_finding_id().get(),
        outdated_positions: reader.outdated,
    })
}

/// Stage the run completely, then install it under `review-runs/`.
#[review_kit_observation::operations]
#[operation(name = "cli.init", input = review_kit_observation::Fields::new(), output = |result| crate::logging::result(result, |_| review_kit_observation::Fields::new()))]
fn install(lock: &WorkspaceLock, captured: &Captured) -> Result<RunId, CliError> {
    let run_id = RunId::try_from(uuid::Uuid::new_v4().to_string())
        .map_err(|error| CliError::adapter("generate run ID", "filesystem", 7, error))?;
    let [context, source, diff, conversation] = &captured.files;
    let manifest = ReviewRunDocument {
        run_id: run_id.clone(),
        subject: captured.manifest_subject.clone(),
        reviewer: captured.reviewer.clone(),
        revision: captured.revision.clone(),
        created_at: utc_now()?,
        lifecycle: RunLifecycle::Open,
        snapshot_files: SnapshotFiles {
            context: reference(&context.1)?,
            source: reference(&source.1)?,
            diff: reference(&diff.1)?,
            conversation: reference(&conversation.1)?,
        },
        review_assessment: None,
        publication_attempted: false,
    };
    let staged = lock
        .stage_run(run_id.clone())
        .map_err(|error| CliError::storage("stage review run", error))?;
    for (file, bytes) in &captured.files {
        staged
            .write(RunFile::Snapshot(*file), bytes)
            .map_err(|error| CliError::storage("write retained run file", error))?;
    }
    let mut draft = AssessmentDocument::initial(&run_id, &captured.context)?;
    if !captured.outdated_positions.is_empty() {
        let ids = captured
            .outdated_positions
            .iter()
            .take(50)
            .map(|id| format!("F{}", id.get()))
            .collect::<Vec<_>>()
            .join(", ");
        draft.local_context = format!(
            "Outdated comment positions: {ids}. Reassess these findings against the captured source. Their retained versions and creation locations are unchanged; old suggestions are outdated. See run start output for the complete ID list."
        );
    }
    staged
        .write_json(WorkingFile::Draft.into(), &draft)
        .map_err(|error| CliError::storage("write assessment draft", error))?;
    staged
        .write_json(WorkingFile::Manifest.into(), &manifest)
        .map_err(|error| CliError::storage("write run manifest", error))?;
    staged
        .install()
        .map_err(|error| CliError::storage("install review run", error))?;
    Ok(run_id)
}

/// The current UTC time at second precision, as written in run documents.
#[review_kit_observation::operations]
#[helper]
pub fn utc_now() -> Result<UtcTimestamp, CliError> {
    let now = time::OffsetDateTime::now_utc();
    let text = now
        .replace_nanosecond(0)
        .map_err(|error| CliError::adapter("read clock", "clock", 7, error))?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| CliError::adapter("format timestamp", "clock", 7, error))?;
    UtcTimestamp::try_from(text)
        .map_err(|error| CliError::adapter("format timestamp", "clock", 7, error))
}
