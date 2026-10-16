#![doc = include_str!("README.md")]

use crate::observation as log;

use review_kit_domain::{
    Assessment, AssessmentDraft, AssessmentError, Finding, FindingId, FindingStatus,
    PolicyEvaluation, PolicyEvaluationError, PolicyId, PolicyJudgment, ReviewDecision, ReviewerId,
};

mod coverage;
pub use coverage::{AssessmentPolicies, InvalidAssessmentPolicies, PolicyCoverageIssue};

use super::discovery::{DiscoveryConflict, validate_snapshot};
use crate::{
    ports::{CommittedSource, PortError, ReviewRenderer},
    publication::Snapshot,
};

/// Editable judgments bound to retained review context and agent-declared policy coverage.
/// Carried findings and policies start unassessed; completing the template requires
/// explicit agent inputs even when the previous review used the same judgments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentTemplate {
    snapshot: Snapshot,
    reviewer: ReviewerId,
    pub policies: Vec<PolicySlot>,
    pub findings: Vec<FindingSlot>,
    coverage: AssessmentPolicies,
    pub decision: Option<ReviewDecision>,
    pub depth: Option<String>,
    pub scope: Option<String>,
    pub limitation: Option<String>,
    /// Local notes never enter the public domain assessment or renderer inputs.
    pub local_context: String,
}

#[review_kit_observation::operations]
impl AssessmentTemplate {
    #[helper]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    #[helper]
    pub fn reviewer(&self) -> &ReviewerId {
        &self.reviewer
    }
    #[helper]
    pub fn coverage(&self) -> &AssessmentPolicies {
        &self.coverage
    }
}

/// One selected policy awaiting an explicit agent evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySlot {
    pub policy_id: PolicyId,
    pub judgment: Option<PolicyJudgment>,
}

/// A carried finding's verified communicated judgment and its fresh reassessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingSlot {
    pub previous: Finding,
    /// Prior status and validation never substitute for a fresh reassessment.
    pub reassessment: Option<Finding>,
}

#[review_kit_observation::operations]
#[operation(name = "application.assessment.prepare", input = log::snapshot(&snapshot), output = |result| {
            log::Completion::result(
                result,
                |value| {
                    log::Fields::new()
                        .count("findings", value.findings.len())
                        .count("policies", value.policies.len())
                },
                log::discovery_conflict,
            )
        })]
/// Prepare unassessed policy and finding slots from retained review observations.
///
/// Finding context uses verified communication, including verified transitions in
/// unfinished rounds. Conflicting identities or finding history reject preparation.
/// This function performs no I/O and does not choose a decision or inherit judgments.
pub fn prepare(
    snapshot: Snapshot,
    reviewer: ReviewerId,
    coverage: AssessmentPolicies,
) -> Result<AssessmentTemplate, DiscoveryConflict> {
    validate_snapshot(&snapshot, &snapshot.subject, &reviewer)?;
    let findings = snapshot
        .findings
        .iter()
        .filter(|f| f.author == reviewer)
        .map(|root| {
            let previous = super::publishing::previous_finding(&snapshot, root, &reviewer)
                .map_err(|_| DiscoveryConflict::InvalidFindingState(root.finding.id()))?;
            Ok(FindingSlot {
                previous,
                reassessment: None,
            })
        })
        .collect::<Result<Vec<_>, DiscoveryConflict>>()?;
    let policies = coverage
        .ids()
        .into_iter()
        .map(|policy_id| PolicySlot {
            policy_id,
            judgment: None,
        })
        .collect();
    Ok(AssessmentTemplate {
        snapshot,
        reviewer,
        policies,
        findings,
        coverage,
        decision: None,
        depth: None,
        scope: None,
        limitation: None,
        local_context: String::new(),
    })
}

