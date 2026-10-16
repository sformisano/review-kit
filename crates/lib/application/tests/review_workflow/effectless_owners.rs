//! Owners whose binding never reached the provider, and owners whose writes may have.

use super::coverage_binding::{Harness, abandonment, bind, binding_error, bound, bound_after};
use super::*;
use review_kit_application::{
    review_run::SettledOutcome,
    use_cases::publishing::{
        AbandonRequest, BindingError, BindingRequest, RoundAuthorization, RunPublication,
        UnresolvedWrite, WritePhase,
    },
};

fn new_head() -> Revision {
    Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap()
}

fn summary_effects() -> Effects {
    Effects::new([Effect::CreateSummary, Effect::UpdateSummary])
}

fn grant(effects: Effects) -> Option<RoundAuthorization> {
    Some(RoundAuthorization { round: 1, effects })
}

/// Stop run A after its first prepared intent is persisted, then restart from
/// the persisted journal as a new process would.
fn crash_after_first_intent(
    harness: &mut Harness,
    request: &PublishRequest,
    run: &BindingRequest,
) -> Write {
    harness.journal.fail_before_write = Some(harness.journal.before_writes + 1);
    let stopped = harness.publish(request, Some(run)).unwrap();
    assert!(matches!(stopped.stop, Some(Stop::Evidence(_))));
    let persisted = harness.journal.persisted.clone().unwrap();
    assert!(!persisted.dispatch_may_have_started);
    let pending = persisted.unresolved.clone().unwrap();
    assert_eq!(pending.phase, WritePhase::Prepared);
    harness.host.progress = persisted;
    harness.journal.fail_before_write = None;
    pending.write
}

/// Run B's fresh review of the new head, with the full review grant.
fn review_new_head(harness: &mut Harness) -> PublishRequest {
    let mut fresh = request("initial", Some(FindingStatus::Open), false);
    replace_revision(&mut fresh, new_head());
    harness.host.snapshot.revision = new_head();
    harness.source.expected_revision = new_head();
    fresh
}

/// Index of the first checkpoint that installs `binding`'s run.
fn installation(harness: &Harness, run: u8) -> usize {
    let run_id = bind(run, 'a').run_id;
    harness
        .journal
        .checkpoints
        .iter()
        .position(|progress| {
            progress
                .coverage_binding
                .as_ref()
                .is_some_and(|binding| binding.run_id == run_id)
        })
        .unwrap()
}

#[test]
fn a_stale_unsent_owner_is_released_by_the_next_run_without_capping_its_grant() {
    // F3: crash after the prepared intent (boundary 6), then a new head.
    let mut harness = Harness::new();
    let a = bind(1, 'b');
    let mut a_request = request("blocked", None, true);
    a_request.effects = summary_effects();
    let stale = crash_after_first_intent(&mut harness, &a_request, &a);
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&a, 1, None))
    );
    assert_eq!(harness.progress().authorization, grant(summary_effects()));
    harness.host.snapshot.revision = new_head();
    harness.reset_observations();

    // The owner's stale prepared intent never left: its own publication drops
    // it, keeps the binding and the clear flag, and the current plan refuses.
    let refused = harness.publish(&a_request, Some(&a)).unwrap_err();
    assert!(
        has_plan_error(&refused, &|error| matches!(
            error,
            PlanError::ChangedRevision
        )),
        "{refused:?}"
    );
    assert_eq!(harness.progress().unresolved, None);
    assert!(!harness.progress().dispatch_may_have_started);
    assert_eq!(harness.progress().authorization, grant(summary_effects()));
    let abandon = abandonment_of_round_one();
    assert!(matches!(
        harness.abandon(&abandon, Some(&a)),
        Err(PlanError::NotAbandonable)
    ));
    assert!(harness.host.writes.is_empty());
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::BoundUnsent(&bound(&a, 1))
    );

    let b = bind(2, 'c');
    let b_request = review_new_head(&mut harness);
    let completed = harness.publish(&b_request, Some(&b)).unwrap();

    assert_eq!(completed.status, PublicationStatus::Completed);
    assert!(effects(&harness.host).contains(&Effect::Finding));
    assert!(!harness.host.writes.contains(&stale));
    let released = &harness.journal.checkpoints[installation(&harness, 2) - 1];
    assert_eq!(released.coverage_binding, None);
    assert_eq!(released.unresolved, None);
    assert_eq!(released.authorization, None);
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&b, 1, None))
    );
    assert_eq!(harness.progress().authorization, grant(Effects::review()));
    assert!(harness.progress().settled_runs.is_empty());
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::Unbound
    );
}

