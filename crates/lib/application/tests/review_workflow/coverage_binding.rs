//! Run-bound publication: admission, installation, dispatch flag and rollback.

use std::time::{Duration, SystemTime};

use super::*;
use review_kit_application::{
    review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles},
    use_cases::{
        publication_run::{
            Clock, ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run_bound,
        },
        publishing::{
            AbandonRequest, BindingError, BindingRequest, RoundAuthorization, RunBinding,
            RunPublication, SettledRun, WritePhase, abandon_bound,
        },
    },
};

pub(super) fn run_id(number: u8) -> RunId {
    RunId::try_from(format!("00000000-0000-4000-8000-{number:012}")).unwrap()
}

pub(super) fn content(digit: char) -> ContentRef {
    ContentRef::from_hex(&digit.to_string().repeat(64)).unwrap()
}

pub(super) fn files(digit: char) -> SnapshotFiles {
    SnapshotFiles {
        context: content(digit),
        source: content(digit),
        diff: content(digit),
        conversation: content(digit),
    }
}

pub(super) fn bind(number: u8, assessment: char) -> BindingRequest {
    BindingRequest {
        run_id: run_id(number),
        review_assessment: content(assessment),
        snapshot_files: files('a'),
    }
}

/// The binding of `request` installed in a journal without a round grant.
pub(super) fn bound(request: &BindingRequest, round: u64) -> RunBinding {
    RunBinding {
        run_id: request.run_id.clone(),
        round,
        review_assessment: request.review_assessment.clone(),
        snapshot_files: request.snapshot_files.clone(),
        prior_authorization: None,
    }
}

/// The binding of `request` installed while `prior` was the journal's round grant.
pub(super) fn bound_after(
    request: &BindingRequest,
    round: u64,
    prior: Option<RoundAuthorization>,
) -> RunBinding {
    RunBinding {
        prior_authorization: prior,
        ..bound(request, round)
    }
}

/// Persists each progress checkpoint and logs it beside provider writes.
pub(super) struct Journal {
    log: Log,
    pub(super) persisted: Option<PublicationProgress>,
    pub(super) checkpoints: Vec<PublicationProgress>,
    pub(super) fail_before_write: Option<usize>,
    pub(super) before_writes: usize,
}

impl ExecutionEvidence for Journal {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        match event {
            ExecutionEvent::Progress { progress, .. } => {
                self.log.borrow_mut().push(format!(
                    "checkpoint:bound={}:dispatch={}:phase={:?}",
                    progress
                        .coverage_binding
                        .as_ref()
                        .map_or("none".to_owned(), |binding| binding.run_id.to_string()),
                    progress.dispatch_may_have_started,
                    progress.unresolved.as_ref().map(|pending| pending.phase),
                ));
                self.persisted = Some(progress.clone());
                self.checkpoints.push(progress.clone());
            }
            ExecutionEvent::BeforeWrite(_) => {
                self.before_writes += 1;
                if self.fail_before_write == Some(self.before_writes) {
                    return Err(failure("record intended write"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) struct FixedClock(pub(super) SystemTime);
impl Clock for FixedClock {
    fn now(&mut self) -> SystemTime {
        self.0
    }
}

struct Retained(Snapshot);
impl ReviewReader for Retained {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.0.clone())
    }
}

pub(super) struct Harness {
    pub(super) host: Host,
    pub(super) source: Source,
    renderer: Renderer,
    pub(super) journal: Journal,
    pub(super) clock: FixedClock,
}

impl Harness {
    pub(super) fn new() -> Self {
        let (host, source, renderer) = fixtures();
        let log = host.log.clone();
        Self {
            host,
            source,
            renderer,
            journal: Journal {
                log,
                persisted: None,
                checkpoints: Vec::new(),
                fail_before_write: None,
                before_writes: 0,
            },
            clock: FixedClock(SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_893_400)),
        }
    }

    pub(super) fn run(
        &mut self,
        request: &PublishRequest,
        binding: Option<&BindingRequest>,
        mode: RunMode,
    ) -> Result<RunOutcome, PlanError> {
        let mut progress = std::mem::take(&mut self.host.progress);
        let mut reader = Retained(self.host.snapshot.clone());
        let report = run_bound(
            request,
            binding,
            mode,
            RunPorts {
                reader: &mut reader,
                host: &mut self.host,
                source: &mut self.source,
                renderer: &mut self.renderer,
                clock: &mut self.clock,
                evidence: &mut self.journal,
                progress: &mut progress,
            },
        );
        self.host.progress = progress;
        report.outcome
    }

    pub(super) fn publish(
        &mut self,
        request: &PublishRequest,
        binding: Option<&BindingRequest>,
    ) -> Result<PublicationResult, PlanError> {
        match self.run(request, binding, RunMode::Publish)? {
            RunOutcome::Published(result) => Ok(*result),
            RunOutcome::Planned(_) => unreachable!("requested effects select publication"),
        }
    }

    pub(super) fn abandon(
        &mut self,
        request: &AbandonRequest,
        binding: Option<&BindingRequest>,
    ) -> Result<PublicationResult, PlanError> {
        let mut progress = std::mem::take(&mut self.host.progress);
        let result = abandon_bound(
            request,
            binding,
            &mut self.host,
            &mut self.renderer,
            &mut progress,
            &mut self.journal,
            &mut self.clock,
        );
        self.host.progress = progress;
        result
    }

    pub(super) fn settle(
        &mut self,
        binding: &BindingRequest,
        result: &PublicationResult,
    ) -> Result<Option<SettledRun>, BindingError> {
        self.host
            .progress
            .settle_publication(binding, result, self.clock.0)
    }

    pub(super) fn progress(&self) -> &PublicationProgress {
        &self.host.progress
    }

    pub(super) fn reset_observations(&mut self) {
        self.host.writes.clear();
        self.host.log.borrow_mut().clear();
        self.journal.checkpoints.clear();
    }
}

pub(super) fn abandonment(host: &Host) -> AbandonRequest {
    AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: host.snapshot.summaries[0].current.number,
        reason: "The finding requires a corrected assessment.".into(),
        effects: Effects::review(),
    }
}

pub(super) fn binding_error(result: Result<PublicationResult, PlanError>) -> Option<BindingError> {
    match result {
        Err(PlanError::Binding(error)) => Some(error),
        _ => None,
    }
}

#[test]
fn first_bound_write_persists_binding_with_intent_and_flags_dispatch_before_sending() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    assert_eq!(
        harness.progress().run_publication(&first.run_id),
        RunPublication::Unbound
    );