/// A missing agent input or inconsistency that prevents draft completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftIssue {
    MissingDecision,
    MissingDepth,
    MissingScope,
    Coverage(PolicyCoverageIssue),
    Discovery(DiscoveryConflict),
    MissingPolicyJudgment(PolicyId),
    InvalidPolicyJudgment {
        policy: PolicyId,
        error: PolicyEvaluationError,
    },
    MissingFindingReassessment(FindingId),
    FindingIdMismatch {
        expected: FindingId,
        actual: FindingId,
    },
    Domain(AssessmentError),
}

#[review_kit_observation::operations]
#[operation(name = "application.assessment.complete", input = log::Fields::new()
            .count("new_findings", new_findings.len())
            .count("carried_findings", template.findings.len()), output = |result| {
            log::Completion::result(result, log::assessment, |errors| log::draft_issues(errors))
        })]
/// Validate explicit agent judgments and assemble a completed domain assessment.
///
/// The caller fills every carried finding, selected policy, decision, depth, and
/// scope input. The agent's decision is retained when valid; it is never derived.
/// Missing slots and independent domain errors are returned together without a
/// partial assessment. Completion performs no source checks or provider writes.
pub fn complete(
    template: AssessmentTemplate,
    new_findings: Vec<Finding>,
) -> Result<Assessment, Vec<DraftIssue>> {
    let mut issues = Vec::new();
    let missing_decision = template.decision.is_none();
    let missing_depth = template.depth.is_none();
    let missing_scope = template.scope.is_none();
    if template.decision.is_none() {
        issues.push(DraftIssue::MissingDecision);
    }
    if template.depth.is_none() {
        issues.push(DraftIssue::MissingDepth);
    }
    if template.scope.is_none() {
        issues.push(DraftIssue::MissingScope);
    }
    if let Err(error) = validate_snapshot(
        &template.snapshot,
        &template.snapshot.subject,
        &template.reviewer,
    ) {
        issues.push(DraftIssue::Discovery(error));
    }
    for root in template
        .snapshot
        .findings
        .iter()
        .filter(|root| root.author == template.reviewer)
    {
        if !template
            .findings
            .iter()
            .any(|slot| slot.previous.id() == root.finding.id())
        {
            issues.push(DraftIssue::MissingFindingReassessment(root.finding.id()));
        }
    }
    let selected_policies: Vec<_> = template
        .policies
        .iter()
        .map(|slot| slot.policy_id.clone())
        .collect();
    issues.extend(
        template
            .coverage
            .validate_ids(&selected_policies)
            .into_iter()
            .map(DraftIssue::Coverage),
    );
    let mut evaluations = Vec::new();
    for slot in template.policies {
        match slot.judgment {
            None => issues.push(DraftIssue::MissingPolicyJudgment(slot.policy_id)),
            Some(judgment) => match PolicyEvaluation::new(slot.policy_id.clone(), judgment) {
                Ok(evaluation) => evaluations.push(evaluation),
                Err(error) => issues.push(DraftIssue::InvalidPolicyJudgment {
                    policy: slot.policy_id,
                    error,
                }),
            },
        }
    }
    let mut findings = Vec::new();
    for slot in template.findings {
        match slot.reassessment {
            None => issues.push(DraftIssue::MissingFindingReassessment(slot.previous.id())),
            Some(finding) => {
                if finding.id() != slot.previous.id() {
                    issues.push(DraftIssue::FindingIdMismatch {
                        expected: slot.previous.id(),
                        actual: finding.id(),
                    });
                }
                findings.push(finding);
            }
        }
    }
    findings.extend(new_findings);
    let assessment = Assessment::new(AssessmentDraft {
        subject: template.snapshot.subject,
        reviewer: template.reviewer,
        revision: template.snapshot.revision,
        selected_policies,
        evaluations,
        findings,
        decision: template.decision.unwrap_or(ReviewDecision::Blocked),
        depth: template.depth.unwrap_or_default(),
        scope: template.scope.unwrap_or_default(),
        limitation: template.limitation,
    });
    match assessment {
        Ok(assessment) if issues.is_empty() => Ok(assessment),
        Ok(_) => Err(issues),
        Err(errors) => {
            issues.extend(
                errors
                    .into_iter()
                    .filter(|error| {
                        !matches!(error,
                            AssessmentError::MissingBlockedLimitation if missing_decision
                        ) && !matches!(error, AssessmentError::BlankDepth if missing_depth)
                            && !matches!(error, AssessmentError::BlankScope if missing_scope)
                    })
                    .map(DraftIssue::Domain),
            );
            Err(issues)
        }
    }
}

