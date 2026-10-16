use super::*;
use review_kit_application::use_cases::{
    publication_run::{ExecutionEvent, ExecutionEvidence},
    publishing::{RecoveryPolicy, WritePhase},
};

#[test]
fn absent_creations_retry_once_and_completed_retries_write_nothing() {
    for effect in [Effect::CreateSummary, Effect::Finding, Effect::Reply] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = if effect == Effect::Reply {
            seed_completed(&mut host, &mut source, &mut renderer);
            request("fixed", Some(FindingStatus::Fixed), false)
        } else {
            request("initial", Some(FindingStatus::Open), false)
        };
        host.fault = Some(Fault::UnknownAbsent(effect));
        let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(first.status, PublicationStatus::Uncertain);
        let retained_write = host.progress.unresolved.as_ref().unwrap().write.clone();
        host.writes.clear();
        let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(recovered.status, PublicationStatus::Completed, "{effect:?}");
        assert_eq!(recovered.round, first.round);
        assert_eq!(recovered.retried_creations, vec![retained_write.clone()]);
        assert_eq!(
            host.writes.iter().filter(|w| *w == &retained_write).count(),
            1
        );
        assert!(host.progress.unresolved.is_none());
        host.writes.clear();
        let unchanged = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(unchanged.status, PublicationStatus::Unchanged);
        assert_eq!(unchanged.round, first.round);
        assert!(host.writes.is_empty());
    }
}

#[test]
fn unknown_or_rejected_resend_keeps_original_ambiguity_and_consumed_budget() {
    for rejected in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = request("initial", Some(FindingStatus::Open), false);
        host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
        publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        let original = host.progress.unresolved.clone().unwrap();
        host.fault = Some(if rejected {
            Fault::Rejected(Effect::Finding)
        } else {
            Fault::UnknownAbsent(Effect::Finding)
        });
        host.writes.clear();
        let retry = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(retry.status, PublicationStatus::Uncertain);
        assert_eq!(
            host.progress.retried_creations,
            vec![original.write.clone()]
        );
        assert_eq!(host.progress.unresolved, Some(original.clone()));
        assert_eq!(host.writes.len(), 1);
        host.writes.clear();
        for _ in 0..3 {
            let stopped = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
            assert_eq!(stopped.status, PublicationStatus::Uncertain);
            assert!(host.writes.is_empty());
            assert_eq!(host.progress.unresolved, Some(original.clone()));
        }
        observe_pending_create(&mut host);
        assert_eq!(
            publish(&req, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Completed
        );
        assert!(
            host.writes
                .iter()
                .all(|write| write.effect() != Effect::Finding)
        );
    }
}

#[test]
fn conservative_recovery_observes_effects_but_never_resends() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.progress.recovery_policy = RecoveryPolicy::Conservative;
    host.writes.clear();
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert!(host.writes.is_empty());
    observe_pending_create(&mut host);
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
}

#[test]
fn an_original_round_grant_survives_settled_progress_and_broader_invocations() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.authority = Effects::new([Effect::CreateSummary, Effect::UpdateSummary]);
    let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(first.status, PublicationStatus::Partial);
    assert!(host.progress.unresolved.is_none());
    assert!(
        !host
            .progress
            .authorization
            .as_ref()
            .unwrap()
            .effects
            .contains(Effect::Finding)
    );
    host.authority = Effects::review();
    host.writes.clear();
    let next = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(next.round, first.round);
    assert!(!effects(&host).contains(&Effect::Finding));
    assert!(
        !host
            .progress
            .authorization
            .as_ref()
            .unwrap()
            .effects
            .contains(Effect::Finding)
    );
}

struct FailPrepared;
impl ExecutionEvidence for FailPrepared {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        if matches!(event, ExecutionEvent::BeforeWrite(_)) {
            Err(failure("prepared evidence"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn interruption_after_preparation_replays_exact_intent_even_in_conservative_mode() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    let stopped = publish_recorded(
        &req,
        &mut host,
        &mut source,
        &mut renderer,
        &mut FailPrepared,
    )
    .unwrap();
    assert_eq!(stopped.status, PublicationStatus::NotStarted);
    assert!(host.writes.is_empty());
    let retained = host.progress.unresolved.as_ref().unwrap().write.clone();
    assert_eq!(
        host.progress.unresolved.as_ref().unwrap().phase,
        WritePhase::Prepared
    );
    host.progress.recovery_policy = RecoveryPolicy::Conservative;
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert!(resumed.retried_creations.is_empty());
    assert_eq!(host.writes.first(), Some(&retained));
}

#[test]
fn legacy_missing_creation_never_gains_dispatch_certainty_or_retry_authority() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    let pending = host.progress.unresolved.as_mut().unwrap();
    pending.phase = WritePhase::LegacyAmbiguous;
    pending.baseline = None;
    host.progress.authorization = None;
    host.writes.clear();
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert!(host.writes.is_empty());
}

#[test]
fn temporarily_narrower_authority_can_restore_only_the_original_ceiling() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    let grant = host.progress.authorization.clone();
    host.authority = Effects::none();
    host.writes.clear();
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert_eq!(host.progress.authorization, grant);
    assert!(host.writes.is_empty());
    host.authority = Effects::review();
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    assert_eq!(host.progress.authorization, grant);
}

#[test]
fn legacy_partial_progress_cannot_invent_an_original_authority_ceiling() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::Rejected(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Partial
    );
    host.progress.authorization = None;
    host.writes.clear();
    let stopped = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(stopped.status, PublicationStatus::Denied);
    assert!(host.writes.is_empty());
    assert!(host.progress.authorization.is_none());
}

