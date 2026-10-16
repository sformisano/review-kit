use crate::observation as log;
use review_kit_domain::{ReviewerId, Subject};

use super::{execution::verified_write, planning::PlanError};
use crate::publication::{Snapshot, Write};

/// Caller-owned progress must accompany every publication and abandonment call.
/// Default means the caller has no earlier unresolved attempt. Provider listings
/// cannot establish that fact. Durable callers persist intent before dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicationProgress {
    pub unresolved: Option<UnresolvedWrite>,
    pub verified_resolutions: Vec<crate::publication::ResolutionReceipt>,
    pub authorization: Option<RoundAuthorization>,
    pub retried_creations: Vec<Write>,
    pub recovery_policy: RecoveryPolicy,
    /// The review run that owns the current publication; `None` before any run publishes.
    pub coverage_binding: Option<super::RunBinding>,
    /// Verified final outcomes, unique by run ID, retained when the binding advances.
    pub settled_runs: Vec<super::SettledRun>,
    /// A write of the current binding could have reached the provider.
    /// It stays true across later prepared writes. It resets when a new binding
    /// is installed or a definite rejection is rolled back; a binding restored
    /// from a settlement by `release_unsent` sets it.
    pub dispatch_may_have_started: bool,
}

/// Exact retained write intent whose outcome has not been settled for this caller.
/// The complete baseline protects owned objects and retained reply targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedWrite {
    pub subject: Subject,
    pub reviewer: ReviewerId,
    pub write: Write,
    pub renderer: Option<crate::ports::RenderProfileIdentity>,
    pub phase: WritePhase,
    pub baseline: Option<Snapshot>,
}

#[review_kit_observation::operations]
impl UnresolvedWrite {
    /// An explicit close authorizes its exact terminal summary independently.
    #[helper]
    pub fn abandonment_round(&self) -> Option<u64> {
        match &self.write {
            Write::Summary {
                expected: Some(expected),
                round,
                history,
                ..
            } if round.status == crate::publication::RoundStatus::Abandoned
                && expected.current.number == round.number
                && expected.current.assessment == round.assessment
                && expected.current.actions == round.actions
                && expected.current.answers == round.answers
                && expected.current.transitions == round.transitions
                && expected.current.answer_targets == round.answer_targets
                && expected.current.review_workflow == round.review_workflow
                && expected.current.policy_set == round.policy_set
                && expected.current.policy_names == round.policy_names
                && expected.current.policy_declarations == round.policy_declarations
                && expected.current.renderer == round.renderer
                && expected.current.publication_limitation == round.publication_limitation
                && expected.current.confirmed_validation == round.confirmed_validation
                && expected.history == *history
                && matches!(
                    expected.current.status,
                    crate::publication::RoundStatus::Pending
                        | crate::publication::RoundStatus::Partial
                )
                && round.abandonment.as_ref().is_some_and(|closed| {
                    !closed.reason.trim().is_empty()
                        && !crate::ports::contains_reserved_marker(&closed.reason)
                }) =>
            {
                Some(round.number)
            }
            _ => None,
        }
    }

    /// Complete retained intent belongs to the authorized round and its baseline.
    #[helper]
    pub fn is_bound_to_round(&self, authorization: &RoundAuthorization) -> bool {
        let Some(baseline) = &self.baseline else {
            return false;
        };
        if baseline.subject != self.subject || baseline.reviewer != self.reviewer {
            return false;
        }
        match &self.write {
            Write::Summary {
                expected, round, ..
            } => {
                round.number == authorization.round
                    && self.renderer == round.renderer
                    && (round.status != crate::publication::RoundStatus::Abandoned
                        || self.abandonment_round().is_some())
                    && round.assessment.subject() == &self.subject
                    && round.assessment.reviewer() == &self.reviewer
                    && expected
                        .as_ref()
                        .is_none_or(|summary| baseline.summaries.contains(summary))
            }
            _ => baseline
                .summaries
                .iter()
                .find(|s| s.author == self.reviewer)
                .is_some_and(|summary| {
                    summary.current.number == authorization.round
                        && summary.current.actions.iter().any(|action| {
                            super::action_write::action_write(
                                action,
                                authorization.round,
                                summary.current.assessment.revision(),
                                baseline,
                                &self.reviewer,
                            )
                            .is_ok_and(|write| write == self.write)
                        })
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WritePhase {
    Prepared,
    DispatchPossible,
    #[default]
    LegacyAmbiguous,
}

/// Bounded retry permits one extra creation after complete reconciliation reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecoveryPolicy {
    #[default]
    BoundedRetry,
    Conservative,
}

/// Trusted effects granted when this round first starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundAuthorization {
    pub round: u64,
    pub effects: crate::publication::Effects,
}

#[review_kit_observation::operations]
impl PublicationProgress {
    #[helper]
    pub(super) fn retries_in_round(&self, round: u64) -> Vec<Write> {
        self.retried_creations
            .iter()
            .filter(|write| match write {
                Write::Summary {
                    round: retained, ..
                } => retained.number == round,
                Write::Finding {
                    round: retained, ..
                } => *retained == round,
                Write::Reply { action, .. } => action.round == round,
                Write::SetResolved { .. } => false,
            })
            .cloned()
            .collect()
    }
    #[helper]
    pub(super) fn reconcile_recorded(
        &mut self,
        snapshot: &Snapshot,
        reviewer: &ReviewerId,
        evidence: &mut crate::use_cases::publication_run::ManagedEvidence<'_>,
    ) -> Result<bool, PlanError> {
        let previous = self.clone();
        let settled = self.reconcile(snapshot, reviewer)?;
        if *self != previous
            && let Err(error) = evidence.record(
                crate::use_cases::publication_run::ExecutionEvent::Progress {
                    subject: &snapshot.subject,
                    reviewer,
                    progress: self,
                },
            )
        {
            *self = previous;
            return Err(PlanError::Evidence(error));
        }
        Ok(settled)
    }
    #[operation(name = "application.publication.reconcile_progress", input = log::Fields::new().flag("unresolved", self.unresolved.is_some()), output = |result| match result {
                Ok(true) => log::Completion::success(log::Fields::new().flag("settled", true)),
                Ok(false) => log::Completion::new(
                    log::Outcome::Uncertain,
                    log::Fields::new().flag("settled", false),
                ),
                Err(error) => log::Completion::failure(log::plan_error(error)),
            })]
    pub(super) fn reconcile(
        &mut self,
        snapshot: &Snapshot,
        reviewer: &ReviewerId,
    ) -> Result<bool, PlanError> {
        let Some(pending) = &self.unresolved else {
            return Ok(true);
        };
        if pending.subject != snapshot.subject || &pending.reviewer != reviewer {
            return Err(PlanError::ProgressBinding);
        }
        if verified_write(&pending.write, snapshot, reviewer)
            && matches_retained_target(pending, snapshot)
        {
            if let Write::SetResolved { thread, resolved } = &pending.write
                && let Some(round) = pending.baseline.as_ref().and_then(|baseline| {
                    baseline
                        .summaries
                        .iter()
                        .find(|s| &s.author == reviewer)
                        .map(|s| s.current.number)
                })
            {
                let receipt = crate::publication::ResolutionReceipt {
                    subject: snapshot.subject.clone(),
                    reviewer: reviewer.clone(),
                    round,
                    thread: thread.clone(),
                    resolved: *resolved,
                    provenance: crate::publication::ResolutionProvenance::Observed,
                };
                if !self.verified_resolutions.contains(&receipt) {
                    self.verified_resolutions.push(receipt);
                }
            }
            self.unresolved = None;
            return Ok(true);
        }
        Ok(false)
    }

