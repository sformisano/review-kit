//! Pure settlement records, outcome mapping and journal queries.

use std::time::{Duration, SystemTime};

use super::coverage_binding::{bind, content, files, run_id};
use super::*;
use review_kit_application::{
    review_run::SettledOutcome,
    use_cases::publishing::{
        BindingError, BindingRequest, ManagedHistory, OwnedHistory, RunBinding, RunPublication,
        SettledRun, UnresolvedWrite, VerifiedEffect, WritePhase, settled_outcome,
    },
};

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn owner(request: &BindingRequest, round: u64) -> RunBinding {
    RunBinding {
        run_id: request.run_id.clone(),
        round,
        review_assessment: request.review_assessment.clone(),
        snapshot_files: request.snapshot_files.clone(),
        prior_authorization: None,
    }
}

fn entry(request: &BindingRequest, outcome: SettledOutcome, reason: Option<&str>) -> SettledRun {
    SettledRun {
        run_id: request.run_id.clone(),
        review_assessment: request.review_assessment.clone(),
        snapshot_files: request.snapshot_files.clone(),
        round: 1,
        outcome,
        reason: reason.map(str::to_owned),
        verified_at: at(10),
    }
}

fn bound_progress(request: &BindingRequest) -> PublicationProgress {
    PublicationProgress {
        coverage_binding: Some(owner(request, 1)),
        dispatch_may_have_started: true,
        ..Default::default()
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::review(),
    }
}

/// The provider after the fixture review completed round 1.
fn completed() -> Snapshot {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot
}

/// The completed provider state with the reviewer's round 1 in `status`.
fn observed(status: RoundStatus) -> Snapshot {
    let mut snapshot = completed();
    snapshot.summaries[0].current.status = status;
    snapshot
}

/// A result whose final snapshot shows round 1 completed.
fn result(status: PublicationStatus) -> PublicationResult {
    PublicationResult {
        final_snapshot: completed(),
        status,
        decision: ReviewDecision::Ready,
        round: 1,
        planned: vec![],
        attempted: vec![],
        verified: vec![],
        retried_creations: vec![],
        skipped: vec![],
        stop: None,
        finalization_stop: None,
    }
}

fn pending_write() -> UnresolvedWrite {
    UnresolvedWrite {
        subject: subject(),
        reviewer: reviewer("agent"),
        write: Write::SetResolved {
            thread: provider_id("owned-root"),
            resolved: true,
        },
        renderer: None,
        phase: WritePhase::DispatchPossible,
        baseline: None,
    }
}

#[test]
fn publication_status_maps_to_the_settled_outcome() {
    use PublicationStatus::*;
    for dispatched in [false, true] {
        for status in [Completed, Blocked, Superseded] {
            assert_eq!(
                settled_outcome(status, dispatched),
                Some(SettledOutcome::Published)
            );
        }
        assert_eq!(
            settled_outcome(Abandoned, dispatched),
            Some(SettledOutcome::Abandoned)
        );
        for status in [Partial, Uncertain, Denied, NotStarted] {
            assert_eq!(settled_outcome(status, dispatched), None, "{status:?}");
        }
    }
    assert_eq!(
        settled_outcome(Unchanged, false),
        Some(SettledOutcome::NoChanges)
    );
    assert_eq!(
        settled_outcome(Unchanged, true),
        Some(SettledOutcome::Published)
    );
}

