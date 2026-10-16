use super::{PlanError, PublicationProgress, PublishRequest, planning::nonempty};
use crate::observation as log;
use crate::publication::{Plan, ResolutionReceipt, Round, RoundStatus, Snapshot};

#[review_kit_observation::operations]
#[operation(name = "application.publication.reconcile_history", input = log::Fields::new().count("receipts", progress.verified_resolutions.len()), output = |result| log::Completion::result(result, log::plan, log::plan_error))]
pub(super) fn with_progress(
    mut plan: Plan,
    request: &PublishRequest,
    snapshot: &Snapshot,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    progress: &PublicationProgress,
) -> Result<Plan, PlanError> {
    let mut changed = false;
    for receipt in &progress.verified_resolutions {
        if &receipt.subject != request.assessment.subject()
            || &receipt.reviewer != request.assessment.reviewer()
        {
            return Err(PlanError::ProgressBinding);
        }
        if let Some(round) = plan
            .history
            .iter_mut()
            .chain(std::iter::once(&mut plan.round))
            .find(|round| round.number == receipt.round)
        {
            changed |= merge_receipt(round, receipt, snapshot)?;
        }
    }
    if !changed {
        return Ok(plan);
    }
    let mut preview_round = plan.round.clone();
    plan.summary_body = nonempty(
        renderer
            .summary(&crate::summary::summary_view(
                &preview_round,
                &plan.history,
                snapshot,
            )?)
            .map_err(PlanError::Render)?,
    )?;
    preview_round.status =
        if request.assessment.decision() == review_kit_domain::ReviewDecision::Blocked {
            RoundStatus::Blocked
        } else {
            RoundStatus::Completed
        };
    plan.terminal_summary_preview = nonempty(
        renderer
            .summary(&crate::summary::summary_view(
                &preview_round,
                &plan.history,
                snapshot,
            )?)
            .map_err(PlanError::Render)?,
    )?;
    Ok(plan)
}

#[review_kit_observation::operations]
#[helper]
fn merge_receipt(
    round: &mut Round,
    receipt: &ResolutionReceipt,
    snapshot: &Snapshot,
) -> Result<bool, PlanError> {
    if round.verified_resolutions.contains(receipt) {
        return Ok(false);
    }
    if round
        .verified_resolutions
        .iter()
        .any(|prior| prior.thread == receipt.thread)
    {
        return Err(PlanError::InconsistentRound);
    }
    let root = snapshot
        .findings
        .iter()
        .find(|root| root.author == receipt.reviewer && root.id == receipt.thread)
        .ok_or(PlanError::InconsistentRound)?;
    if !round.plans_resolution(root.finding.id(), receipt.resolved) {
        return Err(PlanError::InconsistentRound);
    }
    round.verified_resolutions.push(receipt.clone());
    crate::summary::retain_verified_resolutions(round);
    Ok(true)
}
