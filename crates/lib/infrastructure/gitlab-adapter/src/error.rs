use review_kit_application::ports::PortError;
use review_kit_domain::FindingId;

/// Failure categories omit credentials, response bodies, URLs, and local paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum GitLabError {
    /// invalid GitLab configuration
    Config,
    /// GitLab subject or actor does not match the binding
    Identity,
    /// incomplete or inconsistent GitLab response
    Protocol,
    /// invalid or unsupported Review Kit marker
    Marker,
    /// invalid Review Kit retained state
    State,
    /// unsupported Review Kit retained state version {0}
    UnsupportedStateVersion(u64),
    /// source is not a supported committed regular text blob
    UnsupportedSource,
    /// anchor or suggestion range is unavailable in the reviewed diff
    UnsupportedPosition,
    /// GitLab command syntax is forbidden outside fenced or indented code
    UnsafeBody,
    /// rendered GitLab bodies must exclude carriage returns
    CarriageReturnBody,
    /// GitLab comparison changed
    RevisionChanged,
    /// configured GitLab size or pagination limit exceeded
    Limit,
    /// GitLab read failed; check connectivity and retry discovery
    Transport,
    /// GitLab redirect refused; configure the canonical server
    Redirect,
    /// GitLab rejected the credential
    Authentication,
    /// GitLab denied the operation
    Denied,
    /// GitLab object is unavailable
    NotFound,
    /// GitLab rate limit reached
    RateLimited,
    /// The HTTP status remains available for classification and is omitted from display text.
    #[displaydoc("GitLab rejected the request")]
    Rejected(u16),
    /// GitLab write outcome is unknown; retain progress before resuming
    Uncertain,
    /// conflicting Review Kit provider state
    Conflict,
    /// managed history of finding {finding} conflicts: {conflict}
    FindingHistory {
        finding: FindingId,
        conflict: FindingConflict,
    },
}

/// How one finding's managed provider records contradict each other.
///
/// Discovery stops on such records; it never treats the finding as fixed or
/// recreates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display)]
pub enum FindingConflict {
    /// a managed reply remains without its finding root
    MissingRoot,
    /// more than one managed root claims the finding
    DuplicateRoot,
    /// a managed record disagrees with the retained summary history
    Inconsistent,
}

impl GitLabError {
    pub(crate) fn port(self, operation: &'static str) -> PortError {
        PortError::new(operation, self)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::GitLabError;

    #[test]
    fn failure_messages_preserve_safe_display_text_without_nested_sources() {
        for (error, expected) in [
            (GitLabError::Config, "invalid GitLab configuration"),
            (
                GitLabError::Identity,
                "GitLab subject or actor does not match the binding",
            ),
            (
                GitLabError::Protocol,
                "incomplete or inconsistent GitLab response",
            ),
            (
                GitLabError::Marker,
                "invalid or unsupported Review Kit marker",
            ),
            (GitLabError::State, "invalid Review Kit retained state"),
            (
                GitLabError::UnsupportedStateVersion(4),
                "unsupported Review Kit retained state version 4",
            ),
            (
                GitLabError::UnsupportedSource,
                "source is not a supported committed regular text blob",
            ),
            (
                GitLabError::UnsupportedPosition,
                "anchor or suggestion range is unavailable in the reviewed diff",
            ),
            (
                GitLabError::UnsafeBody,
                "GitLab command syntax is forbidden outside fenced or indented code",
            ),
            (
                GitLabError::CarriageReturnBody,
                "rendered GitLab bodies must exclude carriage returns",
            ),
            (GitLabError::RevisionChanged, "GitLab comparison changed"),
            (
                GitLabError::Limit,
                "configured GitLab size or pagination limit exceeded",
            ),
            (
                GitLabError::Transport,
                "GitLab read failed; check connectivity and retry discovery",
            ),
            (
                GitLabError::Redirect,
                "GitLab redirect refused; configure the canonical server",
            ),
            (
                GitLabError::Authentication,
                "GitLab rejected the credential",
            ),
            (GitLabError::Denied, "GitLab denied the operation"),
            (GitLabError::NotFound, "GitLab object is unavailable"),
            (GitLabError::RateLimited, "GitLab rate limit reached"),
            (GitLabError::Rejected(422), "GitLab rejected the request"),
            (GitLabError::Rejected(503), "GitLab rejected the request"),
            (
                GitLabError::Uncertain,
                "GitLab write outcome is unknown; retain progress before resuming",
            ),
            (
                GitLabError::Conflict,
                "conflicting Review Kit provider state",
            ),
        ] {
            assert_eq!(error.to_string(), expected);
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn finding_history_conflicts_name_the_finding_and_the_kind_without_sources()
    -> Result<(), Box<dyn Error>> {
        use super::FindingConflict;
        use review_kit_domain::FindingId;

        let finding = FindingId::try_from(2)?;
        for (conflict, expected) in [
            (
                FindingConflict::MissingRoot,
                "managed history of finding 2 conflicts: a managed reply remains without its finding root",
            ),
            (
                FindingConflict::DuplicateRoot,
                "managed history of finding 2 conflicts: more than one managed root claims the finding",
            ),
            (
                FindingConflict::Inconsistent,
                "managed history of finding 2 conflicts: a managed record disagrees with the retained summary history",
            ),
        ] {
            let error = GitLabError::FindingHistory { finding, conflict };
            assert_eq!(error.to_string(), expected);
            assert!(error.source().is_none());
        }
        Ok(())
    }

    #[test]
    fn port_error_preserves_rejected_status_in_its_source() -> Result<(), &'static str> {
        let error = GitLabError::Rejected(422).port("GitLab discovery");

        assert_eq!(error.to_string(), "GitLab discovery failed");
        let source = error.source().ok_or("GitLab source is missing")?;
        assert_eq!(source.to_string(), "GitLab rejected the request");
        assert_eq!(
            source.downcast_ref::<GitLabError>(),
            Some(&GitLabError::Rejected(422))
        );
        assert!(source.source().is_none());
        Ok(())
    }
}
