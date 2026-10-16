use std::collections::{BTreeMap, BTreeSet};

use review_kit_application::publication::PolicyName;
use review_kit_domain::{Finding, PolicyId};

use super::round::CompactRound;
use crate::{JsonError, domain::FindingData, rounds::PolicyNameData};

const MAX_EXPANDED_STATE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXPANDED_REFERENCES: usize = 65_536;

#[derive(Default)]
pub(super) struct Tables {
    pub findings: Vec<FindingData>,
    pub policy_names: Vec<PolicyNameData>,
    finding_indexes: BTreeMap<String, u64>,
    name_indexes: BTreeMap<(String, String), u64>,
}

#[review_kit_observation::operations]
impl Tables {
    #[helper]
    pub fn finding(&mut self, finding: &Finding) -> Result<u64, JsonError> {
        let value = FindingData::from(finding);
        let key = serde_json::to_string(&value).map_err(JsonError::Syntax)?;
        if let Some(index) = self.finding_indexes.get(&key) {
            return Ok(*index);
        }
        let index = self.findings.len() as u64;
        self.findings.push(value);
        self.finding_indexes.insert(key, index);
        Ok(index)
    }

    #[helper]
    pub fn policy_name(&mut self, policy: &PolicyName) -> Result<u64, JsonError> {
        crate::rounds::nonblank(&policy.name)?;
        let key = (policy.id.as_str().to_owned(), policy.name.clone());
        if let Some(index) = self.name_indexes.get(&key) {
            return Ok(*index);
        }
        let index = self.policy_names.len() as u64;
        self.policy_names.push(PolicyNameData {
            id: key.0.clone(),
            name: key.1.clone(),
        });
        self.name_indexes.insert(key, index);
        Ok(index)
    }
}

pub(super) struct ValidatedTables {
    findings: Vec<Finding>,
    policy_names: Vec<PolicyName>,
}

#[review_kit_observation::operations]
impl ValidatedTables {
    #[helper]
    pub fn new(findings: Vec<FindingData>, names: Vec<PolicyNameData>) -> Result<Self, JsonError> {
        Ok(Self {
            findings: findings
                .into_iter()
                .map(FindingData::into_finding)
                .collect::<Result<_, _>>()?,
            policy_names: names
                .into_iter()
                .map(|name| {
                    crate::rounds::nonblank(&name.name)?;
                    Ok(PolicyName {
                        id: crate::domain::valid(PolicyId::try_from(name.id))?,
                        name: name.name,
                    })
                })
                .collect::<Result<_, JsonError>>()?,
        })
    }

    #[helper]
    pub fn finding(&self, index: u64) -> Result<Finding, JsonError> {
        let index = usize::try_from(index).map_err(|_| JsonError::InvalidValue)?;
        self.findings
            .get(index)
            .cloned()
            .ok_or(JsonError::InvalidValue)
    }

    #[helper]
    pub fn policy_name(&self, index: u64) -> Result<PolicyName, JsonError> {
        let index = usize::try_from(index).map_err(|_| JsonError::InvalidValue)?;
        self.policy_names
            .get(index)
            .cloned()
            .ok_or(JsonError::InvalidValue)
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn validate<'a>(
    findings: &[FindingData],
    names: &[PolicyNameData],
    rounds: impl Iterator<Item = &'a CompactRound>,
    header_bytes: usize,
    encoded_bytes: usize,
) -> Result<(), JsonError> {
    let mut distinct = BTreeSet::new();
    let sizes = findings
        .iter()
        .map(|finding| {
            let key = serde_json::to_string(finding).map_err(JsonError::Syntax)?;
            let size = key.len();
            if !distinct.insert(key) {
                return Err(JsonError::InvalidValue);
            }
            Ok(size)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut distinct_names = BTreeSet::new();
    let name_sizes = names
        .iter()
        .map(|name| {
            if !distinct_names.insert((&name.id, &name.name)) {
                return Err(JsonError::InvalidValue);
            }
            Ok(serde_json::to_vec(name).map_err(JsonError::Syntax)?.len())
        })
        .collect::<Result<Vec<_>, JsonError>>()?;
    let mut used_findings = Usage::new(findings.len());
    let mut used_names = Usage::new(names.len());
    let mut budget = Budget {
        bytes: encoded_bytes,
        references: 0,
    };
    for round in rounds {
        budget.reference(header_bytes)?;
        for _ in 0..round.resolution_count() {
            budget.reference(header_bytes)?;
        }
        for index in round.finding_references() {
            let index = used_findings.reference(index)?;
            budget.reference(sizes[index])?;
        }
        for index in round.policy_name_references() {
            let index = used_names.reference(index)?;
            budget.reference(name_sizes[index])?;
        }
    }
    used_findings.complete()?;
    used_names.complete()?;
    Ok(())
}

struct Usage {
    used: Vec<bool>,
    next: usize,
}

#[review_kit_observation::operations]
impl Usage {
    #[helper]
    fn new(length: usize) -> Self {
        Self {
            used: vec![false; length],
            next: 0,
        }
    }

    #[helper]
    fn reference(&mut self, index: u64) -> Result<usize, JsonError> {
        let index = usize::try_from(index).map_err(|_| JsonError::InvalidValue)?;
        let used = self.used.get_mut(index).ok_or(JsonError::InvalidValue)?;
        if !*used {
            if index != self.next {
                return Err(JsonError::InvalidValue);
            }
            *used = true;
            self.next += 1;
        }
        Ok(index)
    }

    #[helper]
    fn complete(self) -> Result<(), JsonError> {
        if self.next != self.used.len() {
            return Err(JsonError::InvalidValue);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Budget {
    bytes: usize,
    references: usize,
}

#[review_kit_observation::operations]
impl Budget {
    #[helper]
    fn reference(&mut self, bytes: usize) -> Result<(), JsonError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(JsonError::RetainedStateLimit)?;
        self.references = self
            .references
            .checked_add(1)
            .ok_or(JsonError::RetainedStateLimit)?;
        if self.bytes > MAX_EXPANDED_STATE_BYTES || self.references > MAX_EXPANDED_REFERENCES {
            return Err(JsonError::RetainedStateLimit);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_budget_accepts_its_boundary_and_rejects_arithmetic_overflow() {
        let mut bytes = Budget::default();
        assert!(bytes.reference(MAX_EXPANDED_STATE_BYTES).is_ok());
        assert!(matches!(
            bytes.reference(1),
            Err(JsonError::RetainedStateLimit)
        ));
        let mut overflow = Budget {
            bytes: usize::MAX,
            references: 0,
        };
        assert!(matches!(
            overflow.reference(1),
            Err(JsonError::RetainedStateLimit)
        ));
        let mut references = Budget {
            bytes: 0,
            references: MAX_EXPANDED_REFERENCES - 1,
        };
        assert!(references.reference(0).is_ok());
        assert!(matches!(
            references.reference(0),
            Err(JsonError::RetainedStateLimit)
        ));
        let mut overflow = Budget {
            bytes: 0,
            references: usize::MAX,
        };
        assert!(matches!(
            overflow.reference(0),
            Err(JsonError::RetainedStateLimit)
        ));
    }
}
