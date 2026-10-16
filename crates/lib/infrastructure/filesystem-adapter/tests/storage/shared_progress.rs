use super::{Directory, Host, Renderer, Source, fixture};
use std::path::Path;

use review_kit_application::{
    publication::{Effects, ProviderId, Round, RoundStatus, SummaryRecord, Write},
    use_cases::{
        publication_run::{ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run},
        publishing::{
            AbandonRequest, PublicationProgress, PublicationStatus, Stop, UnresolvedWrite,
            abandon_recorded,
        },
    },
};
use review_kit_domain::{Assessment, AssessmentDraft, Revision};
use review_kit_filesystem_adapter::{
    FileError, FileEvidence, FileProgressStore, SystemClock, Workspace,
};

fn summary_intent(
    snapshot: &review_kit_application::publication::Snapshot,
    request: &review_kit_application::use_cases::publishing::PublishRequest,
) -> (SummaryRecord, Write) {
    let round = Round {
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
    };
    let summary = SummaryRecord {
        id: ProviderId::new("12".into()).unwrap(),
        author: snapshot.reviewer.clone(),
        body: "Pending review".into(),
        current: round.clone(),
        history: Vec::new(),
    };
    let write = Write::Summary {
        expected: Some(Box::new(summary.clone())),
        round: Box::new(round),
        history: Vec::new(),
        body: "Updated review".into(),
    };
    (summary, write)
}

#[test]
fn unresolved_update_blocks_publication_and_abandonment_from_a_new_revision_workspace() {
    let directory = Directory::new();
    let first = Workspace::new(directory.0.path().join("first"));
    let second = Workspace::new(directory.0.path().join("second"));
    let (mut snapshot, _, mut request) = fixture();
    let store = FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    store.load(&first).unwrap();
    let (summary, write) = summary_intent(&snapshot, &request);
    let mut evidence = FileEvidence::with_progress_store(&first, &store).unwrap();
    evidence
        .record(ExecutionEvent::BeforeWrite(&write))
        .unwrap();
    let original = store.load(&first).unwrap();
    assert_eq!(original.unresolved.as_ref().unwrap().write, write);

    snapshot.revision =
        Revision::new("4".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
    snapshot.summaries.push(summary);
    let assessment = &request.assessment;
    request.assessment = Assessment::new(AssessmentDraft {
        subject: assessment.subject().clone(),
        reviewer: assessment.reviewer().clone(),
        revision: snapshot.revision.clone(),
        decision: assessment.decision(),
        depth: assessment.depth().into(),
        scope: assessment.scope().into(),
        limitation: assessment.limitation().map(str::to_owned),
        selected_policies: assessment.selected_policies().to_vec(),
        evaluations: assessment.evaluations().to_vec(),
        findings: assessment.findings().to_vec(),
    })
    .unwrap();
    let restarted = FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    assert_eq!(restarted.path(), store.path());
    let mut progress = restarted.load(&second).unwrap();
    assert_eq!(progress, original);
    let mut reader = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut host = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut evidence = FileEvidence::with_progress_store(&second, &restarted).unwrap();
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
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        panic!("publication must retain the unresolved update outcome");
    };
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(result.stop, Some(Stop::UnresolvedWrite)));
    assert_eq!(host.writes, 0);
    assert_eq!(progress, original);

    let result = abandon_recorded(
        &AbandonRequest {
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            round: 1,
            reason: "Review the new revision".into(),
            effects: Effects::review(),
        },
        &mut host,
        &mut Renderer,
        &mut progress,
        &mut evidence,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(result.stop, Some(Stop::UnresolvedWrite)));
    assert_eq!(host.writes, 0);
    restarted.save(&progress).unwrap();
    assert_eq!(store.load(&first).unwrap(), original);

    restarted.save(&PublicationProgress::default()).unwrap();
    progress = restarted.load(&second).unwrap();
    snapshot.summaries.clear();
    host.snapshot = snapshot;
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
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        panic!("settled shared progress must permit publication");
    };
    assert!(!matches!(result.stop, Some(Stop::UnresolvedWrite)));
    assert_eq!(host.writes, 1);
}

#[test]
fn migration_preserves_local_bytes_and_shared_state_supersedes_old_snapshots() {
    let directory = Directory::new();
    let first = Workspace::new(directory.0.path().join("first"));
    let second = Workspace::new(directory.0.path().join("second"));
    let (snapshot, _, request) = fixture();
    let (_, write) = summary_intent(&snapshot, &request);
    let original = PublicationProgress {
        verified_resolutions: Vec::new(),
        unresolved: Some(UnresolvedWrite {
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            write,
            renderer: None,
            phase: Default::default(),
            baseline: None,
        }),
        ..Default::default()
    };
    first
        .save_final_progress(&snapshot.subject, &snapshot.reviewer, &original)
        .unwrap();
    let bytes = first.read_bytes(Path::new("progress.json")).unwrap();
    let store = FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    assert_eq!(store.load(&first).unwrap(), original);
    assert_eq!(first.read_bytes(Path::new("progress.json")).unwrap(), bytes);
    store.save(&PublicationProgress::default()).unwrap();
    second
        .save_final_progress(&snapshot.subject, &snapshot.reviewer, &original)
        .unwrap();
    assert_eq!(store.load(&second).unwrap(), PublicationProgress::default());
    assert_eq!(first.read_bytes(Path::new("progress.json")).unwrap(), bytes);

    let unimported = FileProgressStore::new(
        directory.0.path().join("other-shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    std::fs::remove_file(first.root().join("progress.json")).unwrap();
    first
        .write_bytes(Path::new("result.json"), b"retained execution")
        .unwrap();
    assert!(matches!(
        unimported.load(&first),
        Err(FileError::MissingProgress)
    ));
}

#[test]
fn missing_shared_progress_refuses_restart_and_prevents_dispatch() {
    let directory = Directory::new();
    let first = Workspace::new(directory.0.path().join("first"));
    let second = Workspace::new(directory.0.path().join("second"));
    let (snapshot, _, request) = fixture();
    let store = FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    let mut progress = store.load(&first).unwrap();
    std::fs::remove_file(store.path()).unwrap();
    assert!(matches!(
        store.load(&second),
        Err(FileError::MissingProgress)
    ));
    assert!(matches!(
        store.save(&progress),
        Err(FileError::MissingProgress)
    ));
    let mut reader = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut host = Host {
        snapshot: snapshot.clone(),
        writes: 0,
    };
    let mut evidence = FileEvidence::with_progress_store(&second, &store).unwrap();
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
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        panic!("intent storage failure must preserve the publication result");
    };
    assert!(matches!(result.stop, Some(Stop::Evidence(_))));
    assert_eq!(host.writes, 0);
    assert_eq!(
        progress.unresolved.as_ref().unwrap().phase,
        review_kit_application::use_cases::publishing::WritePhase::Prepared
    );
}
