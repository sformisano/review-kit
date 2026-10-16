use crate::observation as log;
use review_kit_domain::{ReviewerId, Subject};

use super::{
    action_write::action_write,
    binding::{Admission, BindingRequest, BoundInvocation},
    execution::{
        Execution, PublicationResult, PublicationStatus, Stop, status_for, verified_write,
    },
    planning::{PlanError, check_pending_profile, nonempty},
    progress::PublicationProgress,
    reconciliation::{
        finished_communication, observed_remaining_actions, validate_completed_actions,
        validate_historical_roots, validate_retained_structure,
    },
};
use crate::{
    ports::{PublicationHost, ReviewRenderer},
    publication::*,
    use_cases::{
        discovery::{DiscoveryError, discover_managed},
        publication_run::{Clock, ExecutionEvent, ExecutionEvidence, NoEvidence},
    },
};

/// The exact unfinished round to close and its public abandonment reason.
/// Requested effects must permit summary updates; existing findings remain published.
#[derive(Debug, Clone)]
pub struct AbandonRequest {
    pub subject: Subject,
    pub reviewer: ReviewerId,
    pub round: u64,
    pub reason: String,
    pub effects: Effects,
}

/// Close an exact pending or partial round by updating only its owned summary.
///
/// The caller supplies a nonblank public reason and retained publication progress.
/// The update refuses ambiguous intent and can replay its exact prepared close.
/// Findings and replies
/// remain unchanged and their IDs stay reserved. Validation failures return `PlanError`;
/// execution stops are retained in the result. The caller serializes publication.
#[review_kit_observation::operations]
#[helper]
pub fn abandon(
    request: &AbandonRequest,
    host: &mut dyn PublicationHost,
    renderer: &mut dyn ReviewRenderer,
    progress: &mut PublicationProgress,
) -> Result<PublicationResult, PlanError> {
    abandon_recorded(request, host, renderer, progress, &mut NoEvidence)
}

#[review_kit_observation::operations]
#[helper]
/// Abandon an exact unfinished round with evidence recording around the summary update.
///
/// Retained intent and immutable effects must match provider observations.
/// Abandonment refuses ambiguous intent and can replay its exact prepared close.
/// The update needs
/// `UpdateSummary` authority and preserves published content and reserved IDs.
/// A recording failure before dispatch prevents the update. The caller retains
/// progress across calls and serializes publication for this subject and reviewer.
pub fn abandon_recorded(
    request: &AbandonRequest,
    host: &mut dyn PublicationHost,
    renderer: &mut dyn ReviewRenderer,
    progress: &mut PublicationProgress,
    evidence: &mut dyn ExecutionEvidence,
) -> Result<PublicationResult, PlanError> {
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    let mut host = crate::ports::ManagedHost::new(host);
    let mut evidence = crate::use_cases::publication_run::ManagedEvidence::new(evidence);
    abandon_recorded_managed(
        request,
        &mut host,
        &mut renderer,
        progress,
        &mut evidence,
        None,
    )
}

#[review_kit_observation::operations]
#[helper]
/// Abandon the round bound to a review run and record evidence around the update.
///
/// The journal must bind `binding`'s run to the same accepted inputs and round,
/// or permit installing that binding with the abandonment summary. Another run's
/// binding without a possible provider effect is released first; another
/// unsettled binding refuses abandonment unless the provider shows its bound
/// round terminal. `clock` timestamps such a provider-verified settlement. A run
/// that installs no binding cannot claim an already abandoned round:
/// `NotAbandonable`.
/// The caller then records the run's settlement with
/// `PublicationProgress::settle_publication`. `None` behaves as `abandon_recorded`.
pub fn abandon_bound(
    request: &AbandonRequest,
    binding: Option<&BindingRequest>,
    host: &mut dyn PublicationHost,
    renderer: &mut dyn ReviewRenderer,
    progress: &mut PublicationProgress,
    evidence: &mut dyn ExecutionEvidence,
    clock: &mut dyn Clock,
) -> Result<PublicationResult, PlanError> {
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    let mut host = crate::ports::ManagedHost::new(host);
    let mut evidence = crate::use_cases::publication_run::ManagedEvidence::new(evidence);
    let mut bound = binding.map(|request| BoundInvocation { request, clock });
    abandon_recorded_managed(
        request,
        &mut host,
        &mut renderer,
        progress,
        &mut evidence,
        bound.as_mut(),
    )
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.abandon", input = log::subject(&request.subject)
            .number("round", request.round)
            .secret("reason", request.reason.as_str()), output = log::publication_result)]