    let result = harness.publish(&request, Some(&first)).unwrap();

    assert_eq!(result.status, PublicationStatus::Completed);
    let prepared = harness
        .journal
        .checkpoints
        .iter()
        .find(|progress| progress.unresolved.is_some())
        .unwrap();
    assert_eq!(prepared.coverage_binding, Some(bound(&first, 1)));
    assert!(!prepared.dispatch_may_have_started);
    assert_eq!(
        prepared.unresolved.as_ref().unwrap().phase,
        WritePhase::Prepared
    );
    let log = harness.host.log.borrow().clone();
    let first_write = log
        .iter()
        .position(|entry| entry.starts_with("write:"))
        .unwrap();
    let flagged = format!(
        "checkpoint:bound={}:dispatch=true:phase=Some(DispatchPossible)",
        first.run_id
    );
    assert_eq!(log[first_write - 1], flagged);
    assert!(log[..first_write - 1].iter().any(|entry| *entry
        == format!(
            "checkpoint:bound={}:dispatch=false:phase=Some(Prepared)",
            first.run_id
        )));
    assert_eq!(harness.progress().coverage_binding, Some(bound(&first, 1)));
    assert!(harness.progress().dispatch_may_have_started);
    assert_eq!(
        harness.progress().run_publication(&first.run_id),
        RunPublication::BoundUnsettled(&bound(&first, 1))
    );

    let settled = harness.settle(&first, &result).unwrap().unwrap();
    assert_eq!(settled.outcome, SettledOutcome::Published);
    assert_eq!(settled.round, 1);
    assert_eq!(settled.reason, None);
    assert_eq!(harness.progress().coverage_binding, Some(bound(&first, 1)));
    assert_eq!(
        harness.progress().run_publication(&first.run_id),
        RunPublication::Settled(&settled)
    );
}

#[test]
fn dispatch_flag_survives_a_settled_summary_while_the_next_finding_is_only_prepared() {
    // F24: the run's own posted summary must not reopen conversation preflight.
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    harness.journal.fail_before_write = Some(2);

    let stopped = harness.publish(&request, Some(&first)).unwrap();

    assert!(matches!(stopped.stop, Some(Stop::Evidence(_))));
    assert_eq!(effects(&harness.host), vec![Effect::CreateSummary]);
    let persisted = harness.journal.persisted.clone().unwrap();
    assert_eq!(&persisted, harness.progress());
    let pending = persisted.unresolved.as_ref().unwrap();
    assert_eq!(pending.phase, WritePhase::Prepared);
    assert!(matches!(pending.write, Write::Finding { .. }));
    assert!(persisted.dispatch_may_have_started);
    assert_eq!(persisted.coverage_binding, Some(bound(&first, 1)));

    harness.journal.fail_before_write = None;
    harness.reset_observations();
    let resumed = harness.publish(&request, Some(&first)).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert!(
        harness
            .journal
            .checkpoints
            .iter()
            .all(|progress| progress.dispatch_may_have_started)
    );
}