/// A source, position, rendering, or selection failure identified during checking.
#[derive(Debug)]
pub struct CheckIssue {
    pub finding: Option<FindingId>,
    pub problem: CheckProblem,
}

#[derive(Debug)]
pub enum CheckProblem {
    Coverage(PolicyCoverageIssue),
    Source(PortError),
    Position(PortError),
    LineOutsideSource,
    SuggestionMismatch,
    Render(PortError),
    EmptyBody,
    ReservedMarker,
}

/// Rendered open-finding bodies whose source and position checks succeeded.
/// This result does not establish summary validity or publication authority.
#[derive(Debug)]
pub struct CheckedAssessment {
    pub finding_bodies: Vec<(FindingId, String)>,
}

/// Check agent-declared coverage and every open finding's source, position, and rendered body.
///
/// Suggestion originals must match exact committed lines at the reviewed revision.
/// Independent failures are collected across findings; closed findings are skipped.
/// Source and renderer adapters can perform I/O. No provider writes occur.
/// Full reply, summary, and retained-history checks belong to `publishing::check_review`.
#[review_kit_observation::operations]
#[helper]
pub fn check(
    assessment: &Assessment,
    selected: &AssessmentPolicies,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
) -> Result<CheckedAssessment, Vec<CheckIssue>> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);

    check_with_optional_context(assessment, selected, &mut source, &mut renderer, None)
}

/// Check finding source and rendering with the host's explicit public round context.
#[review_kit_observation::operations]
#[helper]
pub fn check_with_render_context(
    assessment: &Assessment,
    selected: &AssessmentPolicies,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
    context: &crate::ports::PublicRenderContext,
) -> Result<CheckedAssessment, Vec<CheckIssue>> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);

    check_with_optional_context(
        assessment,
        selected,
        &mut source,
        &mut renderer,
        Some(context),
    )
}

#[review_kit_observation::operations]
#[operation(name = "application.assessment.check", input = log::assessment(assessment), output = |result| {
            log::Completion::result(
                result,
                |value| log::Fields::new().count("bodies", value.finding_bodies.len()),
                |errors| log::check_issues(errors),
            )
        })]
fn check_with_optional_context(
    assessment: &Assessment,
    selected: &AssessmentPolicies,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    context: Option<&crate::ports::PublicRenderContext>,
) -> Result<CheckedAssessment, Vec<CheckIssue>> {
    let findings: Vec<_> = assessment
        .findings()
        .iter()
        .filter(|finding| finding.status() == FindingStatus::Open)
        .collect();
    let mut issues: Vec<_> = selected
        .validate(assessment)
        .into_iter()
        .map(|issue| CheckIssue {
            finding: None,
            problem: CheckProblem::Coverage(issue),
        })
        .collect();
    let names: Vec<_> = selected
        .policies()
        .iter()
        .map(|policy| crate::publication::PolicyName {
            id: policy.id.clone(),
            name: policy.name.clone(),
        })
        .collect();
    let checked = check_finding_inputs(
        assessment,
        &findings,
        source,
        renderer,
        context.map(|context| (context, names.as_slice())),
    );
    match checked {
        Ok(checked) if issues.is_empty() => Ok(checked),
        Ok(_) => Err(issues),
        Err(finding_issues) => {
            issues.extend(finding_issues);
            Err(issues)
        }
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn check_findings(
    assessment: &Assessment,
    findings: &[&Finding],
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    context: &crate::ports::PublicRenderContext,
    names: &[crate::publication::PolicyName],
) -> Result<CheckedAssessment, Vec<CheckIssue>> {
    check_finding_inputs(
        assessment,
        findings,
        source,
        renderer,
        Some((context, names)),
    )
}

#[review_kit_observation::operations]
#[operation(name = "application.validation.findings", input = log::Fields::new().count("findings", findings.len()), output = |result| {
            log::Completion::result(
                result,
                |value| log::Fields::new().count("bodies", value.finding_bodies.len()),
                |errors| log::check_issues(errors),
            )
        })]
