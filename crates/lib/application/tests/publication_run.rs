#![allow(clippy::unwrap_used)]

use std::{
    cell::RefCell,
    io,
    rc::Rc,
    time::{Duration, SystemTime},
};

use review_kit_application::{
    ports::{
        CommittedSource, PortError, PublicationHost, PublicationSession, ReviewReader,
        ReviewRenderer, WriteOutcome,
    },
    publication::{Effects, ProviderId, Snapshot, SummaryRecord, Write},
    use_cases::{
        assessment::AssessmentPolicies,
        publication_run::{
            Clock, ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run,
        },
        publishing::{PlanError, PublicationStatus, PublishRequest, Stop},
    },
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Finding, FindingStatus, ReviewDecision, ReviewerId, Revision,
    SourcePath, Subject,
};

type Log = Rc<RefCell<Vec<String>>>;

#[path = "support/operation_logs.rs"]
mod operation_logs;

#[path = "publication_run/operation_logging.rs"]
mod operation_logging;

fn failure(operation: &'static str) -> PortError {
    PortError::new(operation, io::Error::other("injected failure"))
}

fn request(effects: Effects) -> PublishRequest {
    let set = AssessmentPolicies::new("test".into(), vec![]).unwrap();
    PublishRequest {
        confirmed_validation: false,
        review_workflow: None,
        assessment: Assessment::new(AssessmentDraft {
            subject: Subject::new("gitlab".into(), "host".into(), "group/repo".into(), 7).unwrap(),
            reviewer: ReviewerId::try_from("agent".to_owned()).unwrap(),
            revision: Revision::new("head".into(), vec!["base".into(), "start".into()]).unwrap(),
            decision: ReviewDecision::Ready,
            depth: "Changed code and callers".into(),
            scope: "Request handling".into(),
            limitation: None,
            selected_policies: vec![],
            evaluations: vec![],
            findings: vec![],
        })
        .unwrap(),
        policies: set.clone(),
        policy_declarations: vec![],
        effects,
        answers: vec![],
        answer_targets: vec![],
        transitions: vec![],
        local_context: "private agent scratch notes".into(),
        allow_colocated: false,
    }
}

fn snapshot(request: &PublishRequest) -> Snapshot {
    Snapshot {
        subject: request.assessment.subject().clone(),
        reviewer: request.assessment.reviewer().clone(),
        revision: request.assessment.revision().clone(),
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::review(),
    }
}

struct Reader {
    snapshot: Snapshot,
    calls: usize,
    fail: bool,
}
impl ReviewReader for Reader {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        self.calls += 1;
        if self.fail {
            Err(failure("discover"))
        } else {
            Ok(self.snapshot.clone())
        }
    }
}

struct Host {
    snapshot: Snapshot,
    reviewer: ReviewerId,
    acquires: usize,
    discoveries: usize,
    fail_discovery: Option<usize>,
    malformed_readback: bool,
    log: Log,
}
impl PublicationHost for Host {
    fn acquire(
        &mut self,
        _: &Subject,
        _: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError> {
        self.acquires += 1;
        Ok(Box::new(Session(self)))
    }
}
struct Session<'a>(&'a mut Host);
impl ReviewReader for Session<'_> {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        self.0.discoveries += 1;
        if self.0.fail_discovery == Some(self.0.discoveries) {
            return Err(failure("discover"));
        }
        Ok(self.0.snapshot.clone())
    }
}
impl PublicationSession for Session<'_> {
    fn authenticated_reviewer(&self) -> &ReviewerId {
        &self.0.reviewer
    }
    fn authority(&mut self) -> Result<Effects, PortError> {
        Ok(Effects::review())
    }
    fn write(&mut self, _: &Subject, reviewer: &ReviewerId, write: &Write) -> WriteOutcome {
        self.0.log.borrow_mut().push("provider-write".into());
        if let Write::Summary {
            round,
            history,
            body,
            ..
        } = write
        {
            self.0.snapshot.summaries = vec![SummaryRecord {
                id: ProviderId::new("summary-note".into()).unwrap(),
                author: reviewer.clone(),
                body: body.clone(),
                current: *round.clone(),
                history: history.clone(),
            }];
            if self.0.malformed_readback {
                self.0
                    .snapshot
                    .summaries
                    .push(self.0.snapshot.summaries[0].clone());
            }
            WriteOutcome::Acknowledged
        } else {
            WriteOutcome::Rejected(failure("unexpected write"))
        }
    }
}