#[test]
fn settlements_are_unique_by_run_and_only_describe_the_current_binding() {
    let first = bind(1, 'b');
    let mut progress = bound_progress(&first);
    let published = entry(&first, SettledOutcome::Published, None);

    assert_eq!(progress.record_settlement(published.clone()), Ok(true));
    let retried = SettledRun {
        verified_at: at(20),
        ..published.clone()
    };
    assert_eq!(progress.record_settlement(retried), Ok(false));
    for conflicting in [
        entry(&first, SettledOutcome::NoChanges, None),
        entry(&first, SettledOutcome::Published, Some("Different reason")),
        SettledRun {
            review_assessment: content('c'),
            ..published.clone()
        },
    ] {
        assert_eq!(
            progress.record_settlement(conflicting),
            Err(BindingError::SettlementConflict(first.run_id.clone()))
        );
    }
    assert_eq!(progress.settled_runs, vec![published]);

    let other = bind(2, 'c');
    assert_eq!(
        progress.record_settlement(entry(&other, SettledOutcome::Published, None)),
        Err(BindingError::Mismatch(other.run_id.clone()))
    );
    let mut other_round = bound_progress(&other);
    assert_eq!(
        other_round.record_settlement(SettledRun {
            round: 2,
            ..entry(&other, SettledOutcome::Published, None)
        }),
        Err(BindingError::Mismatch(other.run_id.clone()))
    );
    for reason in [None, Some(" ")] {
        assert_eq!(
            other_round.record_settlement(entry(&other, SettledOutcome::Abandoned, reason)),
            Err(BindingError::MissingReason)
        );
    }
    other_round.unresolved = Some(pending_write());
    assert_eq!(
        other_round.record_settlement(entry(&other, SettledOutcome::Published, None)),
        Err(BindingError::Unresolved)
    );
    assert!(other_round.settled_runs.is_empty());
}

#[test]
fn run_publication_reports_settled_bound_and_unbound_runs() {
    let first = bind(1, 'b');
    let second = bind(2, 'c');
    let mut progress = bound_progress(&second);
    let earlier = entry(
        &first,
        SettledOutcome::Abandoned,
        Some("Superseded by a new run."),
    );
    progress.settled_runs.push(earlier.clone());

    assert_eq!(
        progress.run_publication(&first.run_id),
        RunPublication::Settled(&earlier)
    );
    assert_eq!(
        progress.run_publication(&second.run_id),
        RunPublication::BoundUnsettled(&owner(&second, 1))
    );
    assert_eq!(
        progress.run_publication(&run_id(3)),
        RunPublication::Unbound
    );
    progress.dispatch_may_have_started = false;
    assert_eq!(
        progress.run_publication(&second.run_id),
        RunPublication::BoundUnsent(&owner(&second, 1))
    );
    assert_eq!(
        PublicationProgress::default().run_publication(&first.run_id),
        RunPublication::Unbound
    );
}

#[test]
fn unfinished_results_settle_nothing() {
    let first = bind(1, 'b');
    let mut superseded_without_summary = result(PublicationStatus::Superseded);
    superseded_without_summary.verified.push(VerifiedEffect {
        write: Write::SetResolved {
            thread: provider_id("owned-root"),
            resolved: true,
        },
        readback: snapshot(),
    });
    superseded_without_summary.finalization_stop = Some(Stop::ConcurrentChange);
    for unfinished in [
        result(PublicationStatus::Partial),
        result(PublicationStatus::Uncertain),
        result(PublicationStatus::Denied),
        result(PublicationStatus::NotStarted),
        result(PublicationStatus::Superseded),
        superseded_without_summary,
    ] {
        let mut progress = bound_progress(&first);
        let before = progress.clone();
        assert_eq!(
            progress.settle_publication(&first, &unfinished, at(30)),
            Ok(None)
        );
        assert_eq!(progress, before);
    }
}

#[test]
fn settlement_refuses_mismatched_inputs_pending_owners_and_missing_abandonment_reasons() {
    let first = bind(1, 'b');
    let second = bind(2, 'c');
    let completed = result(PublicationStatus::Completed);

    let mut progress = bound_progress(&first);
    let changed = BindingRequest {
        snapshot_files: files('d'),
        ..first.clone()
    };
    assert_eq!(
        progress.settle_publication(&changed, &completed, at(30)),
        Err(BindingError::Mismatch(first.run_id.clone()))
    );
    assert_eq!(
        progress.settle_publication(&second, &result(PublicationStatus::Unchanged), at(30)),
        Err(BindingError::PendingOwner(first.run_id.clone()))
    );
    assert_eq!(
        progress.settle_publication(&first, &result(PublicationStatus::Abandoned), at(30)),
        Err(BindingError::MissingReason)
    );
    assert_eq!(progress, bound_progress(&first));

    progress.unresolved = Some(pending_write());
    assert_eq!(
        progress.settle_publication(&first, &completed, at(30)),
        Err(BindingError::Unresolved)
    );
}