fn abandonment_of_round_one() -> AbandonRequest {
    AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: 1,
        reason: "Replace the pending review.".into(),
        effects: Effects::review(),
    }
}

#[test]
fn an_unsent_owner_with_stale_inputs_at_the_same_head_is_released_by_the_next_run() {
    // A conversation change makes A's inputs stale; the CLI refuses to resume A.
    let mut harness = Harness::new();
    let mut a_request = request("blocked", None, true);
    a_request.effects = summary_effects();
    let stale = crash_after_first_intent(&mut harness, &a_request, &bind(1, 'b'));
    harness.reset_observations();

    let b = bind(2, 'c');
    let completed = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&b),
        )
        .unwrap();

    assert_eq!(completed.status, PublicationStatus::Completed);
    assert!(effects(&harness.host).contains(&Effect::Finding));
    assert!(!harness.host.writes.contains(&stale));
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&b, 1, None))
    );
    assert_eq!(harness.progress().authorization, grant(Effects::review()));
    assert_eq!(
        harness.progress().run_publication(&bind(1, 'b').run_id),
        RunPublication::Unbound
    );
}

#[test]
fn a_possibly_dispatched_owner_keeps_its_ceiling_until_it_reconciles_and_abandons() {
    let mut harness = Harness::new();
    let a = bind(1, 'b');
    let a_request = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
    let uncertain = harness.publish(&a_request, Some(&a)).unwrap();
    assert_eq!(uncertain.status, PublicationStatus::Uncertain);
    harness.host.snapshot.revision = new_head();
    harness.reset_observations();

    let resumed = harness.publish(&a_request, Some(&a)).unwrap();
    assert!(matches!(resumed.stop, Some(Stop::UnresolvedWrite)));
    assert!(harness.host.writes.is_empty());
    let pending = harness.progress().clone();
    assert_eq!(
        pending.unresolved.as_ref().map(|write| write.phase),
        Some(WritePhase::DispatchPossible)
    );
    assert_eq!(
        pending.run_publication(&a.run_id),
        RunPublication::BoundUnsettled(&bound(&a, 1))
    );
    harness.reset_observations();

    let b = bind(2, 'c');
    let mut b_request = review_new_head(&mut harness);
    // Round 1 reserves finding 1 although its root was never created.
    let fresh = copy_finding(
        &finding(FindingStatus::Open),
        FindingId::try_from(2).unwrap(),
        policy(),
        None,
    );
    replace_findings(&mut b_request, vec![fresh]);
    assert_eq!(
        binding_error(harness.publish(&b_request, Some(&b))),
        Some(BindingError::PendingOwner(a.run_id.clone()))
    );
    assert_eq!(harness.progress(), &pending);
    assert!(harness.host.writes.is_empty());
    assert!(harness.journal.checkpoints.is_empty());

    observe_pending_create(&mut harness.host);
    let abandon = abandonment(&harness.host);
    let abandoned = harness.abandon(&abandon, Some(&a)).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    let settled = harness.settle(&a, &abandoned).unwrap().unwrap();
    assert_eq!(settled.outcome, SettledOutcome::Abandoned);
    assert_eq!(
        harness.progress().retried_creations,
        pending.retried_creations
    );

    let completed = harness.publish(&b_request, Some(&b)).unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(completed.round, 2);
    assert_eq!(harness.progress().settled_runs, vec![settled]);
}

