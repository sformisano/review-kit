use super::{ErrorInfo, Fields};
use crate::{
    ports::PortError,
    use_cases::{
        assessment::{CheckIssue, CheckProblem, DraftIssue, PolicyCoverageIssue},
        discovery::{DiscoveryConflict, DiscoveryError},
        discovery_bundle::BundleError,
        publishing::{BindingError, PlanError, Stop},
    },
};

pub(crate) fn port_error(_: &PortError) -> ErrorInfo {
    // Third-party ports control both the operation label and the entire cause chain.
    ErrorInfo::new("PortError", "adapter_failure").caused_by(ErrorInfo::opaque("source"))
}

pub(crate) fn discovery_conflict(value: &DiscoveryConflict) -> ErrorInfo {
    let kind = match value {
        DiscoveryConflict::WrongSubject => "wrong_subject",
        DiscoveryConflict::WrongReviewer => "wrong_reviewer",
        DiscoveryConflict::InvalidFindingState(_) => "invalid_finding_state",
        DiscoveryConflict::MissingPublishedFinding(_) => "missing_published_finding",
        DiscoveryConflict::ConflictingReply(_) => "conflicting_reply",
        DiscoveryConflict::MultipleSummaries => "multiple_summaries",
        DiscoveryConflict::DuplicateFinding(_) => "duplicate_finding",
        DiscoveryConflict::DuplicateProviderId => "duplicate_provider_id",
        DiscoveryConflict::InvalidRound => "invalid_round",
        DiscoveryConflict::ForeignRound => "foreign_round",
    };
    ErrorInfo::new("DiscoveryConflict", kind)
}

pub(crate) fn discovery_error(value: &DiscoveryError) -> ErrorInfo {
    match value {
        DiscoveryError::Read(error) => {
            ErrorInfo::new("DiscoveryError", "read").caused_by(port_error(error))
        }
        DiscoveryError::Conflict(error) => {
            ErrorInfo::new("DiscoveryError", "conflict").caused_by(discovery_conflict(error))
        }
    }
}

pub(crate) fn bundle_error(value: &BundleError) -> ErrorInfo {
    match value {
        BundleError::Discovery(error) => {
            ErrorInfo::new("BundleError", "discovery").caused_by(discovery_error(error))
        }
        BundleError::Changes(error) => {
            ErrorInfo::new("BundleError", "changes").caused_by(port_error(error))
        }
        BundleError::FindingState(error) => {
            ErrorInfo::new("BundleError", "finding_state").caused_by(plan_error(error))
        }
        BundleError::WrongSubject => ErrorInfo::new("BundleError", "wrong_subject"),
        BundleError::ChangedRevision => ErrorInfo::new("BundleError", "changed_revision"),
        BundleError::FindingIdExhausted => ErrorInfo::new("BundleError", "finding_id_exhausted"),
    }
}

pub(crate) fn plan_errors(errors: &[PlanError]) -> ErrorInfo {
    ErrorInfo::new("PlanErrors", "validation_failed").with_fields(
        Fields::new().count("count", errors.len()).objects(
            "issues",
            errors
                .iter()
                .map(|error| Fields::new().error("error", plan_error(error))),
        ),
    )
}

pub(crate) fn check_issues(errors: &[CheckIssue]) -> ErrorInfo {
    ErrorInfo::new("CheckIssues", "validation_failed").with_fields(
        Fields::new().count("count", errors.len()).objects(
            "issues",
            errors.iter().map(|issue| {
                let error = match &issue.problem {
                    CheckProblem::Source(error) => {
                        ErrorInfo::new("CheckProblem", "source").caused_by(port_error(error))
                    }
                    CheckProblem::Position(error) => {
                        ErrorInfo::new("CheckProblem", "position").caused_by(port_error(error))
                    }
                    CheckProblem::Render(error) => {
                        ErrorInfo::new("CheckProblem", "render").caused_by(port_error(error))
                    }
                    CheckProblem::Coverage(_) => ErrorInfo::new("CheckProblem", "selection"),
                    CheckProblem::LineOutsideSource => {
                        ErrorInfo::new("CheckProblem", "line_outside_source")
                    }
                    CheckProblem::SuggestionMismatch => {
                        ErrorInfo::new("CheckProblem", "suggestion_mismatch")
                    }
                    CheckProblem::EmptyBody => ErrorInfo::new("CheckProblem", "empty_body"),
                    CheckProblem::ReservedMarker => {
                        ErrorInfo::new("CheckProblem", "reserved_marker")
                    }
                };
                Fields::new().error("error", error)
            }),
        ),
    )
}

pub(crate) fn draft_issues(errors: &[DraftIssue]) -> ErrorInfo {
    ErrorInfo::new("DraftIssues", "validation_failed").with_fields(
        Fields::new().count("count", errors.len()).labels(
            "categories",
            errors.iter().map(|issue| match issue {
                DraftIssue::MissingDecision => "missing_decision",
                DraftIssue::MissingDepth => "missing_depth",
                DraftIssue::MissingScope => "missing_scope",
                DraftIssue::Coverage(_) => "selection",
                DraftIssue::Discovery(_) => "discovery",
                DraftIssue::MissingPolicyJudgment(_) => "missing_policy_judgment",
                DraftIssue::InvalidPolicyJudgment { .. } => "invalid_policy_judgment",
                DraftIssue::MissingFindingReassessment(_) => "missing_finding_reassessment",
                DraftIssue::FindingIdMismatch { .. } => "finding_id_mismatch",
                DraftIssue::Domain(_) => "domain",
            }),
        ),
    )
}

