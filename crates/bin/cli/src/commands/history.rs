//! Managed review history conflicts and unsupported retained formats.
//!
//! A conflict means the authenticated reviewer's own provider records contradict
//! each other. Review Kit reports it and stops; it never repairs provider
//! history, treats the finding as fixed or recreates it. Provider read and
//! transport failures keep their own codes.
//! Unsupported versions retain their version and require a supporting executable.

use std::error::Error;

use review_kit_application::use_cases::{
    discovery::{DiscoveryConflict, DiscoveryError},
    discovery_bundle::BundleError,
    publishing::PlanError,
};
use review_kit_domain::FindingId;
use review_kit_gitlab_adapter::{FindingConflict, GitLab, GitLabError, ManagedRoot};
use serde_json::{Value, json};

use crate::{
    commands::status::bundle_exit,
    error::{CliError, gitlab_cause},
};

/// What the operator can do about a managed history conflict.
pub const GUIDANCE: &str = "Review Kit does not repair provider history: it never treats the finding as fixed or recreates it. This operation is blocked; keep the records unchanged and report the conflict. Provider-history repair or a different reviewer identity requires separate operator authority.";

/// Where an authorized operator resolves duplicate managed roots.
pub const DUPLICATE_GUIDANCE: &str = "Review Kit does not repair provider history and never deletes duplicates. This operation is blocked. A separately authorized operator can use the duplicate-root recovery procedure in the CLI guide; preserve the finding readback records.";

/// The reported `conflict` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// The finding's root note is gone while the summary or its replies still record it.
    MissingFindingRoot,
    /// More than one root claims one finding identity.
    DuplicateFinding,
    /// Summary, finding or reply records disagree with each other.
    InconsistentHistory,
    /// The owned summary declares a retained format this executable cannot read.
    UnsupportedStateVersion(u64),
}

/// A managed history conflict and the finding it concerns, when one is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryConflict {
    pub kind: ConflictKind,
    pub finding: Option<FindingId>,
}

#[review_kit_observation::operations]
impl ConflictKind {
    #[helper]
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingFindingRoot => "missing_finding_root",
            Self::DuplicateFinding => "duplicate_finding",
            Self::InconsistentHistory => "inconsistent_history",
            Self::UnsupportedStateVersion(_) => "unsupported_retained_state_version",
        }
    }

    #[helper]
    fn description(self) -> &'static str {
        match self {
            Self::MissingFindingRoot => {
                "its root note is missing while the reviewer's summary or replies still record it"
            }
            Self::DuplicateFinding => "more than one managed root note claims it",
            Self::InconsistentHistory => {
                "the reviewer's managed summary, finding or reply records contradict each other"
            }
            Self::UnsupportedStateVersion(_) => {
                "the retained summary uses an unsupported state version"
            }
        }
    }
}

/// The managed history conflict behind a discovery failure, if it is one.
///
/// A wrong subject or reviewer is an identity failure, not a history conflict.
/// Invalid retained Review Kit state read from the provider is inconsistent history.
#[review_kit_observation::operations]
#[helper]
pub fn in_discovery(error: &DiscoveryError) -> Option<HistoryConflict> {
    let (kind, finding) = match error {
        DiscoveryError::Read(error) => match gitlab_cause(error)? {
            GitLabError::FindingHistory { finding, conflict } => (
                match conflict {
                    FindingConflict::MissingRoot => ConflictKind::MissingFindingRoot,
                    FindingConflict::DuplicateRoot => ConflictKind::DuplicateFinding,
                    FindingConflict::Inconsistent => ConflictKind::InconsistentHistory,
                },
                Some(*finding),
            ),
            GitLabError::State => (ConflictKind::InconsistentHistory, None),
            GitLabError::UnsupportedStateVersion(version) => {
                (ConflictKind::UnsupportedStateVersion(*version), None)
            }
            _ => return None,
        },
        DiscoveryError::Conflict(conflict) => match conflict {
            DiscoveryConflict::WrongSubject | DiscoveryConflict::WrongReviewer => return None,
            DiscoveryConflict::MissingPublishedFinding(id) => {
                (ConflictKind::MissingFindingRoot, Some(*id))
            }
            DiscoveryConflict::DuplicateFinding(id) => (ConflictKind::DuplicateFinding, Some(*id)),
            DiscoveryConflict::InvalidFindingState(id)
            | DiscoveryConflict::ConflictingReply(id) => {
                (ConflictKind::InconsistentHistory, Some(*id))
            }
            DiscoveryConflict::MultipleSummaries
            | DiscoveryConflict::DuplicateProviderId
            | DiscoveryConflict::InvalidRound
            | DiscoveryConflict::ForeignRound => (ConflictKind::InconsistentHistory, None),
        },
    };
    Some(HistoryConflict { kind, finding })
}

/// The managed history conflict behind a review-context capture failure, if it is one.
///
/// Every finding-history reconciliation failure is one: the retained history
/// cannot be replayed into the reviewer's current finding state.
#[review_kit_observation::operations]
#[helper]
pub fn in_bundle(error: &BundleError) -> Option<HistoryConflict> {
    match error {
        BundleError::Discovery(error) => in_discovery(error),
        BundleError::FindingState(error) => Some(HistoryConflict {
            kind: if matches!(error, PlanError::MissingPublishedFinding(_)) {
                ConflictKind::MissingFindingRoot
            } else {
                ConflictKind::InconsistentHistory
            },
            finding: plan_finding(error),
        }),
        _ => None,
    }
}