#[test]
fn managed_history_counts_only_reviewer_owned_provider_state() {
    let (host, mut source, mut renderer) = fixtures();
    let foreign = host.snapshot.clone();
    assert!(
        foreign
            .findings
            .iter()
            .all(|root| root.author != foreign.reviewer)
    );
    let history = ManagedHistory::observe(&foreign);
    assert!(history.is_absent());
    assert_eq!(history.subject(), &subject());
    assert_eq!(history.reviewer(), &reviewer("agent"));

    let mut host = host;
    seed_completed(&mut host, &mut source, &mut renderer);
    assert!(!ManagedHistory::observe(&host.snapshot).is_absent());
    let mut roots_only = host.snapshot.clone();
    roots_only.summaries.clear();
    assert!(!ManagedHistory::observe(&roots_only).is_absent());
    let mut summary_only = host.snapshot.clone();
    summary_only
        .findings
        .retain(|root| root.author != summary_only.reviewer);
    assert!(!ManagedHistory::observe(&summary_only).is_absent());
}

#[test]
fn an_empty_journal_is_admitted_only_without_history_or_after_a_finished_round() {
    let state = |snapshot: &Snapshot| {
        let history = ManagedHistory::observe(snapshot);
        (history.state(), history.admits_empty_journal())
    };
    assert_eq!(state(&snapshot()), (OwnedHistory::Absent, true));
    for status in [
        RoundStatus::Completed,
        RoundStatus::Blocked,
        RoundStatus::Superseded,
    ] {
        assert_eq!(
            state(&observed(status)),
            (OwnedHistory::Finished(1), true),
            "{status:?}"
        );
    }
    let mut abandoned = observed(RoundStatus::Abandoned);
    abandoned.summaries[0].current.abandonment = Some(Abandonment {
        reason: "Replaced by a corrected review.".into(),
        verified_actions: vec![],
    });
    assert_eq!(state(&abandoned), (OwnedHistory::Finished(1), true));

    for status in [RoundStatus::Pending, RoundStatus::Partial] {
        assert_eq!(
            state(&observed(status)),
            (OwnedHistory::Unfinished(1), false),
            "{status:?}"
        );
    }
    abandoned.summaries[0].current.abandonment = Some(Abandonment {
        reason: " ".into(),
        verified_actions: vec![],
    });
    assert_eq!(state(&abandoned), (OwnedHistory::Ambiguous, false));
    let mut roots_only = completed();
    roots_only.summaries.clear();
    assert_eq!(state(&roots_only), (OwnedHistory::Ambiguous, false));
    let mut two_summaries = completed();
    let mut second = two_summaries.summaries[0].clone();
    second.id = provider_id("second-summary");
    two_summaries.summaries.push(second);
    assert_eq!(state(&two_summaries), (OwnedHistory::Ambiguous, false));
}