#[test]
fn an_existing_round_ceiling_survives_the_release_of_an_unsent_owner() {
    let mut harness = Harness::new();
    harness.host.progress.authorization = grant(summary_effects());
    let a = bind(1, 'b');
    crash_after_first_intent(
        &mut harness,
        &request("initial", Some(FindingStatus::Open), false),
        &a,
    );
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&a, 1, grant(summary_effects())))
    );
    harness.reset_observations();

    let b = bind(2, 'c');
    let blocked = harness
        .publish(&request("blocked", None, true), Some(&b))
        .unwrap();

    assert_eq!(blocked.status, PublicationStatus::Blocked);
    let released = &harness.journal.checkpoints[installation(&harness, 2) - 1];
    assert_eq!(released.coverage_binding, None);
    assert_eq!(released.authorization, grant(summary_effects()));
    assert_eq!(
        harness.progress().coverage_binding,
        Some(bound_after(&b, 1, grant(summary_effects())))
    );
    assert_eq!(harness.progress().authorization, grant(summary_effects()));
}

#[test]
fn an_effectless_owner_is_released_not_settled_from_a_predating_terminal_round() {
    // F1: run A reuses blocked round 1 of run P, never dispatches, then a resend
    // of its prepared intent is definitely rejected.
    let mut harness = Harness::new();
    let p = bind(1, 'b');
    let blocked = harness
        .publish(&request("blocked", None, true), Some(&p))
        .unwrap();
    let p_settled = harness.settle(&p, &blocked).unwrap().unwrap();
    let p_grant = harness.progress().authorization.clone();
    let a = bind(2, 'c');
    let a_request = request("ready", None, false);
    crash_after_first_intent(&mut harness, &a_request, &a);
    harness.host.fault = Some(Fault::Rejected(Effect::UpdateSummary));
    let rejected = harness.publish(&a_request, Some(&a)).unwrap();
    assert!(matches!(rejected.stop, Some(Stop::Rejected(_))));
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::BoundUnsent(&bound_after(&a, 1, p_grant.clone()))
    );
    harness.host.fault = None;
    harness.reset_observations();

    let b = bind(3, 'd');
    let completed = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&b),
        )
        .unwrap();

    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(harness.progress().settled_runs, vec![p_settled]);
    let released = &harness.journal.checkpoints[installation(&harness, 3) - 1];
    assert_eq!(released.coverage_binding, Some(bound(&p, 1)));
    assert!(released.dispatch_may_have_started);
    assert_eq!(released.authorization, p_grant);
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::Unbound
    );
}

#[test]
fn a_rejected_resend_leaves_a_binding_that_the_next_run_releases() {
    let mut harness = Harness::new();
    let a = bind(1, 'b');
    let a_request = request("initial", Some(FindingStatus::Open), false);
    crash_after_first_intent(&mut harness, &a_request, &a);
    harness.host.fault = Some(Fault::Rejected(Effect::CreateSummary));
    harness.publish(&a_request, Some(&a)).unwrap();
    assert!(harness.host.snapshot.summaries.is_empty());
    assert_eq!(harness.progress().unresolved, None);
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::BoundUnsent(&bound(&a, 1))
    );
    harness.host.fault = None;
    harness.reset_observations();

    let b = bind(2, 'c');
    let b_request = review_new_head(&mut harness);
    let completed = harness.publish(&b_request, Some(&b)).unwrap();

    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::Unbound
    );
    assert!(harness.progress().settled_runs.is_empty());
}

