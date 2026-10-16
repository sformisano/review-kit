use super::{
    execution::{PublicationResult, PublicationStatus, same_owned_state},
    planning::PublishRequest,
    progress::{PublicationProgress, RecoveryPolicy, UnresolvedWrite, WritePhase, is_create},
};
use crate::publication::{Effects, Snapshot, Write};
use review_kit_domain::{ReviewDecision, Revision};

/// The permanent reason recovery can never resend a possibly dispatched creation.
///
/// Recovery resends an absent creation at most once and only on the revision it
/// was prepared for. Either condition stays true for the rest of the round, so
/// the write remains uncertain until the provider shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResendBlocked {
    /// The creation's one bounded resend was already used.
    RetryConsumed,
    /// The creation was prepared for another reviewed revision.
    RevisionChanged { reviewed: Revision },
}

#[review_kit_observation::operations]
impl PublicationProgress {
    /// The journal's possibly dispatched creation and why recovery can never resend
    /// it at `current`, or `None` when no such write exists or a resend may still
    /// be eligible.
    #[helper]
    pub fn creation_resend_blocked(
        &self,
        current: &Revision,
    ) -> Option<(&UnresolvedWrite, ResendBlocked)> {
        let pending = self.unresolved.as_ref()?;
        resend_blocked(pending, self, current).map(|blocked| (pending, blocked))
    }
}

#[review_kit_observation::operations]
#[helper]
fn resend_blocked(
    pending: &UnresolvedWrite,
    progress: &PublicationProgress,
    current: &Revision,
) -> Option<ResendBlocked> {
    if pending.phase != WritePhase::DispatchPossible || !is_create(&pending.write) {
        return None;
    }
    if progress.retried_creations.contains(&pending.write) {
        return Some(ResendBlocked::RetryConsumed);
    }
    let reviewed = &pending.baseline.as_ref()?.revision;
    (reviewed != current).then(|| ResendBlocked::RevisionChanged {
        reviewed: reviewed.clone(),
    })
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn eligible(
    pending: &UnresolvedWrite,
    progress: &PublicationProgress,
    request: &PublishRequest,
    snapshot: &Snapshot,
    authority: &Effects,
) -> bool {
    let Some(baseline) = &pending.baseline else {
        return false;
    };
    let Some(grant) = &progress.authorization else {
        return false;
    };
    let round = match &pending.write {
        Write::Summary { round, .. } => round.as_ref(),
        _ => match baseline
            .summaries
            .iter()
            .find(|s| s.author == pending.reviewer)
        {
            Some(summary) => &summary.current,
            None => return false,
        },
    };
    pending.abandonment_round().is_none()
        && pending.is_bound_to_round(grant)
        && pending.subject == *request.assessment.subject()
        && pending.reviewer == *request.assessment.reviewer()
        && round.assessment == request.assessment
        && round.policy_declarations == request.policy_declarations
        && round.answers == request.answers
        && round.transitions == request.transitions
        && round.review_workflow == request.review_workflow
        && round.confirmed_validation == request.confirmed_validation
        && round.policy_set.as_deref() == Some(request.policies.name())
        && grant.round == round.number
        && snapshot.revision == *request.assessment.revision()
        && baseline.revision == snapshot.revision
        && same_owned_state(baseline, snapshot, &pending.reviewer)
        && grant.effects.contains(pending.write.effect())
        && request.effects.contains(pending.write.effect())
        && authority.contains(pending.write.effect())
        && snapshot.permissions.contains(pending.write.effect())
        && (pending.phase == WritePhase::Prepared
            || (pending.phase == WritePhase::DispatchPossible
                && progress.recovery_policy == RecoveryPolicy::BoundedRetry
                && is_create(&pending.write)
                && resend_blocked(pending, progress, &snapshot.revision).is_none()))
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn pending_report(
    pending: &UnresolvedWrite,
    snapshot: &Snapshot,
    decision: ReviewDecision,
    progress: &PublicationProgress,
) -> PublicationResult {
    let round = match &pending.write {
        Write::Summary { round, .. } => round.number,
        Write::Finding { round, .. } => *round,
        Write::Reply { action, .. } => action.round,
        Write::SetResolved { .. } => snapshot
            .summaries
            .iter()
            .find(|s| s.author == pending.reviewer)
            .map_or(0, |s| s.current.number),
    };
    PublicationResult {
        final_snapshot: snapshot.clone(),
        status: PublicationStatus::Uncertain,
        decision,
        round,
        planned: Vec::new(),
        attempted: Vec::new(),
        verified: Vec::new(),
        retried_creations: progress.retries_in_round(round),
        skipped: Vec::new(),
        stop: None,
        finalization_stop: None,
    }
}