fn selection_issues(errors: &[PolicyCoverageIssue]) -> ErrorInfo {
    ErrorInfo::new("PolicyCoverageIssues", "validation_failed").with_fields(
        Fields::new().count("count", errors.len()).labels(
            "categories",
            errors.iter().map(|issue| match issue {
                PolicyCoverageIssue::Missing(_) => "missing",
                PolicyCoverageIssue::Unexpected(_) => "unexpected",
            }),
        ),
    )
}

pub(crate) fn plan_error(value: &PlanError) -> ErrorInfo {
    let kind = match value {
        PlanError::Evidence(error) => {
            return ErrorInfo::new("PlanError", "evidence").caused_by(port_error(error));
        }
        PlanError::Discovery(error) => {
            return ErrorInfo::new("PlanError", "discovery").caused_by(discovery_error(error));
        }
        PlanError::Render(error) => {
            return ErrorInfo::new("PlanError", "render").caused_by(port_error(error));
        }
        PlanError::ShapeChecks(errors) => {
            return ErrorInfo::new("PlanError", "shape_checks").caused_by(plan_errors(errors));
        }
        PlanError::ReplyChecks(errors) => {
            return ErrorInfo::new("PlanError", "reply_checks").caused_by(plan_errors(errors));
        }
        PlanError::Coverage(errors) => return selection_issues(errors),
        PlanError::Checks(errors) => return check_issues(errors),
        PlanError::Binding(error) => {
            return ErrorInfo::new("PlanError", "binding").caused_by(binding_error(error));
        }
        PlanError::MissingTransition(_) => "missing_transition",
        PlanError::InvalidTransition(_) => "invalid_transition",
        PlanError::DuplicateTransition(_) => "duplicate_transition",
        PlanError::ChangedRevision => "changed_revision",
        PlanError::WrongReviewer => "wrong_reviewer",
        PlanError::ProgressBinding => "progress_binding",
        PlanError::MissingCarriedFinding(_) => "missing_carried_finding",
        PlanError::UnselectedHistoricalPolicy(_) => "unselected_historical_policy",
        PlanError::MissingPublishedFinding(_) => "missing_published_finding",
        PlanError::ReusedFindingId(_) => "reused_finding_id",
        PlanError::NewFindingNotOpen(_) => "new_finding_not_open",
        PlanError::ColocatedFinding(_) => "colocated_finding",
        PlanError::InvalidAnswer(_) => "invalid_answer",
        PlanError::DuplicateAnswer => "duplicate_answer",
        PlanError::RoundExhausted => "round_exhausted",
        PlanError::ChangedPendingAssessment => "changed_pending_assessment",
        PlanError::InconsistentRound => "inconsistent_round",
        PlanError::ConflictingReply(_) => "conflicting_reply",
        PlanError::ConflictingFinding(_) => "conflicting_finding",
        PlanError::EmptyBody => "empty_body",
        PlanError::ReservedMarker => "reserved_marker",
        PlanError::NotAbandonable => "not_abandonable",
        PlanError::InvalidAbandonmentReason => "invalid_abandonment_reason",
        PlanError::ChangedRenderProfile => "changed_render_profile",
        PlanError::UnknownPendingRenderProfile => "unknown_pending_render_profile",
        PlanError::InvalidWorkflow => "invalid_workflow",
        PlanError::PolicyDeclarations(_) => "invalid_policy_declarations",
        PlanError::MissingPolicyName(_) => "missing_policy_name",
        PlanError::ChangedReviewIdentity => "changed_review_identity",
    };
    ErrorInfo::new("PlanError", kind)
}

pub(crate) fn binding_error(value: &BindingError) -> ErrorInfo {
    let kind = match value {
        BindingError::PendingOwner(_) => "pending_owner",
        BindingError::Mismatch(_) => "mismatch",
        BindingError::AlreadySettled(_) => "already_settled",
        BindingError::SettlementConflict(_) => "settlement_conflict",
        BindingError::BindingRequired => "binding_required",
        BindingError::Unresolved => "unresolved",
        BindingError::MissingReason => "missing_reason",
    };
    ErrorInfo::new("BindingError", kind)
}

pub(crate) fn stop_error(value: &Stop) -> ErrorInfo {
    let kind = match value {
        Stop::Evidence(error) => {
            return ErrorInfo::new("Stop", "evidence").caused_by(port_error(error));
        }
        Stop::Read(error) => return ErrorInfo::new("Stop", "read").caused_by(port_error(error)),
        Stop::Rejected(error) => {
            return ErrorInfo::new("Stop", "rejected").caused_by(port_error(error));
        }
        Stop::Unknown(error) => {
            return ErrorInfo::new("Stop", "unknown").caused_by(port_error(error));
        }
        Stop::Render(error) => {
            return ErrorInfo::new("Stop", "render").caused_by(plan_error(error));
        }
        Stop::Preparation(error) => {
            return ErrorInfo::new("Stop", "preparation").caused_by(plan_error(error));
        }
        Stop::UnresolvedWrite => "unresolved_write",
        Stop::Authority(_) => "authority",
        Stop::RevisionChanged => "revision_changed",
        Stop::ConcurrentChange => "concurrent_change",
        Stop::ReadbackMismatch => "readback_mismatch",
        Stop::Conflict => "conflict",
    };
    ErrorInfo::new("Stop", kind)
}
