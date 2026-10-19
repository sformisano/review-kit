use review_kit_application::{
    review_run::{ContentRef, RunCompletion, SettledOutcome, SnapshotFiles},
    use_cases::publishing::{
        BindingError, ManagedHistory, OwnedHistory, PublicationProgress, RunPublication, SettledRun,
    },
};
use review_kit_filesystem_adapter::{
    FileError, FileProgressStore, JournalView, ProgressLock, RunDir, RunWriter, WorkingFile,
};
use review_kit_json_adapter::{
    EvidenceDocument,
    checkpoints::{ReviewRunDocument, RunLifecycle},
};
use serde_json::{Value, json};

use crate::{config::Config, error::CliError};

/// The run-bound journal as read without changing any file.
pub enum JournalReading {
    /// No configuration, lock or readable journal; the cause is kept for reporting.
    Unavailable(CliError),
    /// The subject/reviewer store has no review-key directory.
    Absent,
    Journal(Box<PublicationProgress>),
}

#[review_kit_observation::operations]
impl JournalReading {
    #[helper]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "unavailable",
            Self::Absent => "absent",
            Self::Journal(_) => "present",
        }
    }

    /// The journal's record of `run`, when a run-bound journal was read.
    #[helper]
    pub fn publication<'a>(&'a self, run: &ReviewRunDocument) -> Option<RunPublication<'a>> {
        match self {
            Self::Journal(progress) => Some(progress.run_publication(&run.run_id)),
            _ => None,
        }
    }

    /// The run that owns the journal's unfinished publication, if any.
    #[helper]
    pub fn owner(&self) -> Option<&review_kit_application::review_run::RunId> {
        match self {
            Self::Journal(progress) => progress
                .coverage_binding
                .as_ref()
                .filter(|binding| {
                    !progress
                        .settled_runs
                        .iter()
                        .any(|entry| entry.run_id == binding.run_id)
                })
                .map(|binding| &binding.run_id),
            _ => None,
        }
    }
}

/// Read the subject/reviewer journal without changing files.
#[review_kit_observation::operations]
#[helper]
pub fn read(store: &FileProgressStore) -> JournalReading {
    match store.inspect_journal() {
        Ok(JournalView::Absent) => JournalReading::Absent,
        Ok(JournalView::Journal(progress)) => JournalReading::Journal(progress),
        Err(error) => {
            JournalReading::Unavailable(CliError::journal("read publication journal", error))
        }
    }
}

/// Lock the subject/reviewer store, the second lock after the workspace lock.
#[review_kit_observation::operations]
#[helper]
pub fn lock(store: &FileProgressStore) -> Result<ProgressLock, CliError> {
    store
        .try_lock()
        .map_err(|error| CliError::journal("lock publication journal", error))
}

/// Settlement recorded by the journal, as manifest lifecycle.
#[review_kit_observation::operations]
#[helper]
pub fn settled_lifecycle(entry: &SettledRun) -> RunLifecycle {
    RunLifecycle::Completed {
        completion: RunCompletion::from(entry.outcome),
        reason: entry.reason.clone(),
    }
}

#[review_kit_observation::operations]
#[helper]
pub fn outcome_label(outcome: SettledOutcome) -> &'static str {
    match outcome {
        SettledOutcome::Published => "published",
        SettledOutcome::NoChanges => "no_changes",
        SettledOutcome::Abandoned => "abandoned",
    }
}

#[review_kit_observation::operations]
#[helper]
pub fn settlement_value(entry: &SettledRun) -> Value {
    json!({
        "run_id": entry.run_id.as_str(),
        "outcome": outcome_label(entry.outcome),
        "round": entry.round,
        "reason": entry.reason,
        "review_assessment": entry.review_assessment.as_str(),
    })
}

/// The journal's binding or settlement must name this run's accepted assessment and retained files.
///
/// A mismatch means the run directory is not the one the journal recorded, such
/// as a restored older copy; nothing may be repaired or resumed from it.
#[review_kit_observation::operations]
#[helper]
pub fn require_inputs(
    manifest: &ReviewRunDocument,
    review_assessment: &ContentRef,
    snapshot_files: &SnapshotFiles,
) -> Result<(), CliError> {
    if manifest.review_assessment.as_ref() == Some(review_assessment)
        && &manifest.snapshot_files == snapshot_files
    {
        return Ok(());
    }
    Err(CliError::state(
        "binding_mismatch",
        5,
        "the publication journal records different accepted inputs for this run; restore the exact run directory whose inputs it recorded",
    )
    .with_details(json!({"run_id": manifest.run_id.as_str()})))
}

