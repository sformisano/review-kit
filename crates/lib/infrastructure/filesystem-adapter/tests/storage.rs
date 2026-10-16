#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;

use review_kit_application::{
    ports::{
        CommittedSource, PortError, PublicationHost, PublicationSession, ReviewReader,
        ReviewRenderer, WriteOutcome,
    },
    publication::{Effects, PolicyName, Snapshot, Write},
    summary::SummaryView,
    use_cases::{
        assessment::{AssessmentPolicies, prepare},
        discovery_bundle::ObservedChanges,
        initialization::{DraftCreation, initialize},
        publication_run::{ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run},
        publishing::{PublicationProgress, PublishRequest},
    },
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Finding, FindingStatus, PolicyEvaluation, PolicyId,
    PolicyJudgment, ReviewDecision, ReviewerId, Revision, SourcePath, Subject,
};
use review_kit_filesystem_adapter::{
    FileError, FileEvidence, SystemClock, Workspace, atomic_create_new,
};
use review_kit_json_adapter::{ContextDocument, ProgressDocument};

#[path = "storage/journal.rs"]
mod journal;
#[path = "storage/locking.rs"]
mod locking;
#[path = "storage/observation.rs"]
mod observation;
#[path = "storage/shared_progress.rs"]
mod shared_progress;

struct Directory(tempfile::TempDir);
impl Directory {
    fn new() -> Self {
        Self(
            tempfile::Builder::new()
                .prefix("rkc-storage-")
                .tempdir()
                .unwrap(),
        )
    }
    fn workspace(&self) -> Workspace {
        Workspace::new(&self.0)
    }
}
fn fixture() -> (Snapshot, AssessmentPolicies, PublishRequest) {
    let subject = Subject::new(
        "gitlab".into(),
        "https://example.test".into(),
        "team/project".into(),
        1,
    )
    .unwrap();
    let reviewer = ReviewerId::try_from("9".to_owned()).unwrap();
    let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
    let snapshot = Snapshot {
        subject: subject.clone(),
        reviewer: reviewer.clone(),
        revision: revision.clone(),
        summaries: Vec::new(),
        findings: Vec::new(),
        replies: Vec::new(),
        permissions: Effects::review(),
    };
    let proof = PolicyId::try_from("proof".to_owned()).unwrap();
    let set = AssessmentPolicies::new(
        "rust".into(),
        vec![PolicyName {
            id: proof.clone(),
            name: "Proof".into(),
        }],
    )
    .unwrap();
    let assessment = Assessment::new(AssessmentDraft {
        subject,
        reviewer,
        revision,
        decision: ReviewDecision::Ready,
        depth: "complete".into(),
        scope: "change".into(),
        limitation: None,
        selected_policies: vec![proof.clone()],
        evaluations: vec![PolicyEvaluation::new(proof, PolicyJudgment::Satisfied).unwrap()],
        findings: Vec::new(),
    })
    .unwrap();
    let request = PublishRequest {
        confirmed_validation: false,
        review_workflow: None,
        assessment,
        policies: set.clone(),
        policy_declarations: set.policies().iter().map(|policy| review_kit_application::publication::PublicPolicyDeclaration { policy: policy.clone(), status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed failure path requires this policy.".into() }).collect(),
        effects: Effects::review(),
        answers: Vec::new(),
        answer_targets: Vec::new(),
        transitions: Vec::new(),
        local_context: String::new(),
        allow_colocated: false,
    };
    (snapshot, set, request)
}

#[test]
fn competing_draft_creations_preserve_one_complete_destination() {
    let directory = Directory::new();
    let path = directory.0.path().join("assessment.json");
    let mut workers = Vec::new();
    for index in 0..8 {
        let path = path.clone();
        workers.push(std::thread::spawn(move || {
            let body = vec![b'a' + index; 8192];
            (atomic_create_new(&path, &body).unwrap(), body)
        }));
    }
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let winners: Vec<_> = outcomes.iter().filter(|(created, _)| *created).collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), winners[0].1);
    assert!(!atomic_create_new(&path, b"replacement").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), winners[0].1);
}