struct FailDispatch;
impl ExecutionEvidence for FailDispatch {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        if matches!(event, ExecutionEvent::Progress { progress, .. }
            if progress.unresolved.as_ref().is_some_and(|pending| pending.phase == WritePhase::DispatchPossible))
        {
            Err(failure("dispatch checkpoint"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn failed_dispatch_checkpoint_restores_prepared_phase_and_unconsumed_resend() {
    for retry in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = request("initial", Some(FindingStatus::Open), false);
        if retry {
            host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
            publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        }
        let retained = host.progress.unresolved.clone();
        host.writes.clear();
        let stopped = publish_recorded(
            &req,
            &mut host,
            &mut source,
            &mut renderer,
            &mut FailDispatch,
        )
        .unwrap();
        assert!(matches!(stopped.stop, Some(Stop::Evidence(_))));
        assert!(host.writes.is_empty());
        assert!(host.progress.retried_creations.is_empty());
        if retry {
            assert_eq!(host.progress.unresolved, retained);
        } else {
            assert_eq!(
                host.progress.unresolved.as_ref().unwrap().phase,
                WritePhase::Prepared
            );
        }
        assert_eq!(
            publish(&req, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Completed
        );
    }
}

#[test]
fn unrelated_human_reply_does_not_hide_an_exact_observed_finding() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    observe_pending_create(&mut host);
    host.snapshot.replies.push(ReplyRecord {
        id: provider_id("unrelated-reply"),
        thread: provider_id("owned-root"),
        author: reviewer("human"),
        body: "Another comment.".into(),
        action: None,
    });
    host.writes.clear();
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert!(host.progress.unresolved.is_none());
    assert!(!effects(&host).contains(&Effect::Finding));
}

#[test]
fn matching_resolution_from_another_actor_keeps_observed_provenance() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Resolve));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.id == provider_id("owned-root"))
        .unwrap()
        .resolved = true;
    host.writes.clear();
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert!(!effects(&host).contains(&Effect::Resolve));
    let round = &host.snapshot.summaries[0].current;
    assert_eq!(
        round.verified_resolutions[0].provenance,
        ResolutionProvenance::Observed
    );
    let actions = &round.communicated_findings.as_ref().unwrap()[0].actions;
    assert!(actions.contains(&PublishedAction::ObservedResolved));
    assert!(!actions.contains(&PublishedAction::Resolved));
}

#[test]
fn a_resend_survives_failure_of_subsequent_preflight_in_the_result() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.fail_on_discovery = Some(host.discoveries + 6);
    host.writes.clear();
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Uncertain);
    assert!(matches!(
        resumed.stop,
        Some(Stop::Preparation(PlanError::Discovery(_)))
    ));
    assert_eq!(resumed.attempted.len(), 1);
    assert_eq!(resumed.verified.len(), 1);
    assert_eq!(resumed.retried_creations.len(), 1);
    assert_eq!(effects(&host), vec![Effect::Finding]);
    assert!(
        resumed
            .final_snapshot
            .findings
            .iter()
            .any(|root| root.author == reviewer("agent"))
    );
}

#[test]
fn legacy_resolution_observation_clears_intent_without_inventing_historical_receipts() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Resolve));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    let pending = host.progress.unresolved.as_mut().unwrap();
    pending.phase = WritePhase::LegacyAmbiguous;
    pending.baseline = None;
    host.progress.authorization = None;
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.author == reviewer("agent"))
        .unwrap()
        .resolved = true;
    host.writes.clear();
    let stopped = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(stopped.status, PublicationStatus::Denied);
    assert!(host.progress.unresolved.is_none());
    assert!(host.progress.verified_resolutions.is_empty());
    assert!(host.writes.is_empty());
}

