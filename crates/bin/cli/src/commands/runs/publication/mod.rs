//! Planning, publication, recovery and abandonment of the selected run.
//!
//! Commands hold the workspace lock, then the subject/reviewer journal lock. The
//! journal is read before provider discovery to decide between a settled run, a
//! recovery of this run's unfinished publication and a new publication.

mod abandon;
mod effects;
mod guidance;
mod plan;
mod preflight;
mod publish;

use review_kit_application::{
    publication::{Effects, Snapshot},
    use_cases::{
        publication_run::ExecutionEvidence,
        publishing::{
            BindingRequest, PublicationProgress, RunBinding, RunPublication, SettledRun,
            check_review_with_progress,
        },
    },
};
use review_kit_filesystem_adapter::{FileEvidence, FileProgressStore, RunWriter, WorkspaceLock};
use review_kit_gitlab_adapter::{GitLab, validate_plan_bodies};
use review_kit_json_adapter::Diagnostic;
use review_kit_json2md_adapter::Json2MdRenderer;
use serde_json::{Value, json};

use super::{
    chain::AcceptedReview,
    environment,
    journal::{self, JournalReading, settlement_value},
    loading::{LoadedRun, lifecycle_label, load_run, retained_source},
};
use crate::{
    args::UncertainRetry,
    commands::{CommandOptions, CommandOutput, shared::merge_request_url},
    config::Config,
    error::CliError,
};

/// What the publish command was asked to do.
pub enum Request {
    Plan(Effects),
    Publish(Effects, UncertainRetry),
    Abandon(Effects, String),
}

#[review_kit_observation::operations]
impl Request {
    #[helper]
    fn mode(&self) -> &'static str {
        match self {
            Self::Plan(_) => "plan",
            Self::Publish(..) => "publish",
            Self::Abandon(..) => "abandon",
        }
    }
}

/// The selected run's relation to the journal, read before provider discovery.
pub enum Role {
    Settled(SettledRun),
    /// This run owns the journal's unfinished publication.
    Recovery {
        binding: RunBinding,
        dispatched: bool,
        /// None of the binding's writes could have reached the provider.
        unsent: bool,
    },
    New,
}

/// State shared by the plan, publish and abandon paths.
pub struct Session {
    pub config: Config,
    pub lock: WorkspaceLock,
    pub run: LoadedRun,
    pub store: FileProgressStore,
    pub reading: JournalReading,
}

/// Plan, publish, recover or abandon the selected run.
#[review_kit_observation::operations]
#[operation(name = "cli.publish.run", input = crate::logging::command_input("cli.publish.run", options).label("mode", request.mode()), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions, request: Request) -> Result<CommandOutput, CliError> {
    let config = options.load()?;
    let store = environment::locate()?;
    let lock = environment::lock(&store)?;
    let (workspace, run_id) = environment::selected(lock.store())?;
    let run = load_run(lock.store(), Some(&workspace.repository), &run_id)?;
    let progress_store = config.run_store(&run.manifest.subject, &run.manifest.reviewer);
    let _journal_lock = journal::lock(&progress_store)?;
    let reading = journal::read(&progress_store);
    let role = role(&reading, &run)?;
    if matches!(reading, JournalReading::Absent) && journal::attempted(&run.dir, &run.manifest) {
        return Err(journal::missing_for_attempted(&progress_store));
    }
    let mut session = Session {
        config,
        lock,
        run,
        store: progress_store,
        reading,
    };
    if let Role::Settled(entry) = &role {
        return settled(&mut session, entry, request.mode());
    }
    match request {
        Request::Plan(effects) => plan::execute(&mut session, role, effects),
        Request::Publish(effects, retry) => publish::execute(&mut session, role, effects, retry),
        Request::Abandon(effects, reason) => abandon::execute(&mut session, role, effects, &reason),
    }
}

/// Classify the run from the journal as read, rejecting unavailable state.
#[review_kit_observation::operations]
#[helper]
fn role(reading: &JournalReading, run: &LoadedRun) -> Result<Role, CliError> {
    match reading {
        JournalReading::Unavailable(error) => Err(clone_error(error)),
        JournalReading::Absent => Ok(Role::New),
        JournalReading::Journal(progress) => match progress.run_publication(run.run_id()) {
            RunPublication::Settled(entry) => Ok(Role::Settled(entry.clone())),
            RunPublication::BoundUnsettled(binding) => {
                require_binding(run, binding)?;
                Ok(Role::Recovery {
                    binding: binding.clone(),
                    dispatched: progress.dispatch_may_have_started,
                    unsent: false,
                })
            }
            RunPublication::BoundUnsent(binding) => {
                require_binding(run, binding)?;
                Ok(Role::Recovery {
                    binding: binding.clone(),
                    dispatched: false,
                    unsent: true,
                })
            }
            RunPublication::Unbound => Ok(Role::New),
        },
    }
}

/// A journal failure is reported with its own code; its cause chain stays in the document.
#[review_kit_observation::operations]
#[helper]
fn clone_error(error: &CliError) -> CliError {
    let diagnostic = error.document()["diagnostics"][0].clone();
    let code = match diagnostic["code"].as_str() {
        Some("journal_missing") => "journal_missing",
        Some("publication_busy") => "publication_busy",
        _ => "journal_unavailable",
    };
    CliError::state(
        code,
        5,
        diagnostic["message"]
            .as_str()
            .unwrap_or("publication journal is unavailable")
            .to_owned(),
    )
    .with_details(json!({"causes": diagnostic["causes"].clone()}))
}