#[test]
fn release_keeps_retries_receipts_and_settlements() {
    let mut harness = Harness::new();
    let p = bind(1, 'b');
    let initial = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    harness.publish(&initial, Some(&p)).unwrap();
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    harness.publish(&initial, Some(&p)).unwrap();
    observe_pending_create(&mut harness.host);
    let recovered = harness.publish(&initial, Some(&p)).unwrap();
    harness.settle(&p, &recovered).unwrap().unwrap();
    let q = bind(2, 'c');
    let fixed = harness
        .publish(
            &request("fixed", Some(FindingStatus::Fixed), false),
            Some(&q),
        )
        .unwrap();
    let q_settled = harness.settle(&q, &fixed).unwrap().unwrap();
    let retained = harness.progress().clone();
    assert!(!retained.retried_creations.is_empty());
    assert!(!retained.verified_resolutions.is_empty());

    let recurrence = request("recurrence", Some(FindingStatus::Open), false);
    crash_after_first_intent(&mut harness, &recurrence, &bind(3, 'd'));
    assert_eq!(
        harness.progress().retried_creations,
        retained.retried_creations
    );
    harness.reset_observations();
    let completed = harness.publish(&recurrence, Some(&bind(4, 'e'))).unwrap();

    assert_eq!(completed.status, PublicationStatus::Completed);
    let released = &harness.journal.checkpoints[installation(&harness, 4) - 1];
    assert_eq!(released.coverage_binding, Some(bound(&q, q_settled.round)));
    assert!(released.dispatch_may_have_started);
    assert_eq!(released.unresolved, None);
    assert_eq!(released.authorization, retained.authorization);
    assert_eq!(released.retried_creations, retained.retried_creations);
    assert_eq!(released.verified_resolutions, retained.verified_resolutions);
    assert_eq!(released.settled_runs, retained.settled_runs);
}

#[test]
fn an_owner_with_a_possible_effect_is_never_released() {
    let a = bind(1, 'b');
    let intent = |phase| UnresolvedWrite {
        subject: subject(),
        reviewer: reviewer("agent"),
        write: Write::SetResolved {
            thread: provider_id("owned-root"),
            resolved: true,
        },
        renderer: None,
        phase,
        baseline: None,
    };
    let journals = [
        (false, Some(intent(WritePhase::DispatchPossible))),
        (false, Some(intent(WritePhase::LegacyAmbiguous))),
        (true, Some(intent(WritePhase::Prepared))),
        (true, None),
    ];
    for (dispatched, unresolved) in journals {
        let mut harness = Harness::new();
        harness.host.progress = PublicationProgress {
            coverage_binding: Some(bound(&a, 1)),
            dispatch_may_have_started: dispatched,
            unresolved,
            ..Default::default()
        };
        let before = harness.progress().clone();
        assert_eq!(before.unsent_binding(), None);
        assert_eq!(
            before.run_publication(&a.run_id),
            RunPublication::BoundUnsettled(&bound(&a, 1))
        );

        let b = bind(2, 'c');
        assert_eq!(
            binding_error(harness.publish(
                &request("initial", Some(FindingStatus::Open), false),
                Some(&b),
            )),
            Some(BindingError::PendingOwner(a.run_id.clone()))
        );
        assert_eq!(
            binding_error(harness.abandon(&abandonment_of_round_one(), Some(&b))),
            Some(BindingError::PendingOwner(a.run_id.clone()))
        );
        assert_eq!(harness.progress(), &before);
        assert!(harness.host.writes.is_empty());
        assert!(harness.journal.checkpoints.is_empty());
    }
}

