use std::num::NonZeroU64;

/// The provider, host, repository, and local request number identifying one
/// change request.
///
/// It anchors the review across revisions and scopes local finding numbers with
/// the reviewer identity.
///
/// Equality includes every component and preserves the names' exact spelling.
/// Construction checks nonblank names and a positive number.
/// Resource existence and authorization require provider observations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Subject {
    provider: String,
    host: String,
    repository: String,
    request: NonZeroU64,
}

impl Subject {
    /// Retains names without trimming, case conversion, or URL validation.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Blank`] for a blank name, or
    /// [`IdentityError::ZeroRequest`] for request zero.
    pub fn new(
        provider: String,
        host: String,
        repository: String,
        request: u64,
    ) -> Result<Self, IdentityError> {
        for (name, value) in [
            ("provider", &provider),
            ("host", &host),
            ("repository", &repository),
        ] {
            if value.trim().is_empty() {
                return Err(IdentityError::Blank(name));
            }
        }
        let request = NonZeroU64::new(request).ok_or(IdentityError::ZeroRequest)?;
        Ok(Self {
            provider,
            host,
            repository,
            request,
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub fn request(&self) -> u64 {
        self.request.get()
    }
}

/// Opaque reviewer identity, qualified by the enclosing subject's host.
///
/// It distinguishes whose findings and assessments belong to the review.
///
/// Construction rejects blank input and retains every other spelling exactly.
/// Equality compares that spelling only; the enclosing subject supplies the
/// host scope.
/// Provider observations establish authenticated authorship separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReviewerId(String);

impl ReviewerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ReviewerId {
    type Error = IdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(IdentityError::Blank("reviewer"));
        }
        Ok(Self(value))
    }
}

/// Exact head and ordered, provider-defined comparison references.
///
/// It binds an assessment and its actionable suggestions to the comparison reviewed.
/// A changed comparison is a different revision even when the head is unchanged.
///
/// Equality includes the head and each comparison reference in order.
/// An empty comparison list is valid; present references must be nonblank.
/// Adapters preserve the same comparison-reference order on every observation.
/// The value does not establish that any reference exists or is the current
/// provider revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Revision {
    head: String,
    comparison: Vec<String>,
}

impl Revision {
    /// Retains reference spelling, ordering, and duplicates without parsing
    /// revision syntax.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::Blank`] for a blank head or comparison reference.
    pub fn new(head: String, comparison: Vec<String>) -> Result<Self, IdentityError> {
        if head.trim().is_empty() {
            return Err(IdentityError::Blank("head"));
        }
        if comparison
            .iter()
            .any(|reference| reference.trim().is_empty())
        {
            return Err(IdentityError::Blank("comparison reference"));
        }
        Ok(Self { head, comparison })
    }
    pub fn head(&self) -> &str {
        &self.head
    }
    pub fn comparison(&self) -> &[String] {
        &self.comparison
    }
}

/// An identity component fails the local shape checks.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum IdentityError {
    /// The named component is empty after [`str::trim`]; no input text is
    /// retained.
    #[displaydoc("{0} identity must not be blank")]
    Blank(&'static str),
    /// Change request numbering starts at one.
    #[displaydoc("change request number must be greater than zero")]
    ZeroRequest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_scope_distinguishes_equal_local_request_numbers() -> Result<(), IdentityError> {
        let first = Subject::new("gitlab".into(), "one".into(), "repo".into(), 1)?;
        let other_host = Subject::new("gitlab".into(), "two".into(), "repo".into(), 1)?;
        let other_repository = Subject::new("gitlab".into(), "one".into(), "other".into(), 1)?;
        assert_ne!(first, other_host);
        assert_ne!(first, other_repository);
        assert!(Subject::new("gitlab".into(), "one".into(), "repo".into(), 0).is_err());
        assert!(Subject::new("gitlab".into(), " ".into(), "repo".into(), 1).is_err());
        Ok(())
    }

    #[test]
    fn comparison_changes_are_revision_changes() -> Result<(), IdentityError> {
        let revision = Revision::new("head".into(), vec!["base".into(), "start".into()])?;
        let changed = Revision::new("head".into(), vec!["new-base".into(), "start".into()])?;
        assert_ne!(revision, changed);
        assert!(Revision::new("head".into(), vec![" ".into()]).is_err());
        assert!(Revision::new("".into(), vec![]).is_err());
        Ok(())
    }

    #[test]
    fn opaque_actor_identity_is_not_normalized() -> Result<(), IdentityError> {
        assert!(ReviewerId::try_from("\t".to_owned()).is_err());
        assert_eq!(
            ReviewerId::try_from(" Actor/é ".to_owned())?.as_str(),
            " Actor/é "
        );
        Ok(())
    }
}