#[test]
fn initialization_keeps_existing_agent_bytes_and_bound_context() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, set, _) = fixture();
    let changes = ObservedChanges {
        subject: snapshot.subject.clone(),
        revision: snapshot.revision.clone(),
        files: Vec::new(),
        unified_diff: String::new(),
    };
    let context = ContextDocument::new(&snapshot, &changes);
    workspace
        .save_context(Path::new("discovery/one/context.json"), &context)
        .unwrap();
    let bundle = context.bundle().unwrap();
    let mut writer =
        workspace.draft_writer(Path::new("assessment.json"), "discovery/one/context.json");
    assert_eq!(
        initialize(&bundle, set.clone(), &mut writer)
            .unwrap()
            .outcome,
        DraftCreation::Created
    );
    let draft = workspace.load_draft(Path::new("assessment.json")).unwrap();
    assert_eq!(draft.context, "discovery/one/context.json");
    workspace
        .write_bytes(Path::new("assessment.json"), b"agent work")
        .unwrap();
    assert_eq!(
        initialize(&bundle, set.clone(), &mut writer)
            .unwrap()
            .outcome,
        DraftCreation::Existing
    );
    assert_eq!(
        workspace.read_bytes(Path::new("assessment.json")).unwrap(),
        b"agent work"
    );
    assert!(workspace.read_bytes(Path::new("../outside")).is_err());
    let _ = prepare(snapshot, bundle.reviewer().clone(), set.clone()).unwrap();
}

#[test]
fn intent_survives_recorder_restart_and_missing_progress_is_refused() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, _) = fixture();
    workspace
        .load_progress(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    let write = Write::SetResolved {
        thread: review_kit_application::publication::ProviderId::new("thread".into()).unwrap(),
        resolved: true,
    };
    let mut evidence =
        FileEvidence::new(&workspace, &snapshot.subject, &snapshot.reviewer).unwrap();
    evidence
        .record(ExecutionEvent::BeforeWrite(&write))
        .unwrap();
    drop(evidence);
    let progress = workspace
        .load_progress(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    assert_eq!(progress.unresolved.unwrap().write, write);
    let restored: ProgressDocument = workspace.read_json(Path::new("progress.json")).unwrap();
    assert!(
        restored
            .into_progress(
                &snapshot.subject,
                &ReviewerId::try_from("other".to_owned()).unwrap()
            )
            .is_err()
    );
    std::fs::remove_file(directory.0.path().join("progress.json")).unwrap();
    assert!(matches!(
        workspace.load_progress(&snapshot.subject, &snapshot.reviewer),
        Err(FileError::MissingProgress)
    ));
}

#[test]
fn recorder_restart_keeps_original_renderer_identity_and_unknown_legacy_events() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, _) = fixture();
    workspace
        .load_progress(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    let write = Write::SetResolved {
        thread: review_kit_application::publication::ProviderId::new("thread".into()).unwrap(),
        resolved: true,
    };
    for renderer in [None, Some("profile/v1"), Some("profile/v2")] {
        let mut evidence =
            FileEvidence::new(&workspace, &snapshot.subject, &snapshot.reviewer).unwrap();
        if let Some(renderer) = renderer {
            evidence = evidence.with_renderer(renderer);
        }
        evidence
            .record(ExecutionEvent::BeforeWrite(&write))
            .unwrap();
    }
    let document: serde_json::Value = workspace.read_json(Path::new("evidence.json")).unwrap();
    assert!(document["events"][0].get("renderer").is_none());
    assert_eq!(document["events"][1]["renderer"], "profile/v1");
    assert_eq!(document["events"][2]["renderer"], "profile/v2");
    assert!(
        document["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["write"]["thread"] == "thread")
    );
}

#[test]
fn confirmed_resolution_survives_restart_and_the_next_write_intent() {
    use review_kit_application::{
        ports::RenderProfileIdentity,
        publication::{ProviderId, ResolutionReceipt},
    };
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, _) = fixture();
    let store = review_kit_filesystem_adapter::FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    store.load(&workspace).unwrap();
    let profile = RenderProfileIdentity {
        id: "team".into(),
        input_contract: "review-kit-render-input/v1".into(),
        fingerprint: "sha256:fixture".into(),
    };
    let thread = ProviderId::new("thread".into()).unwrap();
    let receipt = ResolutionReceipt {
        provenance: Default::default(),
        subject: snapshot.subject.clone(),
        reviewer: snapshot.reviewer.clone(),
        round: 2,
        thread: thread.clone(),
        resolved: true,
    };
    let progress = PublicationProgress {
        unresolved: None,
        verified_resolutions: vec![receipt.clone()],
        ..Default::default()
    };
    FileEvidence::with_progress_store(&workspace, &store)
        .unwrap()
        .with_profile(&profile)
        .record(ExecutionEvent::Progress {
            subject: &snapshot.subject,
            reviewer: &snapshot.reviewer,
            progress: &progress,
        })
        .unwrap();
    assert_eq!(store.load(&workspace).unwrap(), progress);
    let write = Write::SetResolved {
        thread,
        resolved: false,
    };
    FileEvidence::with_progress_store(&workspace, &store)
        .unwrap()
        .with_profile(&profile)
        .record(ExecutionEvent::BeforeWrite(&write))
        .unwrap();
    let restored = store.load(&workspace).unwrap();
    assert_eq!(restored.verified_resolutions, vec![receipt]);
    let unresolved = restored.unresolved.unwrap();
    assert_eq!(unresolved.write, write);
    assert_eq!(unresolved.renderer, Some(profile));
}

