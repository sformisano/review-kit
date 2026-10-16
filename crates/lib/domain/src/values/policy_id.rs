/// A nonblank policy identifier whose exact spelling is preserved.
///
/// This value imposes no slug alphabet and does not prove that a policy
/// definition exists. Assessments check membership in their declared coverage.
/// Equality and ordering use the retained text, including case and surrounding
/// whitespace. Display writes that text unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, displaydoc::Display)]
#[displaydoc("{0}")]
pub struct PolicyId(String);

impl PolicyId {
    /// The original identifier, without trimming or case conversion.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PolicyId {
    type Error = BlankPolicyId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(BlankPolicyId);
        }
        Ok(Self(value))
    }
}

/// The input is empty after [`str::trim`], including Unicode whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("policy ID must not be blank")]
pub struct BlankPolicyId;

#[cfg(test)]
mod tests {
    use super::{BlankPolicyId, PolicyId};

    #[test]
    fn blank_identifiers_are_rejected() {
        for raw in ["", " ", "\t\n", "\u{2003}"] {
            assert_eq!(PolicyId::try_from(raw.to_owned()), Err(BlankPolicyId));
        }
    }

    #[test]
    fn identifiers_preserve_their_exact_spelling() -> Result<(), BlankPolicyId> {
        for raw in [
            "rust-error-contracts",
            "Team/Policy.v2",
            "règle",
            " policy ",
        ] {
            let id = PolicyId::try_from(raw.to_owned())?;
            assert_eq!(id.as_str(), raw);
        }
        Ok(())
    }

    #[test]
    fn different_spellings_remain_distinct() -> Result<(), BlankPolicyId> {
        let id = PolicyId::try_from("policy".to_owned())?;
        let other_case = PolicyId::try_from("Policy".to_owned())?;
        let padded = PolicyId::try_from(" policy ".to_owned())?;
        assert_ne!(id, other_case);
        assert_ne!(id, padded);
        Ok(())
    }
}
