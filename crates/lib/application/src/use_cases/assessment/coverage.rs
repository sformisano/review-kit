use std::collections::HashSet;

use review_kit_domain::{Assessment, PolicyId};

use crate::publication::PolicyName;

/// Assessed policy slots and names used in public comments.
/// Public applicability is retained separately in each round's declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentPolicies {
    name: String,
    policies: Vec<PolicyName>,
}

#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum InvalidAssessmentPolicies {
    /// policy-set display name must not be blank
    BlankName,
    /// policy display name must not be blank: {0}
    BlankPolicyName(PolicyId),
    /// policy coverage repeats an identifier: {0}
    DuplicatePolicy(PolicyId),
}

#[review_kit_observation::operations]
impl AssessmentPolicies {
    #[helper]
    pub fn new(
        name: String,
        mut policies: Vec<PolicyName>,
    ) -> Result<Self, InvalidAssessmentPolicies> {
        if name.trim().is_empty() {
            return Err(InvalidAssessmentPolicies::BlankName);
        }
        let mut ids = HashSet::new();
        for policy in &policies {
            if policy.name.trim().is_empty() {
                return Err(InvalidAssessmentPolicies::BlankPolicyName(
                    policy.id.clone(),
                ));
            }
            if !ids.insert(&policy.id) {
                return Err(InvalidAssessmentPolicies::DuplicatePolicy(
                    policy.id.clone(),
                ));
            }
        }
        policies.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self { name, policies })
    }

    #[helper]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[helper]
    pub fn policies(&self) -> &[PolicyName] {
        &self.policies
    }

    #[helper]
    pub fn ids(&self) -> Vec<PolicyId> {
        self.policies
            .iter()
            .map(|policy| policy.id.clone())
            .collect()
    }

    #[helper]
    pub fn validate(&self, assessment: &Assessment) -> Vec<PolicyCoverageIssue> {
        self.validate_ids(assessment.selected_policies())
    }

    #[helper]
    pub(super) fn validate_ids(&self, actual: &[PolicyId]) -> Vec<PolicyCoverageIssue> {
        let expected = self.ids();
        expected
            .iter()
            .filter(|id| !actual.contains(id))
            .cloned()
            .map(PolicyCoverageIssue::Missing)
            .chain(
                actual
                    .iter()
                    .filter(|id| !expected.contains(id))
                    .cloned()
                    .map(PolicyCoverageIssue::Unexpected),
            )
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyCoverageIssue {
    Missing(PolicyId),
    Unexpected(PolicyId),
}
