use std::collections::HashSet;

use review_kit_domain::{Finding, FindingId, FindingStatus, PolicyId, PolicyJudgment, Revision};

use super::{Successor, SuccessorCheck};
use crate::review_run::{ContentRef, RunId};

/// Whether a declared policy receives a judgment in this review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApplicabilityStatus {
    Applicable,
    Excluded,
    Unresolved,
}

/// Applicability and its judgment form one declared alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyOutcome {
    Excluded,
    Unresolved { limitation: String },
    Satisfied,
    Violated { evidence: String },
    Blocked { limitation: String },
}

#[review_kit_observation::operations]
impl PolicyOutcome {
    #[helper]
    pub fn status(&self) -> ApplicabilityStatus {
        match self {
            Self::Excluded => ApplicabilityStatus::Excluded,
            Self::Unresolved { .. } => ApplicabilityStatus::Unresolved,
            _ => ApplicabilityStatus::Applicable,
        }
    }
    #[helper]
    pub fn judgment(&self) -> Option<PolicyJudgment> {
        match self {
            Self::Excluded => None,
            Self::Satisfied => Some(PolicyJudgment::Satisfied),
            Self::Violated { evidence } => Some(PolicyJudgment::Violated {
                evidence: evidence.clone(),
            }),
            Self::Blocked { limitation } | Self::Unresolved { limitation } => {
                Some(PolicyJudgment::Blocked {
                    limitation: limitation.clone(),
                })
            }
        }
    }
}

/// Coverage and judgments authored together for one captured review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDeclarationsDraft {
    pub run_id: String,
    pub revision: Revision,
    pub policy_set: String,
    pub supersedes: Option<String>,
    pub reason: String,
    pub policies: Vec<ReviewPolicyDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPolicyDraft {
    pub policy_id: String,
    pub name: String,
    pub required: bool,
    pub reason: String,
    pub outcome: Option<PolicyOutcome>,
}

/// Retained identity and current successor authority for a declaration.
#[derive(Debug, Clone, Copy)]
pub struct ReviewBinding<'a> {
    pub run_id: &'a RunId,
    pub revision: &'a Revision,
    pub carried_findings: &'a [Finding],
    pub successor: Successor<'a>,
}

/// Checked declared coverage; this does not establish discovery completeness or truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDeclarations {
    policy_set: String,
    policies: Vec<ReviewPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPolicy {
    policy_id: PolicyId,
    name: String,
    reason: String,
    outcome: PolicyOutcome,
}

#[review_kit_observation::operations]
impl ReviewDeclarations {
    #[helper]
    pub fn policy_set(&self) -> &str {
        &self.policy_set
    }
    #[helper]
    pub fn policies(&self) -> &[ReviewPolicy] {
        &self.policies
    }
}

#[review_kit_observation::operations]
impl ReviewPolicy {
    #[helper]
    pub fn policy_id(&self) -> &PolicyId {
        &self.policy_id
    }
    #[helper]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[helper]
    pub fn status(&self) -> ApplicabilityStatus {
        self.outcome.status()
    }
    #[helper]
    pub fn reason(&self) -> &str {
        &self.reason
    }
    #[helper]
    pub fn judgment(&self) -> Option<PolicyJudgment> {
        self.outcome.judgment()
    }
}

/// A consistency failure in the complete declaration.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display)]
pub enum ReviewIssue {
    /// review belongs to run {actual}, not this run
    RunBinding { actual: String },
    /// reviewed revision differs from captured source
    RevisionBinding,
    /// policy-set name must not be blank
    BlankPolicySet,
    /// review reason must not be blank
    BlankReason,
    /// review must declare at least one policy
    NoPolicies,
    /// policy row {0} has a blank policy ID
    BlankPolicyId(usize),
    /// policy {0} appears more than once
    DuplicatePolicy(PolicyId),
    /// policy {0} has a blank name
    BlankPolicyName(PolicyId),
    /// supersedes must name the current accepted review reference
    StaleSuccessor {
        expected: Option<ContentRef>,
        actual: Option<String>,
    },
    /// supersedes {0} is not a content reference
    InvalidSupersedes(String),
    /// active carried finding {finding} uses omitted policy {policy}
    MissingActiveFindingPolicy {
        finding: FindingId,
        policy: PolicyId,
    },
    /// policy {0} has no outcome
    MissingOutcome(PolicyId),
    /// policy {0} needs a nonblank applicability reason
    BlankDecisionReason(PolicyId),
    /// required policy {0} cannot be excluded
    RequiredPolicyExcluded(PolicyId),
}

