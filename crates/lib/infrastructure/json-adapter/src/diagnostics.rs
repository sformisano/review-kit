use std::error::Error;

use review_kit_application::use_cases::{
    assessment::{CheckProblem, DraftIssue, PolicyCoverageIssue},
    publishing::{BindingError, PlanError, Stop},
};
use review_kit_domain::{AssessmentError, FindingId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub finding: Option<u64>,
    pub policy: Option<String>,
    pub input: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            finding: None,
            policy: None,
            input: None,
            message: message.into(),
        }
    }
    pub fn finding(code: &str, id: FindingId, message: &str) -> Self {
        Self {
            finding: Some(id.get()),
            ..Self::new(code, message)
        }
    }
    pub fn input(code: &str, input: String, finding: Option<u64>, message: &str) -> Self {
        Self {
            input: Some(input),
            finding,
            ..Self::new(code, message)
        }
    }
    pub fn from_draft_issue(issue: &DraftIssue) -> Self {
        match issue {
            DraftIssue::MissingDecision => {
                Self::new("missing_decision", "Choose a review decision.")
            }
            DraftIssue::MissingDepth => Self::new("missing_depth", "Describe the review depth."),
            DraftIssue::MissingScope => Self::new("missing_scope", "Describe the review scope."),
            DraftIssue::Coverage(issue) => selection_issue(issue),
            DraftIssue::Discovery(error) => Self::new("discovery_conflict", &error.to_string()),
            DraftIssue::MissingPolicyJudgment(id) => Self {
                policy: Some(id.as_str().into()),
                ..Self::new("missing_policy_judgment", "Assess this selected policy.")
            },
            DraftIssue::InvalidPolicyJudgment { policy, error } => Self {
                policy: Some(policy.as_str().into()),
                ..Self::new("invalid_policy_judgment", &error.to_string())
            },
            DraftIssue::MissingFindingReassessment(id) => Self::finding(
                "missing_finding_reassessment",
                *id,
                "Reassess this carried finding.",
            ),
            DraftIssue::FindingIdMismatch { expected, .. } => Self::finding(
                "finding_id_mismatch",
                *expected,
                "Reassessment must retain the carried finding ID.",
            ),
            DraftIssue::Domain(error) => {
                let (code, finding, policy) = match error {
                    AssessmentError::BlankDepth => ("blank_depth", None, None),
                    AssessmentError::BlankScope => ("blank_scope", None, None),
                    AssessmentError::BlankLimitation => ("blank_limitation", None, None),
                    AssessmentError::MissingBlockedLimitation => {
                        ("missing_blocked_limitation", None, None)
                    }
                    AssessmentError::IncompleteDecision(_) => ("incomplete_decision", None, None),
                    AssessmentError::ReadyWithViolation(id) => {
                        ("ready_with_violation", None, Some(id.as_str().into()))
                    }
                    AssessmentError::ReadyWithBlockingFinding(id) => {
                        ("ready_with_blocking_finding", Some(id.get()), None)
                    }
                    AssessmentError::DuplicateSelectedPolicy(id) => {
                        ("duplicate_selected_policy", None, Some(id.as_str().into()))
                    }
                    AssessmentError::UnselectedEvaluation(id) => {
                        ("unselected_evaluation", None, Some(id.as_str().into()))
                    }
                    AssessmentError::DuplicateEvaluation(id) => {
                        ("duplicate_evaluation", None, Some(id.as_str().into()))
                    }
                    AssessmentError::MissingEvaluation(id) => {
                        ("missing_evaluation", None, Some(id.as_str().into()))
                    }
                    AssessmentError::DuplicateFinding(id) => {
                        ("duplicate_finding", Some(id.get()), None)
                    }
                    AssessmentError::UnselectedFindingPolicy(id) => {
                        ("unselected_finding_policy", Some(id.get()), None)
                    }
                    AssessmentError::SuggestionRevisionMismatch(id) => {
                        ("suggestion_revision_mismatch", Some(id.get()), None)
                    }
                };
                Self {
                    finding,
                    policy,
                    ..Self::new(code, &error.to_string())
                }
            }
        }
    }
    pub fn from_plan_error(error: &PlanError) -> Vec<Self> {
        match error {
            PlanError::ShapeChecks(errors) | PlanError::ReplyChecks(errors) => {
                errors.iter().flat_map(Self::from_plan_error).collect()
            }
            PlanError::Coverage(issues) => issues.iter().map(selection_issue).collect(),
            PlanError::Checks(issues) => issues
                .iter()
                .map(|issue| {
                    let (code, message) = match &issue.problem {
                        CheckProblem::Coverage(issue) => return selection_issue(issue),
                        CheckProblem::Source(error) => ("source_unavailable", error_chain(error)),
                        CheckProblem::Position(error) => {
                            ("unsupported_position", error_chain(error))
                        }
                        CheckProblem::LineOutsideSource => (
                            "line_outside_source",
                            "Finding anchor is outside the committed source.".into(),
                        ),
                        CheckProblem::SuggestionMismatch => (
                            "suggestion_mismatch",
                            "Suggestion original differs from committed source.".into(),
                        ),
                        CheckProblem::Render(error) => ("invalid_rendering", error_chain(error)),
                        CheckProblem::EmptyBody => ("empty_body", "Rendered body is blank.".into()),
                        CheckProblem::ReservedMarker => (
                            "reserved_marker",
                            "Rendered body contains a reserved marker.".into(),
                        ),
                    };
                    Self {
                        finding: issue.finding.map(FindingId::get),
                        ..Self::new(code, &message)
                    }
                })
                .collect(),
            _ => {
                let (code, finding) = match error {
                    PlanError::Evidence(_) => ("evidence_unavailable", None),
                    PlanError::Discovery(_) => ("discovery_failed", None),
                    PlanError::ChangedRevision => ("changed_revision", None),
                    PlanError::UncomparedWrite => ("uncompared_write", None),
                    PlanError::WrongReviewer => ("wrong_reviewer", None),
                    PlanError::ProgressBinding => ("progress_binding", None),
                    PlanError::MissingTransition(id) => ("missing_transition", Some(*id)),
                    PlanError::InvalidTransition(id) => ("invalid_transition", Some(*id)),
                    PlanError::DuplicateTransition(id) => ("duplicate_transition", Some(*id)),
                    PlanError::MissingCarriedFinding(id) => ("missing_carried_finding", Some(*id)),
                    PlanError::UnselectedHistoricalPolicy(id) => {
                        ("unselected_historical_policy", Some(*id))
                    }
                    PlanError::MissingPublishedFinding(id) => {
                        ("missing_published_finding", Some(*id))
                    }
                    PlanError::ReusedFindingId(id) => ("reused_finding_id", Some(*id)),
                    PlanError::NewFindingNotOpen(id) => ("new_finding_not_open", Some(*id)),
                    PlanError::ColocatedFinding(id) => ("colocated_finding", Some(*id)),
                    PlanError::InvalidAnswer(id) => ("invalid_answer", Some(*id)),
                    PlanError::DuplicateAnswer => ("duplicate_answer", None),
                    PlanError::RoundExhausted => ("round_exhausted", None),
                    PlanError::ChangedPendingAssessment => ("changed_pending_assessment", None),
                    PlanError::InconsistentRound => ("inconsistent_round", None),
                    PlanError::ConflictingReply(id) => ("conflicting_reply", Some(*id)),
                    PlanError::ConflictingFinding(id) => ("conflicting_finding", Some(*id)),
                    PlanError::Render(_) => ("invalid_rendering", None),
                    PlanError::EmptyBody => ("empty_body", None),
                    PlanError::ReservedMarker => ("reserved_marker", None),
                    PlanError::NotAbandonable => ("not_abandonable", None),
                    PlanError::InvalidAbandonmentReason => ("invalid_abandonment_reason", None),
                    PlanError::ChangedRenderProfile => ("render_profile_changed", None),
                    PlanError::UnknownPendingRenderProfile => {
                        ("pending_render_profile_unavailable", None)
                    }
                    PlanError::InvalidWorkflow => ("invalid_review_workflow", None),
                    PlanError::PolicyDeclarations(_) => ("invalid_policy_declarations", None),
                    PlanError::ChangedReviewIdentity => ("review_identity_changed", None),
                    PlanError::MissingPolicyName(_) => ("policy_display_name_unavailable", None),
                    PlanError::Binding(error) => (binding_code(error), None),
                    PlanError::ShapeChecks(_)
                    | PlanError::ReplyChecks(_)
                    | PlanError::Coverage(_)
                    | PlanError::Checks(_) => unreachable!(),
                };
                vec![Self {
                    finding: finding.map(FindingId::get),
                    ..Self::new(code, &error_chain(error))
                }]
            }
        }
    }
    pub fn from_stop(stop: &Stop) -> Vec<Self> {
        let (code, message) = match stop {
            Stop::UnresolvedWrite => (
                "unresolved_write",
                "Retained write outcome is unresolved.".into(),
            ),
            Stop::Evidence(error) => ("evidence_unavailable", error_chain(error)),
            Stop::Authority(effect) => (
                "authority_denied",
                format!("Effect {effect:?} is not authorized."),
            ),
            Stop::RevisionChanged => (
                "changed_revision",
                "Reviewed revision is no longer current.".into(),
            ),
            Stop::ConcurrentChange => (
                "concurrent_change",
                "Observed owned review state changed.".into(),
            ),
            Stop::Read(error) => ("provider_unavailable", error_chain(error)),
            Stop::Render(error) | Stop::Preparation(error) => return Self::from_plan_error(error),
            Stop::Rejected(error) => ("write_rejected", error_chain(error)),
            Stop::Unknown(error) => ("write_unknown", error_chain(error)),
            Stop::ReadbackMismatch => (
                "readback_mismatch",
                "Observed effect does not match the retained write.".into(),
            ),
            Stop::Conflict => ("conflict", "Observed review records conflict.".into()),
        };
        vec![Self::new(code, &message)]
    }
}

