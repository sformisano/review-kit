use crate::observation as log;
use review_kit_domain::{
    Assessment, Finding, FindingId, FindingStatus, PolicyJudgment, ReviewDecision,
};

use super::reconciliation::{previous_finding, remaining_actions, unfinished_resolution};
use super::reply_checks::{check_replies, reply_error};
use super::shape_checks::{check_shapes, shape_error};
use crate::use_cases::{
    assessment::{AssessmentPolicies, CheckIssue, PolicyCoverageIssue, check_findings},
    discovery::{DiscoveryError, discover_managed, validate_snapshot},
};
use crate::{
    ports::{CommittedSource, PortError, ReviewReader, ReviewRenderer},
    publication::*,
};

/// Agent assessment and authored communication inputs bound to agent-declared coverage.
/// Requested effects limit publication; host authority and current permissions
/// must also permit each write. Private context is excluded from public output.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub assessment: Assessment,
    pub review_workflow: Option<String>,
    pub policies: AssessmentPolicies,
    pub policy_declarations: Vec<PublicPolicyDeclaration>,
    pub effects: Effects,
    pub answers: Vec<Answer>,
    /// Question observations retained when authoring new answers, before live discovery.
    pub answer_targets: Vec<ReplyRecord>,
    pub transitions: Vec<TransitionReply>,
    /// Private agent notes are excluded from rounds, writes, and renderer inputs.
    pub local_context: String,
    /// Permit a new finding to share another finding's path and line.
    pub allow_colocated: bool,
    /// Whether acceptance validated open or uncertain blocking findings only from
    /// recorded confirmations. The planned round records this value.
    pub confirmed_validation: bool,
}

