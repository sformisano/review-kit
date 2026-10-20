//! Current run journal bootstrap, ownership, settlement retention and checkpoints.

#[path = "../runs/harness.rs"]
#[allow(dead_code)]
mod evidence_run;

use super::{Directory, Renderer, Source, fixture};
use std::path::{Path, PathBuf};

use review_kit_application::{
    ports::{PortError, PublicationHost, PublicationSession, ReviewReader, WriteOutcome},
    publication::{
        Abandonment, Effects, ProviderId, Round, RoundStatus, Snapshot, SummaryRecord, Write,
    },
    review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles},
    use_cases::{
        publication_run::{
            ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run_bound,
        },
        publishing::{
            BindingRequest, ManagedHistory, PublicationProgress, PublishRequest, RunBinding,
            SettledRun, Stop, UnresolvedWrite,
        },
    },
};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_filesystem_adapter::{
    FileError, FileEvidence, FileProgressStore, JournalOrigin, JournalView, SystemClock, Workspace,
};
use review_kit_json_adapter::{ProgressDocument, encode};

struct Stores {
    directory: Directory,
    journal: FileProgressStore,
    workspace: Workspace,
    snapshot: Snapshot,
    request: PublishRequest,
}

impl Stores {
    fn new() -> Self {
        let directory = Directory::new();
        let (snapshot, _, request) = fixture();
        let root = directory.0.path().join("shared");
        Self {
            journal: FileProgressStore::new(&root, &snapshot.subject, &snapshot.reviewer),
            workspace: Workspace::new(directory.0.path().join("revision")),
            directory,
            snapshot,
            request,
        }
    }

    fn key_directory(&self) -> PathBuf {
        self.journal.journal_path().parent().unwrap().to_owned()
    }

    fn no_history(&self) -> ManagedHistory {
        ManagedHistory::observe(&self.snapshot)
    }

    /// The reviewer's only summary shows round 1 in `status`.
    fn history(&self, status: RoundStatus) -> ManagedHistory {
        let mut snapshot = self.snapshot.clone();
        let mut owned = summary(&self.request);
        owned.current.status = status;
        if status == RoundStatus::Abandoned {
            owned.current.abandonment = Some(Abandonment {
                reason: "Replaced by a corrected review.".into(),
                verified_actions: Vec::new(),
            });
        }
        snapshot.summaries.push(owned);
        ManagedHistory::observe(&snapshot)
    }

    /// Two owned summaries: the provider shows no single round.
    fn ambiguous_history(&self) -> ManagedHistory {
        let mut snapshot = self.snapshot.clone();
        let mut second = summary(&self.request);
        second.id = ProviderId::new("13".into()).unwrap();
        snapshot.summaries.push(summary(&self.request));
        snapshot.summaries.push(second);
        ManagedHistory::observe(&snapshot)
    }