/// The finding a history reconciliation failure names, if any.
#[review_kit_observation::operations]
#[helper]
fn plan_finding(error: &PlanError) -> Option<FindingId> {
    match error {
        PlanError::MissingPublishedFinding(id)
        | PlanError::ConflictingFinding(id)
        | PlanError::ConflictingReply(id)
        | PlanError::InvalidAnswer(id)
        | PlanError::InvalidTransition(id)
        | PlanError::DuplicateTransition(id)
        | PlanError::MissingTransition(id) => Some(*id),
        _ => None,
    }
}

/// Report a preflight discovery failure; `gitlab` lists the roots of a duplicate finding.
#[review_kit_observation::operations]
#[helper]
pub fn discovery_error(error: DiscoveryError, gitlab: &GitLab) -> CliError {
    match in_discovery(&error) {
        Some(conflict) => conflict_error(conflict, &error, gitlab),
        None => {
            let exit = crate::commands::status::discovery_exit(&error);
            CliError::adapter("discover review", "discovery", exit, error)
        }
    }
}

/// Report a review-context capture failure; `gitlab` lists the roots of a duplicate finding.
#[review_kit_observation::operations]
#[helper]
pub fn bundle_error(error: BundleError, gitlab: &GitLab) -> CliError {
    match in_bundle(&error) {
        Some(conflict) => conflict_error(conflict, &error, gitlab),
        None => {
            let exit = bundle_exit(&error);
            CliError::adapter("discover review", "discovery", exit, error)
        }
    }
}

/// The `managed_history_conflict` diagnostic, keeping the cause chain.
///
/// A duplicate finding adds `roots`: each root note that claims it, with its
/// creation time, or `null` when they cannot be listed.
#[review_kit_observation::operations]
#[helper]
fn conflict_error(
    conflict: HistoryConflict,
    error: &(dyn Error + 'static),
    gitlab: &GitLab,
) -> CliError {
    if let ConflictKind::UnsupportedStateVersion(version) = conflict.kind {
        return unsupported_state_error(version, error);
    }
    let roots = match (conflict.kind, conflict.finding) {
        (ConflictKind::DuplicateFinding, Some(finding)) => Some(gitlab.finding_roots(finding).ok()),
        _ => None,
    };
    let listed = roots
        .iter()
        .flatten()
        .flatten()
        .map(|root| match &root.created_at {
            Some(created) => format!("{} created {created}", root.note.get()),
            None => root.note.get().to_string(),
        })
        .collect::<Vec<_>>();
    let mut description = conflict.kind.description().to_owned();
    if !listed.is_empty() {
        description = format!("{description} (notes {})", listed.join(", "));
    }
    let message = match conflict.finding {
        Some(finding) => {
            format!("managed review history conflicts for finding {finding}: {description}")
        }
        None => format!("managed review history conflicts: {description}"),
    };
    let causes = cause_messages(error);
    let mut details = json!({
        "conflict": conflict.kind.code(),
        "finding": conflict.finding.map(FindingId::get),
        "causes": causes,
        "next": if roots.is_some() { DUPLICATE_GUIDANCE } else { GUIDANCE },
    });
    if let Some(roots) = roots {
        details["roots"] = roots.map_or(Value::Null, |roots| {
            roots.iter().map(root_value).collect::<Value>()
        });
    }
    CliError::state("managed_history_conflict", 5, message).with_details(details)
}

#[review_kit_observation::operations]
#[helper]
fn unsupported_state_error(version: u64, error: &(dyn Error + 'static)) -> CliError {
    let supported = review_kit_json_adapter::rounds::State::VERSION;
    CliError::state(
        "unsupported_retained_state_version", 5,
        format!("retained summary state version {version} is unsupported; this executable reads version {supported}"),
    ).with_details(json!({
        "state_version": version,
        "supported_state_version": supported,
        "causes": cause_messages(error),
        "next": "Retain the provider records and use an executable that supports their state version. Review Kit leaves the records unchanged.",
    }))
}

#[review_kit_observation::operations]
#[helper]
fn cause_messages(error: &(dyn Error + 'static)) -> Vec<String> {
    let mut causes = Vec::new();
    let mut cause = error.source();
    while let Some(current) = cause {
        causes.push(current.to_string());
        cause = current.source();
    }
    causes
}

#[review_kit_observation::operations]
#[helper]
fn root_value(root: &ManagedRoot) -> Value {
    json!({"note": root.note.get(), "created_at": root.created_at})
}

#[cfg(test)]
mod tests {
    use super::*;
    use review_kit_application::ports::PortError;

    #[test]
    fn unsupported_state_keeps_its_version_and_avoids_history_repair_guidance()
    -> Result<(), Box<dyn Error>> {
        let discovery = DiscoveryError::Read(PortError::new(
            "GitLab discovery",
            GitLabError::UnsupportedStateVersion(4),
        ));
        let conflict = in_discovery(&discovery).ok_or("unsupported state classification")?;
        assert_eq!(conflict.kind, ConflictKind::UnsupportedStateVersion(4));
        assert_eq!(conflict.finding, None);
        let error = unsupported_state_error(4, &discovery);
        let document = &error.document()["diagnostics"][0];
        assert_eq!(error.exit(), 5);
        assert_eq!(document["code"], "unsupported_retained_state_version");
        assert_eq!(document["state_version"], 4);
        assert_eq!(document["supported_state_version"], 3);
        assert!(
            !document["next"]
                .as_str()
                .ok_or("version guidance")?
                .contains("repair")
        );
        Ok(())
    }
}
