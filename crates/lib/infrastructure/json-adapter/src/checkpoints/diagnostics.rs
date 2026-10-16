use review_kit_application::use_cases::checkpoints::{ConfirmationIssue, ReviewIssue};
use review_kit_domain::{FindingId, PolicyId};
use review_kit_observation::{ErrorInfo, Fields};

use crate::Diagnostic;

fn sentence(text: impl std::fmt::Display) -> String {
    let text = text.to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => text,
    }
}

fn diagnostic(
    code: &str,
    issue: impl std::fmt::Display,
    policy: Option<&PolicyId>,
    finding: Option<FindingId>,
    input: Option<String>,
) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        finding: finding.map(FindingId::get),
        policy: policy.map(|policy| policy.as_str().into()),
        input,
        message: sentence(issue),
    }
}

fn row(index: usize) -> Option<String> {
    Some(format!("policies[{index}].policy_id"))
}

/// The JSON could not be decoded into the expected document; `input` is `line:column`.
pub(crate) fn malformed(error: &serde_json::Error) -> Diagnostic {
    Diagnostic {
        code: "malformed_draft".into(),
        finding: None,
        policy: None,
        input: Some(format!("{}:{}", error.line(), error.column())),
        message: format!("Draft does not decode as the expected document: {error}."),
    }
}

pub(crate) fn unsupported_version(version: u64) -> Diagnostic {
    Diagnostic::new(
        "unsupported_version",
        &format!("Document version {version} is not supported."),
    )
}

pub(crate) fn review(issue: &ReviewIssue) -> Diagnostic {
    use ReviewIssue as Issue;
    let (code, policy, finding, input) = match issue {
        Issue::RunBinding { .. } => ("run_binding", None, None, None),
        Issue::RevisionBinding => ("revision_mismatch", None, None, None),
        Issue::BlankPolicySet | Issue::NoPolicies => {
            ("incomplete_policy_declaration", None, None, None)
        }
        Issue::BlankReason => ("blank_reason", None, None, None),
        Issue::BlankPolicyId(index) => ("invalid_policy_id", None, None, row(*index)),
        Issue::DuplicatePolicy(policy) => ("duplicate_policy", Some(policy), None, None),
        Issue::BlankPolicyName(policy) => {
            ("incomplete_policy_declaration", Some(policy), None, None)
        }
        Issue::StaleSuccessor { .. } | Issue::InvalidSupersedes(_) => {
            ("stale_successor", None, None, None)
        }
        Issue::MissingActiveFindingPolicy { finding, policy } => (
            "missing_active_finding_policy",
            Some(policy),
            Some(*finding),
            None,
        ),
        Issue::MissingOutcome(policy) | Issue::BlankDecisionReason(policy) => {
            ("incomplete_applicability", Some(policy), None, None)
        }
        Issue::RequiredPolicyExcluded(policy) => {
            ("required_policy_excluded", Some(policy), None, None)
        }
    };
    diagnostic(code, issue, policy, finding, input)
}

pub(crate) fn confirmation(issue: &ConfirmationIssue) -> Diagnostic {
    use ConfirmationIssue as Issue;
    let (code, finding) = match issue {
        Issue::UnknownFinding(finding) => ("unknown_confirmation_finding", finding),
        Issue::DuplicateConfirmation(finding) => ("duplicate_confirmation", finding),
        Issue::BlankBasis(finding) => ("blank_confirmation_basis", finding),
        Issue::StaleConfirmation(finding) => ("stale_confirmation", finding),
        Issue::InvalidRetainedConfirmation(finding) => ("invalid_retained_confirmation", finding),
        Issue::UnconfirmedBlockingFinding(finding) => ("unconfirmed_blocking_finding", finding),
    };
    diagnostic(code, issue, None, Some(*finding), None)
}

pub(crate) fn diagnostics_error(diagnostics: &[Diagnostic]) -> ErrorInfo {
    ErrorInfo::new("CheckpointDiagnostics", "validation_failed")
        .with_fields(Fields::new().count("diagnostics", diagnostics.len()))
}