    fn bytes(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
}

fn round(request: &PublishRequest) -> Round {
    Round {
        confirmed_validation: false,
        verified_resolutions: Vec::new(),
        review_workflow: None,
        policy_set: None,
        policy_names: request.policies.policies().to_vec(),
        policy_declarations: request.policy_declarations.clone(),
        renderer: None,
        communicated_findings: None,
        number: 1,
        assessment: request.assessment.clone(),
        status: RoundStatus::Pending,
        abandonment: None,
        publication_limitation: None,
        answers: Vec::new(),
        transitions: Vec::new(),
        answer_targets: Vec::new(),
        actions: Vec::new(),
    }
}

fn summary(request: &PublishRequest) -> SummaryRecord {
    SummaryRecord {
        id: ProviderId::new("12".into()).unwrap(),
        author: request.assessment.reviewer().clone(),
        body: "Pending review".into(),
        current: round(request),
        history: Vec::new(),
    }
}

fn creation(request: &PublishRequest) -> Write {
    Write::Summary {
        expected: None,
        round: Box::new(round(request)),
        history: Vec::new(),
        body: "summary".into(),
    }
}

fn unresolved(snapshot: &Snapshot, request: &PublishRequest) -> UnresolvedWrite {
    UnresolvedWrite {
        subject: snapshot.subject.clone(),
        reviewer: snapshot.reviewer.clone(),
        write: creation(request),
        renderer: None,
        phase: Default::default(),
        baseline: None,
    }
}

fn content(digit: char) -> ContentRef {
    ContentRef::from_hex(&digit.to_string().repeat(64)).unwrap()
}

fn binding_request(run: u8) -> BindingRequest {
    BindingRequest {
        run_id: RunId::try_from(format!("00000000-0000-4000-8000-{run:012}")).unwrap(),
        review_assessment: content('b'),
        snapshot_files: SnapshotFiles {
            context: content('1'),
            source: content('2'),
            diff: content('3'),
            conversation: content('4'),
        },
        write_scope: None,
    }
}

fn binding(run: u8) -> RunBinding {
    let request = binding_request(run);
    RunBinding {
        run_id: request.run_id,
        round: 1,
        review_assessment: request.review_assessment,
        snapshot_files: request.snapshot_files,
        prior_authorization: None,
    }
}

fn settlement(run: u8) -> SettledRun {
    let binding = binding(run);
    SettledRun {
        run_id: binding.run_id,
        review_assessment: binding.review_assessment,
        snapshot_files: binding.snapshot_files,
        round: 1,
        outcome: SettledOutcome::Published,
        reason: None,
        verified_at: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(60),
    }
}

#[test]
fn a_missing_current_journal_with_unrelated_progress_is_not_adopted() {
    let stores = Stores::new();
    std::fs::create_dir_all(stores.key_directory()).unwrap();
    let bytes = encode(&ProgressDocument::new(
        &stores.snapshot.subject,
        &stores.snapshot.reviewer,
        &PublicationProgress::default(),
    ))
    .unwrap();
    std::fs::write(stores.journal.path(), &bytes).unwrap();
    for result in [
        stores.journal.inspect_journal().map(|_| ()),
        stores
            .journal
            .open_journal(&stores.no_history())
            .map(|_| ()),
        stores.journal.load_journal().map(|_| ()),
        stores.journal.save_journal(&PublicationProgress::default()),
    ] {
        assert!(
            matches!(result, Err(FileError::MissingProgress)),
            "{result:?}"
        );
    }
    assert_eq!(stores.bytes(&stores.journal.path()).unwrap(), bytes);
    assert!(!stores.journal.journal_path().exists());
}

#[test]
fn a_current_journal_ignores_unrelated_files_without_decoding_or_removing_them() {
    let stores = Stores::new();
    let progress = stores
        .journal
        .open_journal(&stores.no_history())
        .unwrap()
        .progress;
    let unrelated = b"not a journal or valid JSON";
    std::fs::write(stores.journal.path(), unrelated).unwrap();
    assert_eq!(
        stores.journal.inspect_journal().unwrap(),
        JournalView::Journal(Box::new(progress.clone()))
    );
    assert_eq!(
        stores
            .journal
            .open_journal(&stores.no_history())
            .unwrap()
            .origin,
        JournalOrigin::Loaded
    );
    assert_eq!(stores.journal.load_journal().unwrap(), progress);
    stores.journal.save_journal(&progress).unwrap();
    assert_eq!(stores.bytes(&stores.journal.path()).unwrap(), unrelated);
}

#[test]
fn generic_progress_apis_refuse_an_existing_current_journal_without_changes() {
    let stores = Stores::new();
    stores.journal.open_journal(&stores.no_history()).unwrap();
    let before = stores.bytes(&stores.journal.journal_path());
    assert!(matches!(
        stores.journal.load(&stores.workspace),
        Err(FileError::RunJournalExists)
    ));
    assert!(
        stores
            .journal
            .save(&PublicationProgress::default())
            .is_err()
    );
    assert_eq!(stores.bytes(&stores.journal.journal_path()), before);
    assert!(!stores.journal.path().exists());
}

#[test]
fn first_use_bootstraps_only_without_a_key_directory_or_an_unfinished_round() {
    // D19: an empty journal needs an absent store and provider history that
    // shows no owned state or a finished latest round.
    let stores = Stores::new();
    let _lock = stores.journal.try_lock().unwrap();
    assert_eq!(
        stores.journal.inspect_journal().unwrap(),
        JournalView::Absent
    );
    for history in [
        stores.history(RoundStatus::Pending),
        stores.history(RoundStatus::Partial),
        stores.ambiguous_history(),
    ] {
        assert!(
            matches!(
                stores.journal.open_journal(&history),
                Err(FileError::MissingProgress)
            ),
            "{:?}",
            history.state()
        );
    }
    let mut other = stores.snapshot.clone();
    other.reviewer = ReviewerId::try_from("10".to_owned()).unwrap();
    assert!(matches!(
        stores
            .journal
            .open_journal(&ManagedHistory::observe(&other)),
        Err(FileError::EvidenceBinding)
    ));
    assert!(!stores.key_directory().exists());

    let opened = stores.journal.open_journal(&stores.no_history()).unwrap();

    assert_eq!(opened.origin, JournalOrigin::Bootstrapped);
    assert_eq!(opened.progress, PublicationProgress::default());
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(stores.journal.journal_path()).unwrap()).unwrap();
    assert_eq!(document["version"], 3);
    assert_eq!(document["coverage_binding"], serde_json::Value::Null);
    assert_eq!(document["settled_runs"], serde_json::json!([]));
    assert_eq!(document["dispatch_may_have_started"], false);
    assert!(!stores.journal.path().exists());
    assert_eq!(
        stores
            .journal
            .open_journal(&stores.history(RoundStatus::Pending))
            .unwrap()
            .origin,
        JournalOrigin::Loaded
    );
}