/// Record a journal settlement in an open manifest before evaluating a command.
///
/// The settlement must name the run's accepted inputs (`binding_mismatch`
/// otherwise, with no change). Terminal manifests keep their lifecycle;
/// settlement never relabels them.
#[review_kit_observation::operations]
#[helper]
pub fn repair(
    writer: &RunWriter<'_>,
    manifest: &mut ReviewRunDocument,
    entry: &SettledRun,
) -> Result<bool, CliError> {
    require_inputs(manifest, &entry.review_assessment, &entry.snapshot_files)?;
    if manifest.lifecycle != RunLifecycle::Open {
        return Ok(false);
    }
    manifest.lifecycle = settled_lifecycle(entry);
    manifest.publication_attempted = true;
    writer
        .replace_json(WorkingFile::Manifest, manifest)
        .map_err(|error| CliError::storage("repair run manifest", error))?;
    Ok(true)
}

#[review_kit_observation::operations]
#[helper]
pub fn pending_owner(owner: &review_kit_application::review_run::RunId, own: bool) -> CliError {
    if own {
        return CliError::state(
            "publication_pending_owner",
            5,
            format!(
                "run {owner} owns unfinished publication; recover it with rkc publish or close it with rkc publish --abandon --reason TEXT before changing it"
            ),
        )
        .with_details(json!({"owner_run_id": owner.as_str()}));
    }
    CliError::state(
        "publication_pending_owner",
        5,
        format!(
            "run {owner} owns unfinished publication for this merge request and reviewer; this run cannot publish until it settles"
        ),
    )
    .with_details(json!({"owner_run_id": owner.as_str(), "next": owner_guidance(owner)}))
}

/// How a run that owns nothing gets past another run's unfinished publication.
#[review_kit_observation::operations]
#[helper]
pub fn owner_guidance(owner: &review_kit_application::review_run::RunId) -> String {
    format!(
        "Run {owner} owns the unfinished publication. Select it with rkc run select {owner}, then recover it with rkc publish and its original effects, or close its round with rkc publish --abandon --reason TEXT once no write is uncertain. Then select this run and publish it, or start a fresh run with rkc run start --new when the merge request changed."
    )
}

/// What to do when an attempted run's journal is missing.
pub const MISSING_GUIDANCE: &str = "Do not publish from this run until you restore the authoritative publication state: the review's directory under the configured publication_state, with its publication-progress.json. rkc never recreates it. After a transfer, continue only from the authoritative copy.";

/// Whether the run attempted publication: its manifest says so, or its own
/// execution events record a checkpoint, a write intent or a publication run.
///
/// Unreadable events prove nothing here; the command that opens them reports them.
#[review_kit_observation::operations]
#[helper]
pub fn attempted(dir: &RunDir, manifest: &ReviewRunDocument) -> bool {
    manifest.publication_attempted
        || dir
            .read_optional_bytes(WorkingFile::PublicationEvents.into())
            .ok()
            .flatten()
            .and_then(|bytes| review_kit_json_adapter::decode::<EvidenceDocument>(&bytes).ok())
            .is_some_and(|events| events.records_publication_attempt())
}

/// An attempted run whose run-bound journal is absent: its publication state
/// was moved or lost, and starting over could repeat its writes.
#[review_kit_observation::operations]
#[helper]
pub fn missing_for_attempted(store: &FileProgressStore) -> CliError {
    CliError::state(
        "journal_missing",
        5,
        "this run attempted publication, but the publication journal for its merge request and reviewer is missing",
    )
    .with_details(json!({"journal": store.journal_path(), "next": MISSING_GUIDANCE}))
}

/// Refuse a run without a journal when the provider history does not permit an empty one.
///
/// [`ManagedHistory::admits_empty_journal`] decides; planning and publication
/// both check it, so a plan refuses what its publication would refuse.
#[review_kit_observation::operations]
#[helper]
pub fn require_empty_start(
    reading: &JournalReading,
    store: &FileProgressStore,
    history: &ManagedHistory,
) -> Result<(), CliError> {
    if !matches!(reading, JournalReading::Absent) || history.admits_empty_journal() {
        return Ok(());
    }
    let (message, round, next) = match history.state() {
        OwnedHistory::Unfinished(round) => (
            format!(
                "the publication journal for this merge request and reviewer is missing, and GitLab shows round {round} of this reviewer's summary unfinished"
            ),
            Some(round),
            unfinished_guidance(round),
        ),
        _ => (
            "the publication journal for this merge request and reviewer is missing, and GitLab does not show one finished round in this reviewer's Review Kit records".to_owned(),
            None,
            "rkc starts an empty journal only when this reviewer has no Review Kit records on the merge request, or when the reviewer's only summary shows its latest round finished. Restore that review's directory under the configured publication_state, with its publication-progress.json, from the environment that published here.".to_owned(),
        ),
    };
    Err(
        CliError::state("journal_missing", 5, message).with_details(json!({
            "journal": store.journal_path(),
            "round": round,
            "next": next,
        })),
    )
}

/// The reviewer's unfinished provider round when this environment has no
/// journal for it: only the publication state that started the round can
/// finish it, and the provider history refuses an empty journal.
#[review_kit_observation::operations]
#[helper]
pub fn unfinished_without_journal(
    reading: &JournalReading,
    history: &ManagedHistory,
) -> Option<u64> {
    match (reading, history.state()) {
        (JournalReading::Absent, OwnedHistory::Unfinished(round)) => Some(round),
        _ => None,
    }
}