#[test]
fn settlement_requires_the_reviewer_round_observed_terminal() {
    // F2: a result settles only when the provider shows its round finished.
    let first = bind(1, 'b');
    let superseded = || {
        let mut value = result(PublicationStatus::Superseded);
        value.verified.push(VerifiedEffect {
            write: Write::SetResolved {
                thread: provider_id("owned-root"),
                resolved: true,
            },
            readback: snapshot(),
        });
        value
    };
    let statuses = [
        result(PublicationStatus::Completed),
        result(PublicationStatus::Blocked),
        result(PublicationStatus::Unchanged),
        superseded(),
    ];
    let unfinished = [
        observed(RoundStatus::Pending),
        observed(RoundStatus::Partial),
        snapshot(),
    ];
    for template in &statuses {
        for final_snapshot in unfinished.iter().cloned() {
            let mut unfinished_result = PublicationResult {
                final_snapshot,
                status: template.status,
                decision: template.decision,
                round: template.round,
                planned: vec![],
                attempted: vec![],
                verified: vec![],
                retried_creations: vec![],
                skipped: vec![],
                stop: None,
                finalization_stop: None,
            };
            if template.status == PublicationStatus::Superseded {
                unfinished_result.verified = superseded().verified;
            }
            for mut progress in [bound_progress(&first), PublicationProgress::default()] {
                let before = progress.clone();
                assert_eq!(
                    progress.settle_publication(&first, &unfinished_result, at(30)),
                    Ok(None),
                    "{:?}",
                    template.status
                );
                assert_eq!(progress, before);
            }
        }
        let mut other_round = PublicationResult {
            final_snapshot: completed(),
            status: template.status,
            decision: template.decision,
            round: 2,
            planned: vec![],
            attempted: vec![],
            verified: superseded().verified,
            retried_creations: vec![],
            skipped: vec![],
            stop: None,
            finalization_stop: None,
        };
        let mut progress = bound_progress(&first);
        assert_eq!(
            progress.settle_publication(&first, &other_round, at(30)),
            Ok(None)
        );
        other_round.round = 1;
        let mut progress = PublicationProgress {
            coverage_binding: Some(owner(&first, 1)),
            dispatch_may_have_started: true,
            ..Default::default()
        };
        assert_eq!(
            progress
                .settle_publication(&first, &other_round, at(30))
                .unwrap()
                .map(|entry| entry.outcome),
            Some(SettledOutcome::Published),
            "{:?}",
            template.status
        );
    }
    for terminal in [RoundStatus::Blocked, RoundStatus::Superseded] {
        let mut progress = bound_progress(&first);
        let mut value = result(PublicationStatus::Completed);
        value.final_snapshot = observed(terminal);
        assert!(
            progress
                .settle_publication(&first, &value, at(30))
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn abandonment_settles_only_the_run_that_owns_the_binding() {
    // F4: an abandoned round observed by another run is not that run's outcome.
    let first = bind(1, 'b');
    let second = bind(2, 'c');
    let mut abandoned = result(PublicationStatus::Abandoned);
    let current = &mut abandoned.final_snapshot.summaries[0].current;
    current.status = RoundStatus::Abandoned;
    current.abandonment = Some(Abandonment {
        reason: "Replaced by a corrected review.".into(),
        verified_actions: vec![],
    });

    for mut progress in [
        PublicationProgress::default(),
        PublicationProgress {
            settled_runs: vec![entry(&first, SettledOutcome::Published, None)],
            ..bound_progress(&first)
        },
    ] {
        let before = progress.clone();
        assert_eq!(
            progress.settle_publication(&second, &abandoned, at(30)),
            Ok(None)
        );
        assert_eq!(progress, before);
    }

    let mut progress = bound_progress(&second);
    let settled = progress
        .settle_publication(&second, &abandoned, at(30))
        .unwrap()
        .unwrap();
    assert_eq!(settled.outcome, SettledOutcome::Abandoned);
    assert_eq!(
        settled.reason.as_deref(),
        Some("Replaced by a corrected review.")
    );
}

#[test]
fn a_no_change_outcome_releases_an_unsent_owner_before_installing_its_binding() {
    let first = bind(1, 'b');
    let second = bind(2, 'c');
    let mut progress = PublicationProgress {
        dispatch_may_have_started: false,
        ..bound_progress(&first)
    };

    let settled = progress
        .settle_publication(&second, &result(PublicationStatus::Unchanged), at(30))
        .unwrap()
        .unwrap();

    assert_eq!(settled.outcome, SettledOutcome::NoChanges);
    assert_eq!(progress.coverage_binding, Some(owner(&second, 1)));
    assert!(!progress.dispatch_may_have_started);
    assert_eq!(progress.settled_runs, vec![settled]);
    assert_eq!(
        progress.run_publication(&first.run_id),
        RunPublication::Unbound
    );
}