#[test]
fn definite_rejection_of_the_first_bound_write_restores_the_previous_binding() {
    // Deviation 6: a rejected first write leaves the earlier settled owner in place.
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let initial = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&first),
        )
        .unwrap();
    harness.settle(&first, &initial).unwrap();
    let before = harness.progress().clone();
    let second = bind(2, 'c');
    let installed = bound_after(&second, 2, before.authorization.clone());
    let follow_up = request("fixed", Some(FindingStatus::Fixed), false);
    harness.host.fault = Some(Fault::Rejected(Effect::UpdateSummary));
    harness.reset_observations();

    let rejected = harness.publish(&follow_up, Some(&second)).unwrap();

    assert_eq!(rejected.status, PublicationStatus::NotStarted);
    assert!(rejected.verified.is_empty());
    assert_eq!(harness.progress(), &before);
    assert_eq!(harness.journal.persisted.as_ref(), Some(&before));
    assert!(
        harness
            .journal
            .checkpoints
            .iter()
            .any(|progress| progress.coverage_binding == Some(installed.clone()))
    );
    assert_eq!(
        harness.progress().run_publication(&second.run_id),
        RunPublication::Unbound
    );

    let published = harness.publish(&follow_up, Some(&second)).unwrap();
    assert_eq!(published.status, PublicationStatus::Completed);
    assert_eq!(harness.progress().coverage_binding, Some(installed));
    assert!(harness.progress().dispatch_may_have_started);
}

#[test]
fn resuming_a_run_with_different_accepted_inputs_is_rejected_before_any_write() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let uncertain = harness.publish(&request, Some(&first)).unwrap();
    assert_eq!(uncertain.status, PublicationStatus::Uncertain);
    let before = harness.progress().clone();
    harness.reset_observations();

    let changed_assessment = bind(1, 'c');
    let changed_files = BindingRequest {
        snapshot_files: files('d'),
        ..first.clone()
    };
    for changed in [changed_assessment, changed_files] {
        assert_eq!(
            binding_error(harness.publish(&request, Some(&changed))),
            Some(BindingError::Mismatch(first.run_id.clone()))
        );
        assert!(harness.host.writes.is_empty());
        assert!(
            !harness
                .host
                .log
                .borrow()
                .iter()
                .any(|entry| entry == "acquire")
        );
        assert_eq!(harness.progress(), &before);
    }
}

#[test]
fn a_resumed_binding_must_still_describe_the_planned_round() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    harness.host.progress.coverage_binding = Some(bound(&first, 5));
    harness.host.progress.dispatch_may_have_started = true;
    let before = harness.progress().clone();

    let refused = harness.publish(
        &request("initial", Some(FindingStatus::Open), false),
        Some(&first),
    );

    assert_eq!(
        binding_error(refused),
        Some(BindingError::Mismatch(first.run_id.clone()))
    );
    assert!(harness.host.writes.is_empty());
    assert_eq!(harness.progress(), &before);
}