/// The journal's binding for this run must name the run's accepted inputs.
#[review_kit_observation::operations]
#[helper]
pub fn require_binding(run: &LoadedRun, binding: &RunBinding) -> Result<(), CliError> {
    journal::require_inputs(
        &run.manifest,
        &binding.review_assessment,
        &binding.snapshot_files,
    )
    .map_err(|error| error.with_detail("round", json!(binding.round)))
}

/// The binding request naming this run's accepted assessment and retained files.
#[review_kit_observation::operations]
#[helper]
pub fn binding_request(run: &LoadedRun, chain: &AcceptedReview) -> BindingRequest {
    BindingRequest {
        run_id: run.run_id().clone(),
        review_assessment: chain.assessment.clone(),
        snapshot_files: run.manifest.snapshot_files.clone(),
    }
}

/// Report a settled run; publication and abandonment record the settlement in an open manifest.
#[review_kit_observation::operations]
#[helper]
fn settled(
    session: &mut Session,
    entry: &SettledRun,
    mode: &str,
) -> Result<CommandOutput, CliError> {
    let repaired = if mode == "plan" {
        journal::require_inputs(
            &session.run.manifest,
            &entry.review_assessment,
            &entry.snapshot_files,
        )?;
        false
    } else {
        let writer = writer(&session.lock, &session.run)?;
        journal::repair(&writer, &mut session.run.manifest, entry)?
    };
    let (status, completion, reason) = lifecycle_label(&session.run.manifest.lifecycle);
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "publish",
        "mode": mode,
        "run_id": session.run.run_id().as_str(),
        "status": "already_settled",
        "settlement": settlement_value(entry),
        "repaired": repaired,
        "run_status": status,
        "completion": completion,
        "reason": reason,
        "provider_writes": false,
        "next": "This run's publication is settled; start a fresh review with rkc run start for later changes.",
    })))
}

#[review_kit_observation::operations]
#[helper]
pub fn writer<'lock>(
    lock: &'lock WorkspaceLock,
    run: &LoadedRun,
) -> Result<RunWriter<'lock>, CliError> {
    lock.run(run.run_id())
        .map_err(|error| CliError::storage("open review run", error))
}

/// Connect to the run's merge request as the authenticated reviewer recorded by the run.
#[review_kit_observation::operations]
#[helper]
pub fn connect(config: &Config, run: &LoadedRun) -> Result<GitLab, CliError> {
    let gitlab = config.connect(&merge_request_url(&run.manifest.subject)?)?;
    if gitlab.reviewer() != &run.manifest.reviewer {
        return Err(CliError::WrongReviewer);
    }
    if gitlab.subject() != &run.manifest.subject {
        return Err(super::loading::repository_mismatch());
    }
    Ok(gitlab)
}

/// Check the accepted request offline with the selected renderer before connecting.
#[review_kit_observation::operations]
#[helper]
pub fn check_offline(
    run: &LoadedRun,
    chain: &AcceptedReview,
    renderer: &mut Json2MdRenderer,
    progress: &PublicationProgress,
) -> Result<(), CliError> {
    let snapshot: Snapshot = run.context.snapshot.clone().into_snapshot()?;
    let mut source = retained_source(&run.dir)?;
    let preview =
        check_review_with_progress(&chain.request, &snapshot, &mut source, renderer, progress)
            .map_err(|errors| {
                CliError::Assessment(
                    errors
                        .iter()
                        .flat_map(Diagnostic::from_plan_error)
                        .collect(),
                )
            })?;
    validate_plan_bodies(&preview)
        .map_err(|error| CliError::adapter("validate GitLab bodies", "gitlab_body", 2, error))
}

/// Progress used for offline checks: another run's unresolved intent is not this run's.
#[review_kit_observation::operations]
#[helper]
pub fn own_progress(reading: &JournalReading, role: &Role) -> PublicationProgress {
    let mut progress = match reading {
        JournalReading::Journal(progress) => progress.as_ref().clone(),
        JournalReading::Absent | JournalReading::Unavailable(_) => PublicationProgress::default(),
    };
    if !matches!(role, Role::Recovery { .. }) {
        progress.unresolved = None;
    }
    progress
}

/// Execution evidence written to the run's `review-publication-events.json`
/// through run storage under the workspace lock.
#[review_kit_observation::operations]
#[helper]
pub fn evidence<'lock>(
    lock: &'lock WorkspaceLock,
    run: &LoadedRun,
    store: &FileProgressStore,
    renderer: &Json2MdRenderer,
) -> Result<impl ExecutionEvidence + use<'lock>, CliError> {
    let evidence = FileEvidence::for_run(writer(lock, run)?, store)
        .map_err(|error| CliError::evidence("initialize execution evidence", error))?
        .with_profile(renderer.identity());
    #[cfg(feature = "fault-injection")]
    let evidence = crate::faults::FaultEvidence::new(evidence);
    Ok(evidence)
}

/// Common fields of every publish output document.
#[review_kit_observation::operations]
#[helper]
pub fn output(session: &Session, mode: &str, profile: &Value) -> Value {
    let (status, completion, reason) = lifecycle_label(&session.run.manifest.lifecycle);
    json!({
        "version": 1,
        "command": "publish",
        "mode": mode,
        "run_id": session.run.run_id().as_str(),
        "assessment": session.run.manifest.review_assessment.as_ref().map(|reference| reference.as_str()),
        "render_profile": profile,
        "progress": session.store.journal_path(),
        "run_status": status,
        "completion": completion,
        "reason": reason,
    })
}