#[test]
fn an_unfinalized_superseded_recovery_stays_unsettled_and_can_be_abandoned() {
    // F2: a superseded result whose provider round is still pending settles nothing.
    let mut harness = Harness::new();
    let a = bind(1, 'b');
    let a_request = request("initial", Some(FindingStatus::Open), false);
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    harness.publish(&a_request, Some(&a)).unwrap();
    harness.host.drift_after_finding = true;
    let superseded = harness.publish(&a_request, Some(&a)).unwrap();
    assert_eq!(superseded.status, PublicationStatus::Superseded);
    assert_eq!(
        superseded.final_snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );

    assert_eq!(harness.settle(&a, &superseded), Ok(None));
    assert!(harness.progress().settled_runs.is_empty());
    assert_eq!(
        harness.progress().run_publication(&a.run_id),
        RunPublication::BoundUnsettled(&bound(&a, 1))
    );

    let abandon = abandonment(&harness.host);
    let abandoned = harness.abandon(&abandon, Some(&a)).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    let settled = harness.settle(&a, &abandoned).unwrap().unwrap();
    assert_eq!(settled.outcome, SettledOutcome::Abandoned);
}

#[test]
fn an_unrelated_run_cannot_claim_an_already_abandoned_round() {
    // F4: abandoning a round that another run closed settles nothing.
    let mut harness = Harness::new();
    let p = bind(1, 'b');
    harness.host.authority = summary_effects();
    harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&p),
        )
        .unwrap();
    harness.host.authority = Effects::review();
    let abandon = abandonment(&harness.host);
    let abandoned = harness.abandon(&abandon, Some(&p)).unwrap();
    harness.settle(&p, &abandoned).unwrap().unwrap();
    let before = harness.progress().clone();
    harness.reset_observations();

    let b = bind(2, 'c');
    assert!(matches!(
        harness.abandon(&abandon, Some(&b)),
        Err(PlanError::NotAbandonable)
    ));
    assert_eq!(harness.settle(&b, &abandoned), Ok(None));

    assert!(harness.host.writes.is_empty());
    assert_eq!(harness.progress(), &before);
}

#[test]
fn the_admission_view_of_an_unsent_owner_equals_its_checkpointed_release() {
    let mut harness = Harness::new();
    let p = bind(1, 'b');
    let blocked = harness
        .publish(&request("blocked", None, true), Some(&p))
        .unwrap();
    let p_settled = harness.settle(&p, &blocked).unwrap().unwrap();
    let p_grant = harness.progress().authorization.clone();
    let a = bind(2, 'c');
    let mut a_request = request("ready", None, false);
    a_request.effects = summary_effects();
    crash_after_first_intent(&mut harness, &a_request, &a);
    harness.reset_observations();
    let journal = harness.progress().clone();
    let b = bind(3, 'd');

    let view = journal.released_for_admission(&b.run_id).unwrap();

    assert_eq!(&journal, harness.progress(), "the journal is unchanged");
    assert!(journal.unresolved.is_some());
    assert_eq!(view.unresolved, None);
    assert_eq!(view.authorization, p_grant);
    assert_eq!(view.coverage_binding, Some(bound(&p, 1)));
    assert!(view.dispatch_may_have_started);
    assert_eq!(view.settled_runs, vec![p_settled]);
    assert_eq!(view.retried_creations, journal.retried_creations);
    assert_eq!(view.verified_resolutions, journal.verified_resolutions);
    assert_eq!(
        journal.released_for_admission(&a.run_id),
        None,
        "an owner never releases its own binding"
    );

    let completed = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&b),
        )
        .unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    let released = &harness.journal.checkpoints[installation(&harness, 3) - 1];
    assert_eq!(
        released, &view,
        "admission releases exactly the planned view"
    );
}

#[test]
fn an_owner_with_a_possible_effect_has_no_admission_view() {
    let mut harness = Harness::new();
    let a = bind(1, 'b');
    harness.host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
    let uncertain = harness
        .publish(
            &request("initial", Some(FindingStatus::Open), false),
            Some(&a),
        )
        .unwrap();
    assert_eq!(uncertain.status, PublicationStatus::Uncertain);
    assert_eq!(
        harness
            .progress()
            .released_for_admission(&bind(2, 'c').run_id),
        None
    );
    assert_eq!(
        PublicationProgress::default().released_for_admission(&bind(2, 'c').run_id),
        None,
        "nothing to release without a binding"
    );
}