#[derive(Default)]
struct Source {
    calls: usize,
}
impl CommittedSource for Source {
    fn lines(
        &mut self,
        _: &Subject,
        _: &Revision,
        _: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        self.calls += 1;
        Err(failure("unexpected source read"))
    }
    fn validate_position(
        &mut self,
        _: &Subject,
        _: &Revision,
        _: &Finding,
    ) -> Result<(), PortError> {
        self.calls += 1;
        Err(failure("unexpected position check"))
    }
}
struct Renderer;
impl ReviewRenderer for Renderer {
    fn finding(&mut self, _: &Finding) -> Result<String, PortError> {
        Err(failure("unexpected finding"))
    }
    fn transition(&mut self, _: &Finding, _: FindingStatus, _: &str) -> Result<String, PortError> {
        Err(failure("unexpected transition"))
    }
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        Ok(body.into())
    }
    fn summary(
        &mut self,
        view: &review_kit_application::summary::SummaryView,
    ) -> Result<String, PortError> {
        Ok(format!(
            "Round {} {:?}",
            view.round.number, view.round.status
        ))
    }
}
#[derive(Default)]
struct TestClock {
    calls: u64,
}
impl Clock for TestClock {
    fn now(&mut self) -> SystemTime {
        self.calls += 1;
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.calls)
    }
}
struct Recorder {
    log: Log,
    events: Vec<String>,
    fail_started: bool,
    fail_write: bool,
    fail_readback: bool,
    fail_finished: bool,
}
impl ExecutionEvidence for Recorder {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        self.events.push(format!("{event:?}"));
        let (name, fail) = match event {
            ExecutionEvent::Started { .. } => ("started", self.fail_started),
            ExecutionEvent::Observed(_) => ("observed", false),
            ExecutionEvent::Planned(_) => ("planned", false),
            ExecutionEvent::BeforeWrite(_) => ("before-write", self.fail_write),
            ExecutionEvent::DiscardedUnsent(_) => ("discarded-unsent", false),
            ExecutionEvent::Readback { .. } => ("readback", self.fail_readback),
            ExecutionEvent::Progress { .. } => ("progress", false),
            ExecutionEvent::Finished(_) => ("finished", self.fail_finished),
        };
        self.log.borrow_mut().push(name.into());
        if fail {
            Err(failure("record evidence"))
        } else {
            Ok(())
        }
    }
}

struct Fixture {
    progress: review_kit_application::use_cases::publishing::PublicationProgress,
    reader: Reader,
    host: Host,
    source: Source,
    renderer: Renderer,
    clock: TestClock,
    recorder: Recorder,
}
impl Fixture {
    fn new(request: &PublishRequest) -> Self {
        let log = Rc::new(RefCell::new(vec![]));
        Self {
            progress: Default::default(),
            reader: Reader {
                snapshot: snapshot(request),
                calls: 0,
                fail: false,
            },
            host: Host {
                snapshot: snapshot(request),
                reviewer: request.assessment.reviewer().clone(),
                acquires: 0,
                discoveries: 0,
                fail_discovery: None,
                malformed_readback: false,
                log: log.clone(),
            },
            source: Source::default(),
            renderer: Renderer,
            clock: TestClock::default(),
            recorder: Recorder {
                log,
                events: vec![],
                fail_started: false,
                fail_write: false,
                fail_readback: false,
                fail_finished: false,
            },
        }
    }
    fn ports(&mut self) -> RunPorts<'_> {
        RunPorts {
            reader: &mut self.reader,
            host: &mut self.host,
            source: &mut self.source,
            renderer: &mut self.renderer,
            clock: &mut self.clock,
            evidence: &mut self.recorder,
            progress: &mut self.progress,
        }
    }
}

#[test]
fn empty_effects_plan_without_acquiring_publication_session_and_record_timestamps() {
    let request = request(Effects::none());
    let mut fixture = Fixture::new(&request);
    let report = run(&request, RunMode::Publish, fixture.ports());
    assert_eq!(report.mode, RunMode::Plan);
    assert!(matches!(report.outcome, Ok(RunOutcome::Planned(_))));
    assert_eq!(
        report.started_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1)
    );
    assert_eq!(
        report.finished_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(2)
    );
    assert_eq!(report.final_assessment, request.assessment);
    assert_eq!(fixture.host.acquires, 0);
    assert_eq!(fixture.reader.calls, 1);
    assert_eq!(fixture.source.calls, 0);
    assert_eq!(
        *fixture.recorder.log.borrow(),
        ["started", "observed", "planned", "finished"]
    );
    assert!(
        !fixture
            .recorder
            .events
            .join("\n")
            .contains(&request.local_context)
    );
}

#[test]
fn explicit_plan_mode_does_not_publish_even_when_effects_are_available() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    let report = run(&request, RunMode::Plan, fixture.ports());
    assert!(matches!(report.outcome, Ok(RunOutcome::Planned(_))));
    assert_eq!(fixture.host.acquires, 0);
}

#[test]
fn recording_start_failure_prevents_discovery_and_publication() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_started = true;
    let report = run(&request, RunMode::Publish, fixture.ports());
    assert!(matches!(report.outcome, Err(PlanError::Evidence(_))));
    assert_eq!(fixture.reader.calls, 0);
    assert_eq!(fixture.host.acquires, 0);
    assert_eq!(*fixture.recorder.log.borrow(), ["started", "finished"]);
}