// Grouped failures display counts while retaining their detailed vector payloads.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("{} publication shape checks failed", .0.len())]
    ShapeChecks(Vec<PlanError>),
    #[error("{} reply checks failed", .0.len())]
    ReplyChecks(Vec<PlanError>),
    #[error("{0}")]
    Evidence(#[source] PortError),
    #[error("{} policy coverage checks failed", .0.len())]
    Coverage(Vec<PolicyCoverageIssue>),
    #[error(transparent)]
    PolicyDeclarations(#[from] InvalidPolicyDeclarations),
    #[error("finding {0} needs a transition explanation")]
    MissingTransition(FindingId),
    #[error("finding {0} transition does not match communicated state")]
    InvalidTransition(FindingId),
    #[error("finding {0} has duplicate transition replies")]
    DuplicateTransition(FindingId),
    #[error("{0}")]
    Discovery(#[source] DiscoveryError),
    #[error("reviewed revision is no longer current")]
    ChangedRevision,
    #[error(
        "the publication would write to an existing thread that the freshness check did not compare"
    )]
    UncomparedWrite,
    #[error("assessment reviewer differs from authenticated actor")]
    WrongReviewer,
    #[error("publication progress belongs to another subject or reviewer")]
    ProgressBinding,
    #[error("{} assessment checks failed", .0.len())]
    Checks(Vec<CheckIssue>),
    #[error("finding {0} needs a fresh assessment")]
    MissingCarriedFinding(FindingId),
    #[error("finding {0} changes an unselected historical policy")]
    UnselectedHistoricalPolicy(FindingId),
    #[error("published finding {0} is missing")]
    MissingPublishedFinding(FindingId),
    #[error("finding {0} reuses a reserved identity")]
    ReusedFindingId(FindingId),
    #[error("new finding {0} must be open")]
    NewFindingNotOpen(FindingId),
    #[error("finding {0} needs explicit permission for a shared location")]
    ColocatedFinding(FindingId),
    #[error("answer target for finding {0} is unavailable")]
    InvalidAnswer(FindingId),
    #[error("only one answer per finding is permitted in a round")]
    DuplicateAnswer,
    #[error("round identity space is exhausted")]
    RoundExhausted,
    #[error("recovery requires the retained assessment and answers")]
    ChangedPendingAssessment,
    #[error("retained round intent is inconsistent with its assessment")]
    InconsistentRound,
    #[error("reply evidence conflicts for finding {0}")]
    ConflictingReply(FindingId),
    #[error("published evidence conflicts for finding {0}")]
    ConflictingFinding(FindingId),
    #[error("{0}")]
    Render(#[source] PortError),
    #[error("rendered body must not be blank")]
    EmptyBody,
    #[error("rendered body contains a reserved review marker")]
    ReservedMarker,
    #[error("only the exact pending or partial round can be abandoned")]
    NotAbandonable,
    #[error("abandonment requires a nonblank public reason without reserved markers")]
    InvalidAbandonmentReason,
    #[error(
        "the render profile differs from the retained unfinished round; restore its original profile before recovery"
    )]
    ChangedRenderProfile,
    #[error(
        "the unfinished round has no retained render profile identity; recovery cannot safely establish the selected profile"
    )]
    UnknownPendingRenderProfile,
    #[error("review workflow public name must not be blank")]
    InvalidWorkflow,
    #[error("policy display name is unavailable: {0}")]
    MissingPolicyName(review_kit_domain::PolicyId),
    #[error(
        "the retained review has different public workflow or policy-set identities; preserve those identities for an unchanged assessment"
    )]
    ChangedReviewIdentity,
    /// Journal binding refused before any provider write.
    #[error(transparent)]
    Binding(#[from] super::BindingError),
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn discovery_error(error: DiscoveryError) -> PlanError {
    match error {
        DiscoveryError::Conflict(
            crate::use_cases::discovery::DiscoveryConflict::MissingPublishedFinding(id),
        ) => PlanError::MissingPublishedFinding(id),
        DiscoveryError::Conflict(
            crate::use_cases::discovery::DiscoveryConflict::ConflictingReply(id),
        ) => PlanError::ConflictingReply(id),
        error => PlanError::Discovery(error),
    }
}

#[review_kit_observation::operations]
#[helper]
/// Discover current review observations and derive a publication plan without writes.
///
/// Preflight checks agent-declared coverage, retained history, source, and available bodies.
/// New finding references remain unresolved until publication verifies their provider
/// identities. Success does not establish authority or guarantee later publication.
pub fn plan(
    request: &PublishRequest,
    reader: &mut dyn ReviewReader,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
) -> Result<Plan, PlanError> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    let mut reader = crate::ports::ManagedReader::new(reader);
    plan_managed(request, &mut reader, &mut source, &mut renderer)
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.plan", input = log::request(request), output = |result| log::Completion::result(result, log::plan, log::plan_error))]
pub(crate) fn plan_managed(
    request: &PublishRequest,
    reader: &mut crate::ports::ManagedReader<'_>,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
) -> Result<Plan, PlanError> {
    let snapshot = discover_managed(
        reader,
        request.assessment.subject(),
        request.assessment.reviewer(),
    )
    .map_err(discovery_error)?;
    plan_from_snapshot_managed(request, &snapshot, source, renderer)
}

#[review_kit_observation::operations]
#[helper]
/// Derive a publication plan from retained review observations without rediscovery.
///
/// This operation acquires no publication session and performs no provider writes.
/// Source and renderer adapters can perform I/O. Snapshot conflicts or preflight
/// failures return `PlanError`; final new-finding references remain unresolved.
pub fn plan_from_snapshot(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
) -> Result<Plan, PlanError> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    plan_from_snapshot_managed(request, snapshot, &mut source, &mut renderer)
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.plan_from_snapshot", input = log::request(request), output = |result| log::Completion::result(result, log::plan, log::plan_error))]
pub(crate) fn plan_from_snapshot_managed(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
) -> Result<Plan, PlanError> {
    validate_snapshot(
        snapshot,
        request.assessment.subject(),
        request.assessment.reviewer(),
    )
    .map_err(|e| discovery_error(DiscoveryError::Conflict(e)))?;
    build_plan(request, snapshot, source, renderer)
}

#[review_kit_observation::operations]
#[helper]
/// Plan using retained confirmed resolution receipts as well as provider observations.
pub fn plan_from_snapshot_with_progress(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
    progress: &super::PublicationProgress,
) -> Result<Plan, PlanError> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    plan_from_snapshot_with_progress_managed(
        request,
        snapshot,
        &mut source,
        &mut renderer,
        progress,
    )
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.plan_with_progress", input = log::request(request), output = |result| log::Completion::result(result, log::plan, log::plan_error))]
pub(crate) fn plan_from_snapshot_with_progress_managed(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    progress: &super::PublicationProgress,
) -> Result<Plan, PlanError> {
    super::progress::check_unresolved_profile(progress, renderer)?;
    let plan = plan_from_snapshot_managed(request, snapshot, source, renderer)?;
    super::history::with_progress(plan, request, snapshot, renderer, progress)
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.build_plan", input = log::request(request), output = |result| log::Completion::result(result, log::plan, log::plan_error))]
pub(super) fn build_plan(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
) -> Result<Plan, PlanError> {
    let assessment = &request.assessment;
    check_pending_profile(snapshot, assessment.reviewer(), renderer)?;
    if assessment.decision() == ReviewDecision::Blocked {
        let mut errors = super::checking::finding_input_errors(request, snapshot, source, renderer);
        if !errors.is_empty() {
            return Err(if errors.len() == 1 {
                errors.remove(0)
            } else {
                PlanError::ShapeChecks(errors)
            });
        }
    }
    let shapes = check_shapes(request, snapshot);
    if !shapes.is_empty() {
        return Err(shape_error(shapes));
    }
    let owned: Vec<_> = snapshot
        .findings
        .iter()
        .filter(|f| &f.author == assessment.reviewer())
        .collect();
    let summary = snapshot
        .summaries
        .iter()
        .find(|s| &s.author == assessment.reviewer());
    let matching = summary.filter(|s| s.current.assessment.revision() == assessment.revision());
    let same = matching.is_some_and(|s| same_communication(&s.current.assessment, assessment));
    let terminal_assessment_match = same
        && matching.is_some_and(|s| already_communicated_inputs(&s.current, request))
        && matching.is_some_and(|s| {
            matches!(
                s.current.status,
                RoundStatus::Completed | RoundStatus::Blocked
            )
        });
    if terminal_assessment_match
        && let Some(previous) = matching
        && (previous
            .current
            .review_workflow
            .as_ref()
            .is_some_and(|workflow| request.review_workflow.as_ref() != Some(workflow))
            || previous
                .current
                .policy_set
                .as_ref()
                .is_some_and(|set| set != request.policies.name()))
    {
        return Err(PlanError::ChangedReviewIdentity);
    }
    if terminal_assessment_match
        && let Some(previous) =
            matching.filter(|s| s.current.policy_declarations == request.policy_declarations)
    {
        let preview_round = &previous.current;
        let preview = if renderer.identity().is_some()
            && renderer.identity() != previous.current.renderer.as_ref()
        {
            nonempty(
                renderer
                    .summary(&crate::summary::summary_view(
                        preview_round,
                        &previous.history,
                        snapshot,
                    )?)
                    .map_err(PlanError::Render)?,
            )?
        } else {
            previous.body.clone()
        };
        return Ok(Plan {
            round: previous.current.clone(),
            history: previous.history.clone(),
            actions: Vec::new(),
            unchanged: true,
            summary_body: preview.clone(),
            terminal_summary_preview: preview,
            unresolved_findings: Vec::new(),
        });
    }
    let (mut round, history) = summary_context(request, snapshot)?;
    round.renderer = renderer.identity().cloned();
    let render_context = public_render_context(&round);
    let policy_names = cumulative_policy_names(&round, &history);
    let number = round.number;
    let blocked = assessment.decision() == ReviewDecision::Blocked;
    let mut actions = Vec::new();
    let checked_replies = check_replies(request, snapshot, renderer, matching.map(|s| &s.current))
        .map_err(reply_error)?;
    round.answer_targets = checked_replies.targets.clone();
    let retained = matching.filter(|s| {
        matches!(
            s.current.status,
            RoundStatus::Pending | RoundStatus::Partial
        )
    });
    if let Some(previous) = retained {
        round.actions = previous.current.actions.clone();
        round.answer_targets = previous.current.answer_targets.clone();
        actions = remaining_actions(&round, snapshot)?;
        let needing_source = source_checks(assessment, snapshot)?;
        let checked = check_findings(
            assessment,
            &needing_source,
            source,
            renderer,
            &render_context,
            &policy_names,
        )
        .map_err(PlanError::Checks)?;
        for action in &actions {
            if let PlannedAction::Finding { finding, body } = action
                && !checked
                    .finding_bodies
                    .iter()
                    .any(|(id, rendered)| *id == finding.id() && rendered == body)
            {
                return Err(PlanError::ConflictingFinding(finding.id()));
            }
            if let PlannedAction::Reply {
                finding,
                kind,
                body,
            } = action
            {
                let expected = match kind {
                    ReplyKind::Transition(_) => checked_replies.transitions.get(finding),
                    ReplyKind::Answer(target) => {
                        checked_replies.answers.get(&(*finding, target.clone()))
                    }
                }
                .ok_or(PlanError::InconsistentRound)?;
                if expected != body {
                    return Err(PlanError::ConflictingReply(*finding));
                }
            }
        }
    } else {
        let missing = source_checks(assessment, snapshot)?;
        let checked = check_findings(
            assessment,
            &missing,
            source,
            renderer,
            &render_context,
            &policy_names,
        )
        .map_err(PlanError::Checks)?;
        let mut ordered_findings: Vec<_> = assessment
            .findings()
            .iter()
            .filter(|finding| finding_effects_allowed(assessment, finding))
            .collect();
        ordered_findings.sort_by_key(|f| f.id());
        for finding in ordered_findings {
            let root = owned.iter().find(|r| r.finding.id() == finding.id());
            if let Some(root) = root {
                let reply = request
                    .transitions
                    .iter()
                    .find(|r| r.finding == finding.id());
                let new_transition = match reply {
                    Some(reply) => !super::reconciliation::communicated_transition(
                        snapshot, root, finding, reply,
                    )?,
                    None => false,
                };
                if new_transition {
                    let body = checked_replies
                        .transitions
                        .get(&finding.id())
                        .ok_or(PlanError::MissingTransition(finding.id()))?
                        .clone();
                    push_reply(
                        snapshot,
                        assessment,
                        &root.id,
                        ActionId {
                            round: number,
                            finding: finding.id(),
                            kind: ReplyKind::Transition(finding.status()),
                        },
                        body,
                        &mut actions,
                    )?;
                }
                let transition_resolution =
                    reply.filter(|_| new_transition).and_then(|reply| {
                        match (reply.from, reply.to) {
                            (
                                FindingStatus::Open | FindingStatus::Uncertain,
                                FindingStatus::Fixed | FindingStatus::Withdrawn,
                            ) => Some(true),
                            (
                                FindingStatus::Fixed | FindingStatus::Withdrawn,
                                FindingStatus::Open,
                            ) => Some(false),
                            _ => None,
                        }
                    });
                let resolution = match transition_resolution {
                    Some(resolved) => Some(resolved),
                    None => unfinished_resolution(snapshot, root, finding)?,
                };
                if let Some(resolved) = resolution
                    && root.resolved != resolved
                {
                    actions.push(PlannedAction::SetResolved {
                        finding: finding.id(),
                        resolved,
                    });
                }
            } else {
                let body = checked
                    .finding_bodies
                    .iter()
                    .find(|(id, _)| *id == finding.id())
                    .map(|(_, body)| body.clone())
                    .ok_or(PlanError::EmptyBody)?;
                actions.push(PlannedAction::Finding {
                    finding: Box::new(finding.clone()),
                    body,
                });
            }
        }
        for answer in &request.answers {
            if !assessment.findings().iter().any(|finding| {
                finding.id() == answer.finding && finding_effects_allowed(assessment, finding)
            }) {
                continue;
            }
            let root = owned
                .iter()
                .find(|r| r.finding.id() == answer.finding)
                .ok_or(PlanError::InvalidAnswer(answer.finding))?;
            if super::reconciliation::already_answered(snapshot, root, answer)? {
                continue;
            }
            let body = checked_replies
                .answers
                .get(&(answer.finding, answer.in_reply_to.clone()))
                .ok_or(PlanError::InvalidAnswer(answer.finding))?
                .clone();
            push_reply(
                snapshot,
                assessment,
                &root.id,
                ActionId {
                    round: number,
                    finding: answer.finding,
                    kind: ReplyKind::Answer(answer.in_reply_to.clone()),
                },
                body,
                &mut actions,
            )?;
        }
    }
    if retained.is_none() {
        round.actions = actions.clone();
    }
    if blocked && !round.actions.is_empty() {
        round.status = retained.map_or(RoundStatus::Pending, |summary| summary.current.status);
    }
    crate::summary::retain_communicated_findings(&mut round, &history, snapshot)?;
    let summary_body = nonempty(
        renderer
            .summary(&crate::summary::summary_view(&round, &history, snapshot)?)
            .map_err(PlanError::Render)?,
    )?;
    let mut terminal = round.clone();
    terminal.status = if blocked {
        RoundStatus::Blocked
    } else {
        RoundStatus::Completed
    };
    let terminal_summary_preview = nonempty(
        renderer
            .summary(&crate::summary::summary_view(
                &terminal, &history, snapshot,
            )?)
            .map_err(PlanError::Render)?,
    )?;
    let unresolved_findings = actions
        .iter()
        .filter_map(|action| match action {
            PlannedAction::Finding { finding, .. } => Some(finding.id()),
            _ => None,
        })
        .collect();
    Ok(Plan {
        round,
        history,
        actions,
        unchanged: false,
        summary_body,
        terminal_summary_preview,
        unresolved_findings,
    })
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn finding_effects_allowed(assessment: &Assessment, finding: &Finding) -> bool {
    if assessment.decision() != ReviewDecision::Blocked {
        return true;
    }
    finding.validated()
        && finding.status() != FindingStatus::Uncertain
        && assessment
            .evaluations()
            .iter()
            .find(|evaluation| evaluation.policy_id() == finding.policy_id())
            .map_or_else(
                || {
                    !assessment.selected_policies().contains(finding.policy_id())
                        && matches!(
                            finding.status(),
                            FindingStatus::Fixed | FindingStatus::Withdrawn
                        )
                },
                |evaluation| !matches!(evaluation.judgment(), PolicyJudgment::Blocked { .. }),
            )
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn source_checks<'a>(
    assessment: &'a Assessment,
    snapshot: &Snapshot,
) -> Result<Vec<&'a Finding>, PlanError> {
    let mut needed = Vec::new();
    for finding in assessment
        .findings()
        .iter()
        .filter(|f| f.status() == FindingStatus::Open)
    {
        match snapshot
            .findings
            .iter()
            .find(|r| &r.author == assessment.reviewer() && r.finding.id() == finding.id())
        {
            None => needed.push(finding),
            Some(root) => {
                let previous = previous_finding(snapshot, root, assessment.reviewer())?;
                if previous.location() != finding.location()
                    || !same_verified_suggestion(previous.suggestion(), finding.suggestion())
                {
                    needed.push(finding);
                }
            }
        }
    }
    Ok(needed)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn summary_context(
    request: &PublishRequest,
    snapshot: &Snapshot,
) -> Result<(Round, Vec<Round>), PlanError> {
    let assessment = &request.assessment;
    let reviewer = assessment.reviewer();
    let summary = snapshot.summaries.iter().find(|s| &s.author == reviewer);
    let matching = summary.filter(|s| s.current.assessment.revision() == assessment.revision());
    let reused = matching.is_some_and(|s| {
        matches!(
            s.current.status,
            RoundStatus::Pending | RoundStatus::Partial
        ) || (s.current.status == RoundStatus::Blocked
            && s.current.actions.is_empty()
            && s.current.policy_declarations == request.policy_declarations)
            || (same_communication(&s.current.assessment, assessment)
                && s.current.policy_declarations == request.policy_declarations
                && already_communicated_inputs(&s.current, request)
                && matches!(
                    s.current.status,
                    RoundStatus::Completed | RoundStatus::Blocked
                ))
    });
    let highest = summary
        .map(|s| s.current.number)
        .into_iter()
        .chain(
            snapshot
                .findings
                .iter()
                .filter(|r| &r.author == reviewer)
                .map(|r| r.created_in),
        )
        .chain(
            snapshot
                .replies
                .iter()
                .filter(|r| &r.author == reviewer)
                .filter_map(|r| r.action.as_ref().map(|action| action.round)),
        )
        .max()
        .unwrap_or(0);
    let number = if reused {
        matching
            .map(|s| s.current.number)
            .ok_or(PlanError::RoundExhausted)?
    } else {
        highest.checked_add(1).ok_or(PlanError::RoundExhausted)?
    };
    let mut history = summary.map_or_else(Vec::new, |s| s.history.clone());
    if !reused && let Some(summary) = summary {
        let mut previous = summary.current.clone();
        if matches!(previous.status, RoundStatus::Pending | RoundStatus::Partial)
            && previous.communicated_findings.is_some()
        {
            crate::summary::retain_communicated_findings(
                &mut previous,
                &summary.history,
                snapshot,
            )?;
        }
        history.push(previous);
    }
    Ok((
        Round {
            number,
            abandonment: None,
            assessment: assessment.clone(),
            status: if assessment.decision() == ReviewDecision::Blocked {
                RoundStatus::Blocked
            } else {
                RoundStatus::Pending
            },
            answers: request.answers.clone(),
            transitions: request.transitions.clone(),
            answer_targets: Vec::new(),
            actions: Vec::new(),
            publication_limitation: None,
            review_workflow: request.review_workflow.clone(),
            policy_set: matching
                .filter(|summary| {
                    matches!(
                        summary.current.status,
                        RoundStatus::Pending | RoundStatus::Partial
                    )
                })
                .map_or_else(
                    || Some(request.policies.name().to_owned()),
                    |summary| summary.current.policy_set.clone(),
                ),
            policy_names: matching
                .filter(|summary| {
                    matches!(
                        summary.current.status,
                        RoundStatus::Pending | RoundStatus::Partial
                    )
                })
                .map(|summary| summary.current.policy_names.clone())
                .unwrap_or_else(|| {
                    request
                        .policies
                        .policies()
                        .iter()
                        .map(|policy| PolicyName {
                            id: policy.id.clone(),
                            name: policy.name.clone(),
                        })
                        .collect()
                }),
            policy_declarations: request.policy_declarations.clone(),
            renderer: None,
            communicated_findings: matching
                .filter(|_| reused)
                .and_then(|summary| summary.current.communicated_findings.clone()),
            verified_resolutions: matching
                .filter(|_| reused)
                .map_or_else(Vec::new, |summary| {
                    summary.current.verified_resolutions.clone()
                }),
            confirmed_validation: request.confirmed_validation,
        },
        history,
    ))
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn check_pending_profile(
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
    renderer: &crate::ports::ManagedRenderer<'_>,
) -> Result<(), PlanError> {
    if let Some(round) = snapshot
        .summaries
        .iter()
        .find(|summary| &summary.author == reviewer)
        .map(|summary| &summary.current)
        .filter(|round| matches!(round.status, RoundStatus::Pending | RoundStatus::Partial))
    {
        match (&round.renderer, renderer.identity()) {
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
pub(super) fn public_render_context(round: &Round) -> crate::ports::PublicRenderContext {
    crate::ports::PublicRenderContext {
        round: round.number,
        revision: round.assessment.revision().clone(),
        publication_status: round.status,
        review_workflow: round.review_workflow.clone(),
        policy_set: round.policy_set.clone().unwrap_or_default(),
        scope: round.assessment.scope().to_owned(),
        depth: round.assessment.depth().to_owned(),
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn cumulative_policy_names(round: &Round, history: &[Round]) -> Vec<PolicyName> {
    let mut names = std::collections::BTreeMap::new();
    for retained in history.iter().chain(std::iter::once(round)) {
        for policy in &retained.policy_names {
            names.insert(policy.id.clone(), policy.name.clone());
        }
    }
    names
        .into_iter()
        .map(|(id, name)| PolicyName { id, name })
        .collect()
}

#[review_kit_observation::operations]
#[helper]
fn already_communicated_inputs(round: &Round, request: &PublishRequest) -> bool {
    let mut answers = std::collections::BTreeSet::new();
    let mut transitions = std::collections::BTreeSet::new();
    request
        .answers
        .iter()
        .all(|answer| answers.insert(answer.finding) && round.answers.contains(answer))
        && request
            .transitions
            .iter()
            .all(|reply| transitions.insert(reply.finding) && round.transitions.contains(reply))
}

#[review_kit_observation::operations]
#[helper]
fn push_reply(
    snapshot: &Snapshot,
    assessment: &Assessment,
    thread: &ProviderId,
    id: ActionId,
    body: String,
    actions: &mut Vec<PlannedAction>,
) -> Result<(), PlanError> {
    let matches: Vec<_> = snapshot
        .replies
        .iter()
        .filter(|r| &r.author == assessment.reviewer() && r.action.as_ref() == Some(&id))
        .collect();
    match matches.as_slice() {
        [] => actions.push(PlannedAction::Reply {
            finding: id.finding,
            kind: id.kind,
            body,
        }),
        [reply] if &reply.thread == thread && reply.body == body => {}
        _ => return Err(PlanError::ConflictingReply(id.finding)),
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn nonempty(body: String) -> Result<String, PlanError> {
    if crate::ports::contains_reserved_marker(&body) {
        Err(PlanError::ReservedMarker)
    } else if body.trim().is_empty() {
        Err(PlanError::EmptyBody)
    } else {
        Ok(body)
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn same_finding(left: &Finding, right: &Finding) -> bool {
    left.id() == right.id()
        && left.policy_id() == right.policy_id()
        && left.status() == right.status()
        && left.severity() == right.severity()
        && left.disposition() == right.disposition()
        && left.validated() == right.validated()
        && left.location() == right.location()
        && same_suggestion(left.suggestion(), right.suggestion())
}

#[review_kit_observation::operations]
#[helper]
fn same_suggestion(
    left: Option<&review_kit_domain::Suggestion>,
    right: Option<&review_kit_domain::Suggestion>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.location() == right.location()
                && left.start_line() == right.start_line()
                && left.original() == right.original()
                && left.replacement() == right.replacement()
        }
        _ => false,
    }
}

#[review_kit_observation::operations]
#[helper]
fn same_verified_suggestion(
    left: Option<&review_kit_domain::Suggestion>,
    right: Option<&review_kit_domain::Suggestion>,
) -> bool {
    same_suggestion(left, right)
        && match (left, right) {
            (Some(left), Some(right)) => left.revision() == right.revision(),
            _ => true,
        }
}

#[review_kit_observation::operations]
#[helper]
fn same_communication(left: &Assessment, right: &Assessment) -> bool {
    left.subject() == right.subject()
        && left.decision() == right.decision()
        && left.depth() == right.depth()
        && left.scope() == right.scope()
        && left.limitation() == right.limitation()
        && left.reviewer() == right.reviewer()
        && left.revision() == right.revision()
        && left.selected_policies().len() == right.selected_policies().len()
        && left
            .selected_policies()
            .iter()
            .all(|p| right.selected_policies().contains(p))
        && left.evaluations().len() == right.evaluations().len()
        && left.evaluations().iter().all(|p| {
            right.evaluations().iter().any(|other| {
                p.policy_id() == other.policy_id()
                    && matches!(
                        (p.judgment(), other.judgment()),
                        (PolicyJudgment::Satisfied, PolicyJudgment::Satisfied)
                            | (
                                PolicyJudgment::Violated { .. },
                                PolicyJudgment::Violated { .. }
                            )
                            | (
                                PolicyJudgment::Blocked { .. },
                                PolicyJudgment::Blocked { .. }
                            )
                    )
            })
        })
        && left.findings().len() == right.findings().len()
        && left
            .findings()
            .iter()
            .all(|f| right.findings().iter().any(|other| same_finding(f, other)))
}