#[test]
fn another_run_waits_for_the_pending_owner_without_touching_its_retries() {
    // D14, F19, C09 and F22.
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request_one = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    harness.publish(&request_one, Some(&first)).unwrap();
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let retried = harness.publish(&request_one, Some(&first)).unwrap();
    assert_eq!(retried.status, PublicationStatus::Uncertain);
    let consumed = harness.progress().retried_creations.clone();
    assert_eq!(consumed.len(), 1);
    let pending = harness.progress().clone();
    harness.reset_observations();

    let second = bind(2, 'c');
    for request_two in [
        request("initial", Some(FindingStatus::Open), false),
        request("fixed", Some(FindingStatus::Fixed), false),
    ] {
        assert_eq!(
            binding_error(harness.publish(&request_two, Some(&second))),
            Some(BindingError::PendingOwner(first.run_id.clone()))
        );
    }
    let abandon = AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: 1,
        reason: "Replace the pending review.".into(),
        effects: Effects::review(),
    };
    assert_eq!(
        binding_error(harness.abandon(&abandon, Some(&second))),
        Some(BindingError::PendingOwner(first.run_id.clone()))
    );
    assert!(harness.host.writes.is_empty());
    assert!(harness.journal.checkpoints.is_empty());
    assert_eq!(harness.progress(), &pending);

    observe_pending_create(&mut harness.host);
    let recovered = harness.publish(&request_one, Some(&first)).unwrap();
    assert_eq!(recovered.status, PublicationStatus::Completed);
    let settled = harness.settle(&first, &recovered).unwrap().unwrap();
    assert_eq!(settled.outcome, SettledOutcome::Published);
    let first_grant = harness.progress().authorization.clone();

    let follow_up = harness
        .publish(
            &request("fixed", Some(FindingStatus::Fixed), false),
            Some(&second),
        )
        .unwrap();
    assert_eq!(follow_up.status, PublicationStatus::Completed);
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&second, 2, first_grant))
    );
    assert_eq!(harness.progress().settled_runs, vec![settled.clone()]);
    assert_eq!(harness.progress().retried_creations, consumed);
    assert_eq!(
        harness.progress().run_publication(&first.run_id),
        RunPublication::Settled(&settled)
    );
}

#[test]
fn a_terminal_provider_round_settles_an_owner_whose_local_settlement_was_lost() {
    // Deviation 2: completion without a recorded settlement must not block later
    // runs. The owner dispatched its writes, so the provider round is its own.
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&first),
        )
        .unwrap();
    assert!(harness.progress().dispatch_may_have_started);
    let first_grant = harness.progress().authorization.clone();
    harness.reset_observations();
    let second = bind(2, 'c');

    let follow_up = harness
        .publish(
            &request("fixed", Some(FindingStatus::Fixed), false),
            Some(&second),
        )
        .unwrap();

    assert_eq!(follow_up.status, PublicationStatus::Completed);
    let expected = SettledRun {
        run_id: first.run_id.clone(),
        review_assessment: first.review_assessment.clone(),
        snapshot_files: first.snapshot_files.clone(),
        round: 1,
        outcome: SettledOutcome::Published,
        reason: None,
        verified_at: harness.clock.0,
    };
    assert_eq!(harness.progress().settled_runs, vec![expected.clone()]);
    let checkpoints = &harness.journal.checkpoints;
    let settled_first = checkpoints
        .iter()
        .position(|progress| progress.settled_runs == vec![expected.clone()])
        .unwrap();
    let installed_second = checkpoints
        .iter()
        .position(|progress| {
            progress.coverage_binding == Some(bound_after(&second, 2, first_grant.clone()))
        })
        .unwrap();
    assert!(settled_first < installed_second);
    assert_eq!(
        checkpoints[settled_first].coverage_binding,
        Some(bound(&first, 1))
    );
}

#[test]
fn an_unfinished_provider_round_keeps_the_owner_until_abandonment_settles_it() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    harness.host.authority = Effects::new([Effect::CreateSummary, Effect::UpdateSummary]);
    let partial = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&first),
        )
        .unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    assert!(harness.progress().unresolved.is_none());
    harness.host.authority = Effects::review();
    let second = bind(2, 'c');
    let replacement = request("blocked", None, true);
    let before = harness.progress().clone();

    assert_eq!(
        binding_error(harness.publish(&replacement, Some(&second))),
        Some(BindingError::PendingOwner(first.run_id.clone()))
    );
    assert_eq!(harness.progress(), &before);

    let abandon = abandonment(&harness.host);
    let abandoned = harness.abandon(&abandon, Some(&first)).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    let first_grant = harness.progress().authorization.clone();
    harness.reset_observations();

    let blocked = harness.publish(&replacement, Some(&second)).unwrap();
    assert_eq!(blocked.status, PublicationStatus::Blocked);
    assert_eq!(
        harness.progress().settled_runs,
        vec![SettledRun {
            run_id: first.run_id.clone(),
            review_assessment: first.review_assessment.clone(),
            snapshot_files: first.snapshot_files.clone(),
            round: 1,
            outcome: SettledOutcome::Abandoned,
            reason: Some(abandon.reason.clone()),
            verified_at: harness.clock.0,
        }]
    );
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&second, 2, first_grant))
    );
}