#[test]
fn discovery_failure_keeps_run_metadata_and_records_failed_completion() {
    let request = request(Effects::none());
    let mut fixture = Fixture::new(&request);
    fixture.reader.fail = true;
    let report = run(&request, RunMode::Plan, fixture.ports());
    assert!(matches!(report.outcome, Err(PlanError::Discovery(_))));
    assert_eq!(report.final_assessment, request.assessment);
    assert!(report.finished_at > report.started_at);
    assert_eq!(*fixture.recorder.log.borrow(), ["started", "finished"]);
}

#[test]
fn publication_records_intent_before_each_write_and_each_verified_readback() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    let report = run(&request, RunMode::Publish, fixture.ports());
    match report.outcome {
        Ok(RunOutcome::Published(result)) => {
            assert_eq!(result.status, PublicationStatus::Completed);
            assert_eq!(result.verified.len(), 2);
        }
        other => assert!(matches!(other, Ok(RunOutcome::Published(_))), "{other:?}"),
    }
    let io_order: Vec<_> = fixture
        .recorder
        .log
        .borrow()
        .iter()
        .filter(|name| {
            matches!(
                name.as_str(),
                "before-write" | "provider-write" | "readback"
            )
        })
        .cloned()
        .collect();
    assert_eq!(
        io_order,
        [
            "before-write",
            "provider-write",
            "readback",
            "before-write",
            "provider-write",
            "readback"
        ]
    );
}

#[test]
fn failed_prewrite_evidence_prevents_mutation() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_write = true;
    let report = run(&request, RunMode::Publish, fixture.ports());
    match report.outcome {
        Ok(RunOutcome::Published(result)) => {
            assert!(matches!(result.stop, Some(Stop::Evidence(_))));
            assert_eq!(result.status, PublicationStatus::NotStarted);
            assert!(result.attempted.is_empty());
            assert!(result.verified.is_empty());
        }
        other => assert!(matches!(other, Ok(RunOutcome::Published(_))), "{other:?}"),
    }
    assert!(
        !fixture
            .recorder
            .log
            .borrow()
            .iter()
            .any(|name| name == "provider-write")
    );
}

#[test]
fn final_evidence_failure_preserves_actual_completed_publication() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_finished = true;
    let report = run(&request, RunMode::Publish, fixture.ports());
    assert!(report.evidence_failure.is_some());
    assert!(
        matches!(report.outcome, Ok(RunOutcome::Published(result)) if result.status == PublicationStatus::Completed)
    );
}

#[test]
fn readback_recording_failure_keeps_the_verified_effect_and_stops_writes() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_readback = true;
    let report = run(&request, RunMode::Publish, fixture.ports());
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        unreachable!()
    };
    assert!(matches!(result.stop, Some(Stop::Evidence(_))));
    assert_eq!(result.verified.len(), 1);
    assert_eq!(result.attempted.len(), 1);
    assert_eq!(
        result.final_snapshot.summaries,
        fixture.host.snapshot.summaries
    );
    assert_eq!(fixture.host.snapshot.summaries.len(), 1);
    assert_eq!(
        fixture
            .recorder
            .log
            .borrow()
            .iter()
            .filter(|event| event.as_str() == "provider-write")
            .count(),
        1
    );
}

#[test]
fn prewrite_read_failure_is_not_started() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.host.fail_discovery = Some(2);
    let report = run(&request, RunMode::Publish, fixture.ports());
    match report.outcome {
        Ok(RunOutcome::Published(result)) => {
            assert_eq!(result.status, PublicationStatus::NotStarted);
            assert!(matches!(result.stop, Some(Stop::Read(_))));
            assert!(result.attempted.is_empty());
            assert!(result.verified.is_empty());
        }
        other => assert!(
            matches!(other, Ok(RunOutcome::Published(_))),
            "expected publication result, got {other:?}"
        ),
    }
    assert!(
        !fixture
            .recorder
            .log
            .borrow()
            .iter()
            .any(|name| name == "provider-write")
    );
}

#[test]
fn malformed_postwrite_readback_is_recorded_before_conflict() {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.host.malformed_readback = true;
    let report = run(&request, RunMode::Publish, fixture.ports());
    match report.outcome {
        Ok(RunOutcome::Published(result)) => {
            assert!(matches!(result.stop, Some(Stop::Conflict)));
            assert_eq!(result.attempted.len(), 1);
            assert!(result.verified.is_empty());
        }
        other => assert!(
            matches!(other, Ok(RunOutcome::Published(_))),
            "expected publication result, got {other:?}"
        ),
    }
    assert_eq!(
        fixture
            .recorder
            .log
            .borrow()
            .iter()
            .filter(|name| *name == "provider-write")
            .count(),
        1
    );
    assert!(
        fixture
            .recorder
            .log
            .borrow()
            .iter()
            .any(|name| name == "readback")
    );
}
