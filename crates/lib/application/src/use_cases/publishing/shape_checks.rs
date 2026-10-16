use super::{
    planning::{PlanError, PublishRequest, same_finding, summary_context},
    reconciliation::{
        finished_communication, previous_finding, validate_completed_actions,
        validate_historical_roots, validate_retained_intent,
    },
};
use crate::observation as log;
use crate::{
    publication::{RoundStatus, Snapshot},
    use_cases::discovery::highest_finding_id,
};
use review_kit_domain::{FindingStatus, ReviewDecision};

#[review_kit_observation::operations]
#[operation(name = "application.validation.publication_shape", input = log::request(request), output = |errors| {
            if errors.is_empty() {
                log::Completion::success(log::Fields::new())
            } else {
                log::Completion::failure(log::plan_errors(errors))
            }
        })]
pub(super) fn check_shapes(request: &PublishRequest, snapshot: &Snapshot) -> Vec<PlanError> {
    let assessment = &request.assessment;
    let reviewer = assessment.reviewer();
    let owned: Vec<_> = snapshot
        .findings
        .iter()
        .filter(|root| &root.author == reviewer)
        .collect();
    let summary = snapshot.summaries.iter().find(|s| &s.author == reviewer);
    let matching = summary.filter(|s| s.current.assessment.revision() == assessment.revision());
    let mut errors = Vec::new();
    if request
        .review_workflow
        .as_ref()
        .is_some_and(|name| name.trim().is_empty())
    {
        errors.push(PlanError::InvalidWorkflow);
    }
    let selection = request.policies.validate(assessment);
    if selection.is_empty() {
        if let Err(error) = crate::publication::validate_policy_declarations(
            &request.policy_declarations,
            assessment,
            request.policies.policies(),
        ) {
            errors.push(PlanError::PolicyDeclarations(error));
        }
    } else {
        errors.push(PlanError::Coverage(selection));
    }
    if &snapshot.revision != assessment.revision() {
        errors.push(PlanError::ChangedRevision);
    }
    for root in &owned {
        if !assessment
            .findings()
            .iter()
            .any(|f| f.id() == root.finding.id())
        {
            errors.push(PlanError::MissingCarriedFinding(root.finding.id()));
        }
    }
    if let Some(summary) = summary {
        for round in summary
            .history
            .iter()
            .chain(std::iter::once(&summary.current))
        {
            if let Err(error) = validate_historical_roots(round, snapshot) {
                errors.push(error);
            } else if finished_communication(round)
                && let Err(error) = validate_completed_actions(round, snapshot)
            {
                errors.push(error);
            }
        }
    }
    if let Some(summary) = matching.filter(|s| {
        matches!(
            s.current.status,
            RoundStatus::Pending | RoundStatus::Partial
        )
    }) {
        if summary.current.assessment != *assessment
            || summary.current.policy_declarations != request.policy_declarations
            || summary.current.answers != request.answers
            || summary.current.transitions != request.transitions
            || summary.current.review_workflow != request.review_workflow
            || summary
                .current
                .policy_set
                .as_ref()
                .is_some_and(|name| name != request.policies.name())
        {
            errors.push(PlanError::ChangedPendingAssessment);
        }
        if let Err(error) = validate_retained_intent(&summary.current, snapshot) {
            errors.push(error);
        }
    }
    let number = match summary_context(request, snapshot) {
        Ok((round, _)) => Some(round.number),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let highest = highest_finding_id(snapshot, reviewer);
    for finding in assessment.findings() {
        if let Some(root) = owned.iter().find(|root| root.finding.id() == finding.id()) {
            if matching.is_some_and(|s| {
                matches!(
                    s.current.status,
                    RoundStatus::Pending | RoundStatus::Partial | RoundStatus::Blocked
                )
            }) && number.is_some_and(|number| root.created_in > number)
            {
                errors.push(PlanError::ConflictingFinding(finding.id()));
            }
            match previous_finding(snapshot, root, reviewer) {
                Ok(previous) => {
                    if !assessment.selected_policies().contains(finding.policy_id())
                        && previous.policy_id() != finding.policy_id()
                    {
                        errors.push(PlanError::UnselectedHistoricalPolicy(finding.id()));
                    }
                    if !same_finding(&previous, finding)
                        && !request
                            .transitions
                            .iter()
                            .any(|r| r.finding == finding.id())
                    {
                        errors.push(PlanError::MissingTransition(finding.id()));
                    }
                }
                Err(error) => errors.push(error),
            }
        } else {
            if !assessment.selected_policies().contains(finding.policy_id()) {
                errors.push(PlanError::UnselectedHistoricalPolicy(finding.id()));
            }
            let reserved = matching.is_some_and(|s| {
                s.current.status != RoundStatus::Abandoned
                    && s.current
                        .assessment
                        .findings()
                        .iter()
                        .any(|f| f.id() == finding.id())
            });
            if highest.is_some_and(|highest| finding.id() <= highest) && !reserved {
                errors.push(PlanError::ReusedFindingId(finding.id()));
            }
            if finding.status() != FindingStatus::Open
                && !(assessment.decision() == ReviewDecision::Blocked
                    && finding.status() == FindingStatus::Uncertain)
            {
                errors.push(PlanError::NewFindingNotOpen(finding.id()));
            }
            if !request.allow_colocated
                && (owned
                    .iter()
                    .any(|root| root.finding.location() == finding.location())
                    || assessment.findings().iter().any(|other| {
                        other.id() < finding.id() && other.location() == finding.location()
                    }))
            {
                errors.push(PlanError::ColocatedFinding(finding.id()));
            }
        }
    }
    errors
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn shape_error(mut errors: Vec<PlanError>) -> PlanError {
    if errors.len() == 1 {
        errors.remove(0)
    } else {
        PlanError::ShapeChecks(errors)
    }
}