fn selection_issue(issue: &PolicyCoverageIssue) -> Diagnostic {
    match issue {
        PolicyCoverageIssue::Missing(id) => Diagnostic {
            policy: Some(id.as_str().into()),
            ..Diagnostic::new(
                "missing_selected_policy",
                "Required selected policy is missing.",
            )
        },
        PolicyCoverageIssue::Unexpected(id) => Diagnostic {
            policy: Some(id.as_str().into()),
            ..Diagnostic::new(
                "unexpected_policy",
                "Policy is outside trusted selected coverage.",
            )
        },
    }
}

/// Stable codes for journal binding refusals.
fn binding_code(error: &BindingError) -> &'static str {
    match error {
        BindingError::PendingOwner(_) => "publication_pending_owner",
        BindingError::Mismatch(_) => "binding_mismatch",
        BindingError::AlreadySettled(_) => "already_settled",
        BindingError::SettlementConflict(_) => "settlement_conflict",
        BindingError::BindingRequired => "binding_required",
        BindingError::Unresolved => "unresolved_write",
        BindingError::MissingReason => "invalid_abandonment_reason",
    }
}

pub fn error_chain(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

impl crate::JsonError {
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            Self::RetainedStateLimit => {
                vec![Diagnostic::new("retained_state_limit", &self.to_string())]
            }
            Self::Draft(diagnostics) => diagnostics.clone(),
            Self::Syntax(_) => vec![Diagnostic::new("invalid_json", &error_chain(self))],
            Self::Version(_) => vec![Diagnostic::new("unsupported_version", &self.to_string())],
            Self::ContextBinding => vec![Diagnostic::new("context_binding", &self.to_string())],
            Self::Policies(_) => vec![Diagnostic::new(
                "invalid_policy_coverage",
                &error_chain(self),
            )],
            Self::Discovery(_) | Self::Preparation(_) => {
                vec![Diagnostic::new("discovery_conflict", &error_chain(self))]
            }
            Self::InvalidValue => vec![Diagnostic::new("invalid_value", &self.to_string())],
            Self::Finding(_) => vec![Diagnostic::new("invalid_finding", &error_chain(self))],
        }
    }
}