/// Check declared coverage, applicability and judgment combinations.
///
/// Active carried-finding policies must remain declared; assessment completion
/// decides whether exclusion closes them.
#[review_kit_observation::operations]
#[operation(
    name = "application.checkpoints.review",
    input = crate::observation::Fields::new().count("policies", draft.policies.len()),
    output = |result| crate::observation::Completion::result(result, |_| crate::observation::Fields::new(), |issues| crate::observation::ErrorInfo::new("ReviewIssues", "validation_failed").with_fields(crate::observation::Fields::new().count("count", issues.len()))),
)]
pub fn validate_declarations(
    draft: &ReviewDeclarationsDraft,
    binding: &ReviewBinding<'_>,
) -> Result<ReviewDeclarations, Vec<ReviewIssue>> {
    let mut issues = Vec::new();
    if draft.run_id != binding.run_id.as_str() {
        issues.push(ReviewIssue::RunBinding {
            actual: draft.run_id.clone(),
        });
    }
    if &draft.revision != binding.revision {
        issues.push(ReviewIssue::RevisionBinding);
    }
    if draft.policy_set.trim().is_empty() {
        issues.push(ReviewIssue::BlankPolicySet);
    }
    if draft.reason.trim().is_empty() {
        issues.push(ReviewIssue::BlankReason);
    }
    if draft.policies.is_empty() {
        issues.push(ReviewIssue::NoPolicies);
    }
    match binding.successor.check(draft.supersedes.as_deref()) {
        SuccessorCheck::Valid => {}
        SuccessorCheck::Stale(expected) => issues.push(ReviewIssue::StaleSuccessor {
            expected,
            actual: draft.supersedes.clone(),
        }),
        SuccessorCheck::Invalid => issues.push(ReviewIssue::InvalidSupersedes(
            draft.supersedes.clone().unwrap_or_default(),
        )),
    }
    let mut ids = HashSet::new();
    let mut policies = Vec::new();
    for (index, row) in draft.policies.iter().enumerate() {
        let Ok(id) = PolicyId::try_from(row.policy_id.clone()) else {
            issues.push(ReviewIssue::BlankPolicyId(index));
            continue;
        };
        if !ids.insert(id.clone()) {
            issues.push(ReviewIssue::DuplicatePolicy(id));
            continue;
        }
        if row.name.trim().is_empty() {
            issues.push(ReviewIssue::BlankPolicyName(id.clone()));
        }
        if row.reason.trim().is_empty() {
            issues.push(ReviewIssue::BlankDecisionReason(id.clone()));
        }
        let Some(outcome) = &row.outcome else {
            issues.push(ReviewIssue::MissingOutcome(id));
            continue;
        };
        let status = outcome.status();
        if row.required && status == ApplicabilityStatus::Excluded {
            issues.push(ReviewIssue::RequiredPolicyExcluded(id.clone()));
        }
        policies.push(ReviewPolicy {
            policy_id: id,
            name: row.name.clone(),
            reason: row.reason.clone(),
            outcome: outcome.clone(),
        });
    }
    for finding in binding
        .carried_findings
        .iter()
        .filter(|f| matches!(f.status(), FindingStatus::Open | FindingStatus::Uncertain))
    {
        if !ids.contains(finding.policy_id()) {
            issues.push(ReviewIssue::MissingActiveFindingPolicy {
                finding: finding.id(),
                policy: finding.policy_id().clone(),
            });
        }
    }
    if issues.is_empty() {
        Ok(ReviewDeclarations {
            policy_set: draft.policy_set.clone(),
            policies,
        })
    } else {
        Err(issues)
    }
}