#[test]
fn first_use_after_a_finished_round_bootstraps_an_empty_journal() {
    // A fresh state root after earlier publication finished: the provider
    // holds the round history, and nothing of that round is unresolved.
    for status in [
        RoundStatus::Completed,
        RoundStatus::Blocked,
        RoundStatus::Superseded,
        RoundStatus::Abandoned,
    ] {
        let stores = Stores::new();
        let _lock = stores.journal.try_lock().unwrap();

        let opened = stores
            .journal
            .open_journal(&stores.history(status))
            .unwrap();

        assert_eq!(opened.origin, JournalOrigin::Bootstrapped, "{status:?}");
        assert_eq!(
            opened.progress,
            PublicationProgress::default(),
            "{status:?}"
        );
        assert_eq!(
            stores.journal.load_journal().unwrap(),
            PublicationProgress::default()
        );
    }
}

#[test]
fn an_initialized_store_without_a_journal_is_missing_and_never_recreated() {
    let stores = Stores::new();
    stores.journal.open_journal(&stores.no_history()).unwrap();
    std::fs::remove_file(stores.journal.journal_path()).unwrap();

    for history in [stores.no_history(), stores.history(RoundStatus::Completed)] {
        assert!(matches!(
            stores.journal.open_journal(&history),
            Err(FileError::MissingProgress)
        ));
    }
    assert!(matches!(
        stores.journal.inspect_journal(),
        Err(FileError::MissingProgress)
    ));
    assert!(matches!(
        stores.journal.load_journal(),
        Err(FileError::MissingProgress)
    ));
    assert!(matches!(
        stores.journal.save_journal(&PublicationProgress::default()),
        Err(FileError::MissingProgress)
    ));
    assert_eq!(
        std::fs::read_dir(stores.key_directory()).unwrap().count(),
        0
    );
}