struct Host {
    snapshot: Snapshot,
    writes: usize,
}
struct Session<'a>(&'a mut Host);
impl ReviewReader for Host {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.snapshot.clone())
    }
}
impl ReviewReader for Session<'_> {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.0.snapshot.clone())
    }
}
impl PublicationHost for Host {
    fn acquire(
        &mut self,
        _: &Subject,
        _: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError> {
        Ok(Box::new(Session(self)))
    }
}
impl PublicationSession for Session<'_> {
    fn authenticated_reviewer(&self) -> &ReviewerId {
        &self.0.snapshot.reviewer
    }
    fn authority(&mut self) -> Result<Effects, PortError> {
        Ok(Effects::review())
    }
    fn write(&mut self, _: &Subject, _: &ReviewerId, _: &Write) -> WriteOutcome {
        self.0.writes += 1;
        WriteOutcome::Rejected(PortError::new(
            "test provider write",
            std::io::Error::other("unexpected dispatch"),
        ))
    }
}
struct Source;
impl CommittedSource for Source {
    fn lines(
        &mut self,
        _: &Subject,
        _: &Revision,
        _: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        Ok(Vec::new())
    }
    fn validate_position(
        &mut self,
        _: &Subject,
        _: &Revision,
        _: &Finding,
    ) -> Result<(), PortError> {
        Ok(())
    }
}
struct Renderer;
impl ReviewRenderer for Renderer {
    fn finding(&mut self, _: &Finding) -> Result<String, PortError> {
        Ok("finding".into())
    }
    fn transition(&mut self, _: &Finding, _: FindingStatus, _: &str) -> Result<String, PortError> {
        Ok("transition".into())
    }
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        Ok(body.into())
    }
    fn summary(&mut self, _: &SummaryView) -> Result<String, PortError> {
        Ok("summary".into())
    }
}

#[test]
fn failed_progress_persistence_prevents_provider_dispatch() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, request) = fixture();
    let mut progress: PublicationProgress = workspace
        .load_progress(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    std::fs::remove_file(directory.0.path().join("progress.json")).unwrap();
    std::fs::create_dir(directory.0.path().join("progress.json")).unwrap();
    let mut reader = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut host = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut evidence = FileEvidence::new(&workspace, &snapshot.subject, &snapshot.reviewer)
        .unwrap()
        .with_renderer("test-profile/v1");
    let report = run(
        &request,
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
    assert_eq!(host.writes, 0);
    let document: serde_json::Value = workspace.read_json(Path::new("evidence.json")).unwrap();
    let events = document["events"].as_array().unwrap();
    assert_eq!(events[0]["kind"], "started");
    assert_eq!(events[0]["renderer"], "test-profile/v1");
    let prepared = events
        .iter()
        .find(|event| event["kind"] == "progress")
        .unwrap();
    assert_eq!(prepared["progress"]["unresolved"]["phase"], "prepared");
    assert_eq!(
        prepared["progress"]["unresolved"]["write"]["body"],
        "summary"
    );
    assert_eq!(
        events.last().unwrap()["report"]["renderer"],
        "test-profile/v1"
    );
    match report.outcome {
        Ok(RunOutcome::Published(result)) => assert!(matches!(
            result.stop,
            Some(review_kit_application::use_cases::publishing::Stop::Evidence(_))
        )),
        outcome => panic!("unexpected run outcome: {outcome:?}"),
    }
}