/// Why an unfinished provider round needs the publication state that started
/// it, and how to restore that state or leave the round to its environment.
#[review_kit_observation::operations]
#[helper]
pub fn unfinished_guidance(round: u64) -> String {
    format!(
        "Round {round} was started from publication state that is not here, and only that state holds the round's possible unresolved write and effect ceiling. rkc starts an empty journal only after the reviewer's latest round finished. If another environment is still publishing, let it finish, then start a fresh run here with rkc run start --new. Otherwise restore that review's directory under the configured publication_state, with its publication-progress.json, and the review run that started round {round}, then recover that run with rkc publish. Without that state rkc neither finishes nor closes round {round}."
    )
}

/// Guidance for a run that owns a binding none of whose writes could have reached the provider.
pub const UNSENT_GUIDANCE: &str = "No write of this run's publication binding could have reached the provider. If the reviewed source or the reviewer's own threads changed, start a fresh run with rkc run start --new and publish it; that publication releases this binding, after which this run can be stopped.";

/// The run keeps its binding although nothing was sent; only another run's publication releases it.
#[review_kit_observation::operations]
#[helper]
pub fn unsent_owner(owner: &review_kit_application::review_run::RunId) -> CliError {
    CliError::state(
        "publication_pending_owner",
        5,
        format!("run {owner} owns a publication binding with no possible provider write; {UNSENT_GUIDANCE}"),
    )
    .with_details(json!({"owner_run_id": owner.as_str(), "provider_write_possible": false}))
}

#[review_kit_observation::operations]
#[helper]
pub fn unavailable(cause: CliError) -> CliError {
    CliError::state(
        "journal_unavailable",
        5,
        "this run has attempted publication, but its publication journal is unavailable; restore the configured publication state before changing the run",
    )
    .with_details(json!({"cause": cause.document()["diagnostics"][0].clone()}))
}

/// Outcome of reconciling a local mutation with the journal.
pub struct Reconciled {
    /// The journal settlement that repaired the manifest in this command.
    pub repaired: Option<SettledRun>,
    pub journal: &'static str,
}

/// Consult the journal before an accepted-input or lifecycle mutation of `manifest`.
///
/// A settled run's open manifest is repaired first. A run owning unfinished
/// publication is rejected. Without a readable journal, a run that never attempted
/// publication may continue; one that did, by its manifest or its own events, is
/// rejected because only the journal can show its outcome.
#[review_kit_observation::operations]
#[helper]
pub fn reconcile_local(
    config: Option<&Config>,
    writer: &RunWriter<'_>,
    manifest: &mut ReviewRunDocument,
) -> Result<Reconciled, CliError> {
    let reading = match config {
        Some(config) => {
            let store = config.run_store(&manifest.subject, &manifest.reviewer);
            match lock(&store) {
                Ok(_guard) => read(&store),
                Err(error) if is_busy(&error) => return Err(error),
                Err(error) => JournalReading::Unavailable(error),
            }
        }
        None => JournalReading::Unavailable(CliError::MissingConfig),
    };
    let journal = reading.label();
    let attempted = attempted(writer, manifest);
    let unavailable_for_attempted = |cause: CliError| {
        if attempted {
            Err(unavailable(cause))
        } else {
            Ok(Reconciled {
                repaired: None,
                journal,
            })
        }
    };
    match reading {
        JournalReading::Journal(progress) => match progress.run_publication(&manifest.run_id) {
            RunPublication::Settled(entry) => {
                let entry = entry.clone();
                let repaired = repair(writer, manifest, &entry)?;
                Ok(Reconciled {
                    repaired: repaired.then_some(entry),
                    journal,
                })
            }
            RunPublication::BoundUnsettled(binding) => Err(pending_owner(&binding.run_id, true)),
            RunPublication::BoundUnsent(binding) => Err(unsent_owner(&binding.run_id)),
            RunPublication::Unbound => Ok(Reconciled {
                repaired: None,
                journal,
            }),
        },
        JournalReading::Unavailable(cause) => unavailable_for_attempted(cause),
        JournalReading::Absent => unavailable_for_attempted(CliError::journal(
            "read publication journal",
            FileError::MissingProgress,
        )),
    }
}

/// The stable diagnostic code of a journal binding refusal.
#[review_kit_observation::operations]
#[helper]
pub fn binding_code(error: &BindingError) -> &'static str {
    match error {
        BindingError::PendingOwner(_) => "publication_pending_owner",
        BindingError::Mismatch(_) => "binding_mismatch",
        BindingError::AlreadySettled(_) => "already_settled",
        BindingError::SettlementConflict(_) => "settlement_conflict",
        BindingError::BindingRequired => "binding_required",
        BindingError::Unresolved => "unresolved_write",
        BindingError::MissingReason => "invalid_abandonment_reason",
    }
}

#[review_kit_observation::operations]
#[helper]
fn is_busy(error: &CliError) -> bool {
    matches!(
        error,
        CliError::Adapter {
            code: "publication_busy",
            ..
        }
    )
}