#[test]
fn bootstrap_creates_the_key_directory_with_its_journal_over_a_staging_leftover() {
    // F5: a crash while bootstrapping leaves only the staging directory, which
    // every reader ignores and the next bootstrap replaces.
    let stores = Stores::new();
    let _lock = stores.journal.try_lock().unwrap();
    let key = stores.key_directory();
    let staging = key.with_file_name(format!(
        ".rkc-{}.tmp",
        key.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(staging.join("nested")).unwrap();
    std::fs::write(staging.join("publication-progress.json"), b"{\"partial\"").unwrap();
    assert_eq!(
        stores.journal.inspect_journal().unwrap(),
        JournalView::Absent
    );

    let opened = stores.journal.open_journal(&stores.no_history()).unwrap();

    assert_eq!(opened.origin, JournalOrigin::Bootstrapped);
    assert_eq!(opened.progress, PublicationProgress::default());
    assert!(!staging.exists());
    let entries: Vec<_> = std::fs::read_dir(&key)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec!["publication-progress.json"]);
    assert_eq!(
        stores
            .journal
            .open_journal(&stores.history(RoundStatus::Pending))
            .unwrap()
            .origin,
        JournalOrigin::Loaded
    );
}

#[test]
fn a_key_directory_without_a_journal_stays_missing_beside_a_staging_leftover() {
    let stores = Stores::new();
    let _lock = stores.journal.try_lock().unwrap();
    let key = stores.key_directory();
    std::fs::create_dir(&key).unwrap();
    let staging = key.with_file_name(format!(
        ".rkc-{}.tmp",
        key.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir(&staging).unwrap();

    assert!(matches!(
        stores.journal.open_journal(&stores.no_history()),
        Err(FileError::MissingProgress)
    ));
    assert!(matches!(
        stores.journal.inspect_journal(),
        Err(FileError::MissingProgress)
    ));
    assert_eq!(std::fs::read_dir(&key).unwrap().count(), 0);
}

#[test]
fn saving_refuses_to_discard_settlements_or_store_invalid_relations() {
    let stores = Stores::new();
    stores.journal.open_journal(&stores.no_history()).unwrap();
    let settled = PublicationProgress {
        coverage_binding: Some(binding(2)),
        settled_runs: vec![settlement(1)],
        ..Default::default()
    };
    stores.journal.save_journal(&settled).unwrap();
    let bytes = stores.bytes(&stores.journal.journal_path());

    for regression in [
        PublicationProgress {
            settled_runs: Vec::new(),
            ..settled.clone()
        },
        PublicationProgress {
            settled_runs: vec![SettledRun {
                outcome: SettledOutcome::NoChanges,
                ..settlement(1)
            }],
            ..settled.clone()
        },
    ] {
        assert!(matches!(
            stores.journal.save_journal(&regression),
            Err(FileError::JournalRegression)
        ));
    }
    let unbound_intent = PublicationProgress {
        coverage_binding: None,
        unresolved: Some(unresolved(&stores.snapshot, &stores.request)),
        ..settled.clone()
    };
    assert!(matches!(
        stores.journal.save_journal(&unbound_intent),
        Err(FileError::Json(_))
    ));
    assert_eq!(stores.bytes(&stores.journal.journal_path()), bytes);
    assert_eq!(stores.journal.load_journal().unwrap(), settled);
}

#[test]
fn generic_saving_refuses_run_bound_state_instead_of_dropping_it() {
    let stores = Stores::new();
    stores.journal.load(&stores.workspace).unwrap();
    let bytes = stores.bytes(&stores.journal.path());
    let bound = PublicationProgress {
        coverage_binding: Some(binding(1)),
        ..Default::default()
    };

    assert!(stores.journal.save(&bound).is_err());
    assert_eq!(stores.bytes(&stores.journal.path()), bytes);
    assert!(!stores.journal.journal_path().exists());
}

#[test]
fn run_bound_evidence_checkpoints_into_the_journal_file_only() {
    let stores = Stores::new();
    stores.journal.open_journal(&stores.no_history()).unwrap();
    let repository = evidence_run::Repository::new();
    let runtime = repository.store();
    let lock = runtime.create_and_lock().unwrap();
    let writer = evidence_run::install_run(&lock, &evidence_run::run_id(1));
    let run_path = writer.path().to_owned();
    let mut evidence = FileEvidence::for_run(writer, &stores.journal).unwrap();
    let progress = PublicationProgress {
        coverage_binding: Some(binding(1)),
        dispatch_may_have_started: true,
        ..Default::default()
    };
    evidence
        .record(ExecutionEvent::Progress {
            subject: &stores.snapshot.subject,
            reviewer: &stores.snapshot.reviewer,
            progress: &progress,
        })
        .unwrap();
    assert_eq!(stores.journal.load_journal().unwrap(), progress);
    // Intent without a prepared checkpoint is retained without dispatch certainty.
    let write = creation(&stores.request);
    evidence
        .record(ExecutionEvent::BeforeWrite(&write))
        .unwrap();

    let stored = stores.journal.load_journal().unwrap();
    let pending = stored.unresolved.as_ref().unwrap();
    assert_eq!(pending.write, write);
    assert_eq!(
        pending.phase,
        review_kit_application::use_cases::publishing::WritePhase::LegacyAmbiguous
    );
    assert_eq!(stored.coverage_binding, progress.coverage_binding);
    assert!(stored.dispatch_may_have_started);
    assert!(!stores.journal.path().exists());
    assert!(!stores.workspace.root().join("progress.json").exists());
    let events: serde_json::Value = serde_json::from_slice(
        &std::fs::read(run_path.join("review-publication-events.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(events["events"][0]["progress"]["version"], 3);
    drop(stores.directory);
}

/// Reads the durable journal at the moment the provider write would be sent.
struct Probe {
    snapshot: Snapshot,
    journal: PathBuf,
    observed_at_dispatch: Option<serde_json::Value>,
    writes: usize,
}
struct ProbeSession<'a>(&'a mut Probe);
impl ReviewReader for Probe {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.snapshot.clone())
    }
}
impl ReviewReader for ProbeSession<'_> {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.0.snapshot.clone())
    }
}
impl PublicationHost for Probe {
    fn acquire(
        &mut self,
        _: &Subject,
        _: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError> {
        Ok(Box::new(ProbeSession(self)))
    }
}
impl PublicationSession for ProbeSession<'_> {
    fn authenticated_reviewer(&self) -> &ReviewerId {
        &self.0.snapshot.reviewer
    }
    fn authority(&mut self) -> Result<Effects, PortError> {
        Ok(Effects::review())
    }
    fn write(&mut self, _: &Subject, _: &ReviewerId, _: &Write) -> WriteOutcome {
        self.0.writes += 1;
        self.0.observed_at_dispatch = std::fs::read(&self.0.journal)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        WriteOutcome::Rejected(PortError::new(
            "test provider write",
            std::io::Error::other("definite rejection"),
        ))
    }
}

#[test]
fn the_journal_holds_binding_and_dispatch_flag_before_the_provider_call() {
    let stores = Stores::new();
    let _lock = stores.journal.try_lock().unwrap();
    let mut progress = stores
        .journal
        .open_journal(&stores.no_history())
        .unwrap()
        .progress;
    let request = binding_request(1);
    let mut reader = Probe {
        snapshot: stores.snapshot.clone(),
        journal: stores.journal.journal_path(),
        observed_at_dispatch: None,
        writes: 0,
    };
    let mut host = Probe {
        snapshot: stores.snapshot.clone(),
        journal: stores.journal.journal_path(),
        observed_at_dispatch: None,
        writes: 0,
    };
    let repository = evidence_run::Repository::new();
    let runtime = repository.store();
    let lock = runtime.create_and_lock().unwrap();
    let writer = evidence_run::install_run(&lock, &evidence_run::run_id(1));
    let mut evidence = FileEvidence::for_run(writer, &stores.journal).unwrap();

    let report = run_bound(
        &stores.request,
        Some(&request),
        RunMode::Publish,
        RunPorts {
            reader: &mut reader,
            host: &mut host,
            source: &mut Source,
            renderer: &mut Renderer,
            clock: &mut SystemClock,
            evidence: &mut evidence,
            progress: &mut progress,
        },
    );

    assert_eq!(host.writes, 1);
    let at_dispatch = host.observed_at_dispatch.unwrap();
    assert_eq!(at_dispatch["version"], 3);
    assert_eq!(at_dispatch["dispatch_may_have_started"], true);
    assert_eq!(at_dispatch["authorization"]["round"], 1);
    assert!(at_dispatch["authorization"]["effects"].is_array());
    assert_eq!(
        at_dispatch["coverage_binding"]["run_id"],
        request.run_id.as_str()
    );
    assert_eq!(at_dispatch["unresolved"]["phase"], "dispatch_possible");
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        unreachable!("the rejected write still produces a publication result");
    };
    assert!(matches!(result.stop, Some(Stop::Rejected(_))));
    // The definite rejection of the binding's first write restores the empty journal.
    assert_eq!(progress, PublicationProgress::default());
    assert_eq!(stores.journal.load_journal().unwrap(), progress);
}