pub(crate) fn abandon_recorded_managed(
    request: &AbandonRequest,
    host: &mut crate::ports::ManagedHost<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    progress: &mut PublicationProgress,
    evidence: &mut crate::use_cases::publication_run::ManagedEvidence<'_>,
    mut binding: Option<&mut BoundInvocation<'_>>,
) -> Result<PublicationResult, PlanError> {
    let admission = progress.admit(binding.as_deref().map(|bound| bound.request))?;
    if !matches!(admission, Admission::ReleaseOwner(_)) {
        super::progress::check_unresolved_profile(progress, renderer)?;
    }
    if request.reason.trim().is_empty() || crate::ports::contains_reserved_marker(&request.reason) {
        return Err(PlanError::InvalidAbandonmentReason);
    }
    let mut session = host
        .acquire(&request.subject, &request.reviewer)
        .map_err(|error| PlanError::Discovery(DiscoveryError::Read(error)))?;
    if session.authenticated_reviewer() != &request.reviewer {
        return Err(PlanError::WrongReviewer);
    }
    let authority = session
        .authority()
        .map_err(|error| PlanError::Discovery(DiscoveryError::Read(error)))?;
    let snapshot = discover_managed(&mut session.reader(), &request.subject, &request.reviewer)
        .map_err(super::planning::discovery_error)?;
    let owner = super::binding::owner_settlement(
        &admission,
        binding.as_deref_mut(),
        &snapshot,
        &request.reviewer,
    )?;
    check_pending_profile(&snapshot, &request.reviewer, renderer)?;
    evidence
        .record(ExecutionEvent::Observed(&snapshot))
        .map_err(PlanError::Evidence)?;
    let admission = progress.settle_owner(
        admission,
        owner,
        &request.subject,
        &request.reviewer,
        evidence,
    )?;
    let install =
        super::binding::installation(&admission, binding.as_deref(), progress, request.round)?;
    let summary = snapshot
        .summaries
        .iter()
        .find(|summary| summary.author == request.reviewer)
        .cloned()
        .ok_or(PlanError::NotAbandonable)?;
    // A run that neither resumed its binding nor writes the summary has
    // nothing to abandon in an already abandoned round.
    let already_abandoned = !matches!(admission, Admission::Replaceable)
        && summary.current.status == RoundStatus::Abandoned
        && summary
            .current
            .abandonment
            .as_ref()
            .is_some_and(|closed| closed.reason == request.reason);
    if summary.current.number != request.round
        || !(already_abandoned
            || matches!(
                summary.current.status,
                RoundStatus::Pending | RoundStatus::Partial
            ))
    {
        return Err(PlanError::NotAbandonable);
    }
    for round in summary
        .history
        .iter()
        .chain(std::iter::once(&summary.current))
    {
        validate_historical_roots(round, &snapshot)?;
        if finished_communication(round) {
            validate_completed_actions(round, &snapshot)?;
        }
    }
    validate_retained_structure(&summary.current, &snapshot)?;
    let mut report = PublicationResult {
        final_snapshot: snapshot.clone(),
        status: PublicationStatus::NotStarted,
        decision: summary.current.assessment.decision(),
        round: request.round,
        planned: Vec::new(),
        attempted: Vec::new(),
        verified: Vec::new(),
        retried_creations: progress.retries_in_round(request.round),
        skipped: summary.current.actions.clone(),
        stop: None,
        finalization_stop: None,
    };
    if !progress.reconcile_recorded(&snapshot, &request.reviewer, evidence)? {
        if let Some(pending) = progress.unresolved.clone()
            && pending.phase == super::progress::WritePhase::Prepared
            && pending.abandonment_round() == Some(request.round)
            && let Write::Summary {
                expected: Some(expected),
                round,
                ..
            } = &pending.write
            && expected.as_ref() == &summary
            && round
                .abandonment
                .as_ref()
                .is_some_and(|closed| closed.reason == request.reason)
        {
            let mut execution = Execution {
                session,
                reviewer: &request.reviewer,
                authority: request.effects.intersection(&authority),
                revision: snapshot.revision.clone(),
                snapshot,
                evidence,
                progress,
                install: None,
            };
            match execution.apply_retained(pending.write.clone(), true, &mut report, Some(pending))
            {
                Ok(()) => report.status = PublicationStatus::Abandoned,
                Err(stop) => {
                    report.status = status_for(&stop, report.attempted.is_empty());
                    report.stop = Some(stop);
                }
            }
            return Ok(report);
        }
        // Any other prepared write never left and cannot resume through abandonment.
        if !progress.discard_unsent(&request.subject, &request.reviewer, evidence)? {
            report.status = PublicationStatus::Uncertain;
            report.stop = Some(Stop::UnresolvedWrite);
            return Ok(report);
        }
    }
    if already_abandoned {
        report.status = PublicationStatus::Abandoned;
        return Ok(report);
    }
    let observed_snapshot =
        discover_managed(&mut session.reader(), &request.subject, &request.reviewer)
            .map_err(super::planning::discovery_error)?;
    report.final_snapshot = observed_snapshot.clone();
    if observed_snapshot
        .summaries
        .iter()
        .find(|record| record.author == request.reviewer)
        != Some(&summary)
    {
        report.stop = Some(Stop::ConcurrentChange);
        return Ok(report);
    }
    for root in snapshot
        .findings
        .iter()
        .filter(|root| root.author == request.reviewer)
    {
        let observed = observed_snapshot
            .findings
            .iter()
            .find(|observed| {
                observed.author == request.reviewer && observed.finding.id() == root.finding.id()
            })
            .ok_or(PlanError::MissingPublishedFinding(root.finding.id()))?;
        let mut expected = root.clone();
        expected.resolved = observed.resolved;
        if expected != *observed {
            return Err(PlanError::ConflictingFinding(root.finding.id()));
        }
    }
    for reply in snapshot
        .replies
        .iter()
        .filter(|reply| reply.author == request.reviewer && reply.action.is_some())
    {
        if let Some(action) = &reply.action
            && !observed_snapshot.replies.contains(reply)
        {
            return Err(PlanError::ConflictingReply(action.finding));
        }
    }
    for round in summary
        .history
        .iter()
        .chain(std::iter::once(&summary.current))
    {
        validate_historical_roots(round, &observed_snapshot)?;
        if finished_communication(round) {
            validate_completed_actions(round, &observed_snapshot)?;
        }
    }
    validate_retained_structure(&summary.current, &observed_snapshot)?;
    observed_remaining_actions(&summary.current, &observed_snapshot)?;
    let mut committed = Vec::new();
    for action in &summary.current.actions {
        let write = action_write(
            action,
            summary.current.number,
            summary.current.assessment.revision(),
            &observed_snapshot,
            summary.current.assessment.reviewer(),
        )
        .map_err(PlanError::MissingPublishedFinding)?;
        if verified_write(&write, &observed_snapshot, &request.reviewer) {
            committed.push(action.clone());
        }
    }
    report.skipped = summary
        .current
        .actions
        .iter()
        .filter(|action| !committed.contains(action))
        .cloned()
        .collect();
    let mut round = summary.current.clone();
    round.status = RoundStatus::Abandoned;
    round.abandonment = Some(Abandonment {
        reason: request.reason.clone(),
        verified_actions: committed,
    });
    crate::summary::retain_communicated_findings(&mut round, &summary.history, &observed_snapshot)?;
    for receipt in progress.verified_resolutions.iter().filter(|receipt| {
        receipt.subject == request.subject
            && receipt.reviewer == request.reviewer
            && receipt.round == round.number
    }) {
        if !round.verified_resolutions.contains(receipt) {
            round.verified_resolutions.push(receipt.clone());
        }
    }
    crate::summary::retain_verified_resolutions(&mut round);
    let body = nonempty(
        renderer
            .summary(&crate::summary::summary_view(
                &round,
                &summary.history,
                &observed_snapshot,
            )?)
            .map_err(PlanError::Render)?,
    )?;
    let write = Write::Summary {
        expected: Some(Box::new(summary.clone())),
        round: Box::new(round),
        history: summary.history.clone(),
        body,
    };
    let mut execution = Execution {
        session,
        reviewer: &request.reviewer,
        authority: request.effects.intersection(&authority),
        revision: observed_snapshot.revision.clone(),
        snapshot: observed_snapshot,
        evidence,
        progress,
        install,
    };
    match execution.apply(write, true, &mut report) {
        Ok(()) => {
            report.status = PublicationStatus::Abandoned;
        }
        Err(stop) => {
            let no_writes = report.attempted.is_empty()
                || (report.verified.is_empty() && matches!(stop, Stop::Rejected(_)));
            report.status = status_for(&stop, no_writes);
            report.stop = Some(stop);
        }
    }
    Ok(report)
}