    /// Drop a retained `Prepared` write that can no longer resume.
    ///
    /// The dispatch checkpoint persists `DispatchPossible` before any provider
    /// call, so a `Prepared` write never reached the provider. The discard is
    /// recorded, then checkpointed; every other field, including the dispatch
    /// flag, stays unchanged. Returns `false` without a change when no write is
    /// unresolved or it may have been dispatched.
    #[helper]
    pub(super) fn discard_unsent(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
        evidence: &mut crate::use_cases::publication_run::ManagedEvidence<'_>,
    ) -> Result<bool, PlanError> {
        let Some(pending) = self
            .unresolved
            .as_ref()
            .filter(|pending| pending.phase == WritePhase::Prepared)
        else {
            return Ok(false);
        };
        evidence
            .record(
                crate::use_cases::publication_run::ExecutionEvent::DiscardedUnsent(&pending.write),
            )
            .map_err(PlanError::Evidence)?;
        let previous = self.clone();
        self.unresolved = None;
        if let Err(error) = evidence.record(
            crate::use_cases::publication_run::ExecutionEvent::Progress {
                subject,
                reviewer,
                progress: self,
            },
        ) {
            *self = previous;
            return Err(PlanError::Evidence(error));
        }
        Ok(true)
    }

    #[helper]
    pub(super) fn before_write(
        &mut self,
        snapshot: &Snapshot,
        reviewer: &ReviewerId,
        write: &Write,
        renderer: Option<&crate::ports::RenderProfileIdentity>,
    ) {
        self.unresolved = Some(UnresolvedWrite {
            subject: snapshot.subject.clone(),
            reviewer: reviewer.clone(),
            write: write.clone(),
            renderer: renderer.cloned(),
            phase: WritePhase::Prepared,
            baseline: Some(snapshot.clone()),
        });
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn check_unresolved_profile(
    progress: &PublicationProgress,
    renderer: &crate::ports::ManagedRenderer<'_>,
) -> Result<(), PlanError> {
    if let Some(unresolved) = &progress.unresolved {
        match (&unresolved.renderer, renderer.identity()) {
            (Some(retained), Some(selected)) if retained != selected => {
                return Err(PlanError::ChangedRenderProfile);
            }
            (None, Some(_)) | (Some(_), None) => {
                return Err(PlanError::UnknownPendingRenderProfile);
            }
            _ => {}
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn is_create(write: &Write) -> bool {
    matches!(
        write,
        Write::Summary { expected: None, .. } | Write::Finding { .. } | Write::Reply { .. }
    )
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn matches_retained_target(pending: &UnresolvedWrite, snapshot: &Snapshot) -> bool {
    let Some(before) = &pending.baseline else {
        return true;
    };
    match &pending.write {
        Write::SetResolved { thread, resolved } => {
            let expected = before
                .findings
                .iter()
                .find(|root| root.author == pending.reviewer && &root.id == thread)
                .map(|root| {
                    let mut root = root.clone();
                    root.resolved = *resolved;
                    root
                });
            expected.is_some_and(|root| snapshot.findings.contains(&root))
                && before
                    .summaries
                    .iter()
                    .find(|s| s.author == pending.reviewer)
                    == snapshot
                        .summaries
                        .iter()
                        .find(|s| s.author == pending.reviewer)
        }
        Write::Reply {
            action:
                crate::publication::ActionId {
                    kind: crate::publication::ReplyKind::Answer(note),
                    ..
                },
            ..
        } => before
            .replies
            .iter()
            .find(|reply| &reply.id == note)
            .is_some_and(|reply| snapshot.replies.contains(reply)),
        _ => true,
    }
}