#[test]
fn bound_abandonment_settles_the_run_as_abandoned_and_then_refuses_it() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    harness.host.authority = Effects::new([Effect::CreateSummary, Effect::UpdateSummary]);
    harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&first),
        )
        .unwrap();
    harness.host.authority = Effects::review();
    let abandon = abandonment(&harness.host);

    let wrong_round = AbandonRequest {
        round: 2,
        ..abandon.clone()
    };
    assert_eq!(
        binding_error(harness.abandon(&wrong_round, Some(&first))),
        Some(BindingError::Mismatch(first.run_id.clone()))
    );
    let abandoned = harness.abandon(&abandon, Some(&first)).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    let settled = harness.settle(&first, &abandoned).unwrap().unwrap();

    assert_eq!(settled.outcome, SettledOutcome::Abandoned);
    assert_eq!(settled.reason.as_deref(), Some(abandon.reason.as_str()));
    assert_eq!(harness.progress().coverage_binding, Some(bound(&first, 1)));
    assert_eq!(
        binding_error(harness.abandon(&abandon, Some(&first))),
        Some(BindingError::AlreadySettled(first.run_id.clone()))
    );
    assert_eq!(
        binding_error(harness.publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&first)
        )),
        Some(BindingError::AlreadySettled(first.run_id.clone()))
    );
}

#[test]
fn unchanged_publication_settles_no_changes_or_published_by_dispatch_history() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    harness.publish(&request, Some(&first)).unwrap();

    harness.reset_observations();
    let repeated = harness.publish(&request, Some(&first)).unwrap();
    assert_eq!(repeated.status, PublicationStatus::Unchanged);
    assert!(harness.host.writes.is_empty());
    let first_settled = harness.settle(&first, &repeated).unwrap().unwrap();
    assert_eq!(first_settled.outcome, SettledOutcome::Published);

    let second = bind(2, 'b');
    let unchanged = harness.publish(&request, Some(&second)).unwrap();
    assert_eq!(unchanged.status, PublicationStatus::Unchanged);
    assert!(harness.host.writes.is_empty());
    assert_eq!(harness.progress().coverage_binding, Some(bound(&first, 1)));
    let second_settled = harness.settle(&second, &unchanged).unwrap().unwrap();

    assert_eq!(second_settled.outcome, SettledOutcome::NoChanges);
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(
            &second,
            1,
            harness.progress().authorization.clone()
        ))
    );
    assert!(!harness.progress().dispatch_may_have_started);
    assert_eq!(
        harness.progress().settled_runs,
        vec![first_settled, second_settled.clone()]
    );
    assert_eq!(
        harness.settle(&second, &unchanged).unwrap(),
        Some(second_settled)
    );
}

#[test]
fn unbound_publication_refuses_a_run_bound_journal() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    harness.publish(&request, Some(&first)).unwrap();
    let before = harness.progress().clone();
    harness.reset_observations();

    assert_eq!(
        binding_error(harness.publish(&request, None)),
        Some(BindingError::BindingRequired)
    );
    assert!(matches!(
        publish(
            &request,
            &mut harness.host,
            &mut harness.source,
            &mut harness.renderer
        ),
        Err(PlanError::Binding(BindingError::BindingRequired))
    ));
    assert!(harness.host.writes.is_empty());
    assert_eq!(harness.progress(), &before);
}

#[test]
fn generic_unresolved_progress_refuses_a_bound_request_without_mutation() {
    let mut harness = Harness::new();
    let request = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(
        &request,
        &mut harness.host,
        &mut harness.source,
        &mut harness.renderer,
    )
    .unwrap();
    assert!(harness.progress().coverage_binding.is_none());
    let before = harness.progress().clone();
    harness.reset_observations();

    assert_eq!(
        binding_error(harness.publish(&request, Some(&bind(1, 'b')))),
        Some(BindingError::Unresolved)
    );
    assert!(harness.host.writes.is_empty());
    assert_eq!(harness.progress(), &before);
}

#[test]
fn planning_never_changes_the_binding_or_dispatch_flag() {
    let mut harness = Harness::new();
    let first = bind(1, 'b');
    let request = request("initial", Some(FindingStatus::Open), false);
    for progress in [
        PublicationProgress::default(),
        PublicationProgress {
            coverage_binding: Some(bound(&first, 1)),
            dispatch_may_have_started: true,
            ..Default::default()
        },
    ] {
        harness.host.progress = progress.clone();
        for binding in [Some(&first), Some(&bind(2, 'c')), None] {
            let outcome = harness.run(&request, binding, RunMode::Plan).unwrap();
            assert!(matches!(outcome, RunOutcome::Planned(_)));
            assert_eq!(harness.progress(), &progress);
        }
    }
    assert!(harness.host.writes.is_empty());
    assert!(harness.journal.checkpoints.is_empty());
}
