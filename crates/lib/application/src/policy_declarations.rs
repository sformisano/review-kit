use std::collections::BTreeSet;

use review_kit_domain::{Assessment, PolicyId, PolicyJudgment};

use crate::{publication::PolicyName, use_cases::checkpoints::ApplicabilityStatus};

/// Public inventory identity and applicability for one publication round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPolicyDeclaration {
    pub policy: PolicyName,
    pub status: ApplicabilityStatus,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum InvalidPolicyDeclarations {
    /// policy declaration repeats an identifier: {0}
    Duplicate(PolicyId),
    /// policy declaration has a blank display name or reason: {0}
    Blank(PolicyId),
    /// assessment slots differ from the public policy declaration
    Slots,
    /// unresolved applicability requires a blocked judgment: {0}
    UnresolvedJudgment(PolicyId),
}

/// Declared applicability owns the assessed-slot names and identifiers.
#[review_kit_observation::operations]
#[helper]
pub fn validate_policy_declarations(
    declarations: &[PublicPolicyDeclaration],
    assessment: &Assessment,
    names: &[PolicyName],
) -> Result<(), InvalidPolicyDeclarations> {
    let mut ids = BTreeSet::new();
    let mut slots = Vec::new();
    for row in declarations {
        if !ids.insert(&row.policy.id) {
            return Err(InvalidPolicyDeclarations::Duplicate(row.policy.id.clone()));
        }
        if row.policy.name.trim().is_empty() || row.reason.trim().is_empty() {
            return Err(InvalidPolicyDeclarations::Blank(row.policy.id.clone()));
        }
        if row.status != ApplicabilityStatus::Excluded {
            slots.push(row.policy.clone());
        }
        if row.status == ApplicabilityStatus::Unresolved
            && !assessment.evaluations().iter().any(|evaluation| {
                evaluation.policy_id() == &row.policy.id
                    && matches!(evaluation.judgment(), PolicyJudgment::Blocked { .. })
            })
        {
            return Err(InvalidPolicyDeclarations::UnresolvedJudgment(
                row.policy.id.clone(),
            ));
        }
    }
    slots.sort_by(|left, right| left.id.cmp(&right.id));
    if slots != names
        || slots.len() != assessment.selected_policies().len()
        || slots
            .iter()
            .any(|slot| !assessment.selected_policies().contains(&slot.id))
    {
        return Err(InvalidPolicyDeclarations::Slots);
    }
    Ok(())
}