fn check_finding_inputs(
    assessment: &Assessment,
    findings: &[&Finding],
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    context: Option<(
        &crate::ports::PublicRenderContext,
        &[crate::publication::PolicyName],
    )>,
) -> Result<CheckedAssessment, Vec<CheckIssue>> {
    let mut issues = Vec::new();
    let mut bodies = Vec::new();
    for finding in findings {
        match source.lines(
            assessment.subject(),
            assessment.revision(),
            finding.location().path(),
        ) {
            Ok(lines) => {
                if finding.location().line().get() > lines.len() as u64 {
                    issues.push(CheckIssue {
                        finding: Some(finding.id()),
                        problem: CheckProblem::LineOutsideSource,
                    });
                }
                if let Some(suggestion) = finding.suggestion() {
                    let matches = usize::try_from(suggestion.start_line().get() - 1)
                        .ok()
                        .and_then(|start| {
                            start
                                .checked_add(suggestion.original().len())
                                .and_then(|end| lines.get(start..end))
                        })
                        .is_some_and(|actual| {
                            suggestion
                                .verify_original(
                                    assessment.revision(),
                                    finding.location().path(),
                                    actual,
                                )
                                .is_ok()
                        });
                    if !matches {
                        issues.push(CheckIssue {
                            finding: Some(finding.id()),
                            problem: CheckProblem::SuggestionMismatch,
                        });
                    }
                }
            }
            Err(e) => issues.push(CheckIssue {
                finding: Some(finding.id()),
                problem: CheckProblem::Source(e),
            }),
        }
        if let Err(e) =
            source.validate_position(assessment.subject(), assessment.revision(), finding)
        {
            issues.push(CheckIssue {
                finding: Some(finding.id()),
                problem: CheckProblem::Position(e),
            });
        }
        let rendered = if let Some((context, names)) = context {
            names
                .iter()
                .find(|policy| &policy.id == finding.policy_id())
                .ok_or_else(|| {
                    PortError::new(
                        "render finding",
                        crate::use_cases::publishing::PlanError::MissingPolicyName(
                            finding.policy_id().clone(),
                        ),
                    )
                })
                .and_then(|policy| {
                    renderer.finding_context(crate::ports::FindingRenderInput {
                        finding,
                        policy_name: &policy.name,
                        context,
                    })
                })
        } else {
            renderer.finding(finding)
        };
        match rendered {
            Ok(body) if crate::ports::contains_reserved_marker(&body) => issues.push(CheckIssue {
                finding: Some(finding.id()),
                problem: CheckProblem::ReservedMarker,
            }),
            Ok(body) if !body.trim().is_empty() => bodies.push((finding.id(), body)),
            Ok(_) => issues.push(CheckIssue {
                finding: Some(finding.id()),
                problem: CheckProblem::EmptyBody,
            }),
            Err(e) => issues.push(CheckIssue {
                finding: Some(finding.id()),
                problem: CheckProblem::Render(e),
            }),
        }
    }
    if issues.is_empty() {
        Ok(CheckedAssessment {
            finding_bodies: bodies,
        })
    } else {
        Err(issues)
    }
}