#[test]
fn explicit_abandonment_resumes_prepared_intent_with_current_authority() {
    for legacy in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = request("initial", Some(FindingStatus::Open), false);
        host.authority = Effects::new([Effect::CreateSummary, Effect::Finding]);
        publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        if legacy {
            host.progress.authorization = None;
        }
        host.authority = Effects::new([Effect::UpdateSummary]);
        host.writes.clear();
        let request = review_kit_application::use_cases::publishing::AbandonRequest {
            subject: subject(),
            reviewer: reviewer("agent"),
            round: host.snapshot.summaries[0].current.number,
            reason: "Close this exact round.".into(),
            effects: Effects::review(),
        };
        let mut progress = std::mem::take(&mut host.progress);
        let stopped = review_kit_application::use_cases::publishing::abandon_recorded(
            &request,
            &mut host,
            &mut renderer,
            &mut progress,
            &mut FailPrepared,
        )
        .unwrap();
        host.progress = progress;
        assert_eq!(stopped.status, PublicationStatus::NotStarted);
        assert_eq!(
            host.progress.unresolved.as_ref().unwrap().phase,
            WritePhase::Prepared
        );
        assert!(host.writes.is_empty());
        let exact = host.progress.unresolved.as_ref().unwrap().write.clone();
        let mut progress = std::mem::take(&mut host.progress);
        let resumed = review_kit_application::use_cases::publishing::abandon(
            &request,
            &mut host,
            &mut renderer,
            &mut progress,
        )
        .unwrap();
        host.progress = progress;
        assert_eq!(resumed.status, PublicationStatus::Abandoned);
        assert_eq!(host.writes, vec![exact]);
        host.writes.clear();
        let mut progress = std::mem::take(&mut host.progress);
        let unchanged = review_kit_application::use_cases::publishing::abandon(
            &request,
            &mut host,
            &mut renderer,
            &mut progress,
        )
        .unwrap();
        host.progress = progress;
        assert_eq!(unchanged.status, PublicationStatus::Abandoned);
        assert!(host.writes.is_empty());
    }
}

#[test]
fn denied_or_rejected_unstarted_attempts_can_begin_with_a_later_grant() {
    for rejected in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = request("initial", None, false);
        host.authority = if rejected {
            Effects::new([Effect::CreateSummary])
        } else {
            Effects::none()
        };
        if rejected {
            host.fault = Some(Fault::Rejected(Effect::CreateSummary));
        }
        let stopped = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert!(matches!(
            stopped.status,
            PublicationStatus::Denied | PublicationStatus::NotStarted
        ));
        assert!(host.snapshot.summaries.is_empty());
        assert!(host.progress.authorization.is_none());
        host.authority = Effects::review();
        assert_eq!(
            publish(&req, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Completed
        );
    }
}

#[test]
fn deletion_and_rejected_recreation_cannot_reset_an_established_round_ceiling() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", None, false);
    host.authority = Effects::new([Effect::CreateSummary]);
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    let original = host.progress.authorization.clone();
    host.snapshot.summaries.clear();
    host.authority = Effects::review();
    host.fault = Some(Fault::Rejected(Effect::CreateSummary));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(host.progress.authorization, original);
    host.writes.clear();
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert!(!effects(&host).contains(&Effect::UpdateSummary));
    assert_eq!(host.progress.authorization, original);
}

#[test]
fn findings_in_one_round_keep_distinct_retry_budgets_and_cumulative_warnings() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = request("initial", Some(FindingStatus::Open), false);
    req.allow_colocated = true;
    replace_findings(
        &mut req,
        vec![finding(FindingStatus::Open), numbered_finding(2)],
    );
    host.fault = Some(Fault::UnknownAbsentFinding(FindingId::try_from(1).unwrap()));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.fault = Some(Fault::UnknownAbsentFinding(FindingId::try_from(2).unwrap()));
    let second = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(second.status, PublicationStatus::Uncertain);
    assert_eq!(second.retried_creations.len(), 1);
    host.fault = Some(Fault::UnknownAbsentFinding(FindingId::try_from(2).unwrap()));
    host.writes.clear();
    let third = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(third.status, PublicationStatus::Uncertain);
    assert_eq!(third.retried_creations.len(), 2);
    assert_eq!(host.progress.retried_creations.len(), 2);
    assert_eq!(effects(&host), vec![Effect::Finding]);
    host.writes.clear();
    for _ in 0..2 {
        let capped = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(capped.retried_creations.len(), 2);
        assert!(host.writes.is_empty());
    }
}

#[test]
fn post_resend_discovery_conflict_preserves_the_effect_and_typed_cause() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.duplicate_root_on_discovery = Some(host.discoveries + 6);
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(
        result.stop,
        Some(Stop::Preparation(PlanError::Discovery(_)))
    ));
    assert_eq!(result.attempted.len(), 1);
    assert_eq!(result.verified.len(), 1);
    assert_eq!(result.retried_creations.len(), 1);
    assert_eq!(effects(&host), vec![Effect::Finding]);
}

#[test]
fn rejection_of_a_new_round_summary_restores_the_previous_round_grant() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let previous = host.progress.authorization.clone();
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    host.authority = Effects::new([Effect::UpdateSummary]);
    host.fault = Some(Fault::Rejected(Effect::UpdateSummary));
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(host.progress.authorization, previous);
    assert_eq!(host.snapshot.summaries[0].current.number, 1);
    host.authority = Effects::review();
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert_eq!(resumed.round, 2);
    assert_eq!(
        host.progress.authorization.as_ref().unwrap().effects,
        Effects::review()
    );
}
