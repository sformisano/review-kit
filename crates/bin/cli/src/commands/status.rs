use review_kit_application::use_cases::{
    assessment::CheckProblem,
    discovery::{DiscoveryConflict, DiscoveryError},
    discovery_bundle::BundleError,
    publication_run::{RunOutcome, RunReport},
    publishing::{BindingError, PlanError, PublicationResult, Stop},
};

use crate::error::{gitlab_read_exit, source_check_exit};

pub fn run_exit(report: &RunReport) -> u8 {
    if report.evidence_failure.is_some() {
        return 7;
    }
    match &report.outcome {
        Ok(RunOutcome::Planned(_)) => 0,
        Ok(RunOutcome::Published(result)) => publication_exit(result),
        Err(error) => plan_exit(error),
    }
}

pub fn plan_exit(error: &PlanError) -> u8 {
    match error {
        PlanError::ChangedRevision => 3,
        PlanError::WrongReviewer => 4,
        PlanError::ProgressBinding => 5,
        PlanError::Discovery(error) => discovery_exit(error),
        PlanError::Evidence(_) => 7,
        PlanError::Binding(error) => match error {
            BindingError::Unresolved => 6,
            BindingError::MissingReason => 2,
            _ => 5,
        },
        PlanError::ShapeChecks(errors) | PlanError::ReplyChecks(errors) => {
            combined_exit(errors.iter().map(plan_exit))
        }
        PlanError::Checks(issues) => {
            combined_exit(issues.iter().map(|issue| match &issue.problem {
                CheckProblem::Source(error) | CheckProblem::Position(error) => {
                    source_check_exit(error)
                }
                _ => 2,
            }))
        }
        _ => 2,
    }
}

pub fn bundle_exit(error: &BundleError) -> u8 {
    match error {
        BundleError::Discovery(error) => discovery_exit(error),
        BundleError::Changes(error) => gitlab_read_exit(error),
        BundleError::ChangedRevision => 3,
        BundleError::FindingState(_)
        | BundleError::WrongSubject
        | BundleError::FindingIdExhausted => 5,
    }
}

pub fn discovery_exit(error: &DiscoveryError) -> u8 {
    match error {
        DiscoveryError::Conflict(DiscoveryConflict::WrongReviewer) => 4,
        DiscoveryError::Conflict(_) => 5,
        DiscoveryError::Read(_) if super::history::in_discovery(error).is_some() => 5,
        DiscoveryError::Read(error) => gitlab_read_exit(error),
    }
}

fn combined_exit(exits: impl Iterator<Item = u8>) -> u8 {
    let mut result = 2;
    for exit in exits {
        if exit == 3 {
            return 3;
        }
        if result == 2 && exit != 2 {
            result = exit;
        }
    }
    result
}

pub fn publication_exit(result: &PublicationResult) -> u8 {
    let stops: Vec<_> = result
        .stop
        .iter()
        .chain(result.finalization_stop.iter())
        .collect();
    if stops.iter().any(|stop| {
        matches!(
            stop,
            Stop::Unknown(_) | Stop::UnresolvedWrite | Stop::ReadbackMismatch
        )
    }) {
        return 6;
    }
    stops.first().map_or(0, |stop| match stop {
        Stop::Authority(_) => 4,
        Stop::RevisionChanged => 3,
        Stop::ConcurrentChange | Stop::Conflict => 5,
        Stop::Render(_) => 2,
        Stop::Preparation(error) => plan_exit(error),
        Stop::Read(error) | Stop::Rejected(error) => gitlab_read_exit(error),
        Stop::Evidence(_) => 7,
        Stop::Unknown(_) | Stop::UnresolvedWrite | Stop::ReadbackMismatch => 6,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use review_kit_application::ports::PortError;

    fn unavailable() -> PortError {
        PortError::new("read GitLab", std::io::Error::other("connection lost"))
    }

    #[test]
    fn publication_reads_preserve_conflicts_and_revision_drift_with_uncertainty_priority()
    -> Result<(), Box<dyn std::error::Error>> {
        use review_kit_application::{
            publication::{Effects, Snapshot},
            use_cases::publishing::PublicationStatus,
        };
        use review_kit_domain::{ReviewDecision, ReviewerId, Revision, Subject};
        use review_kit_gitlab_adapter::GitLabError;

        for (cause, expected) in [
            (GitLabError::Conflict, 5),
            (GitLabError::RevisionChanged, 3),
            (GitLabError::Transport, 7),
        ] {
            let snapshot = Snapshot {
                subject: Subject::new(
                    "gitlab".into(),
                    "https://example.test".into(),
                    "team/project".into(),
                    1,
                )?,
                reviewer: ReviewerId::try_from("9".to_owned())?,
                revision: Revision::new("a".repeat(40), vec!["b".repeat(40), "c".repeat(40)])?,
                summaries: Vec::new(),
                findings: Vec::new(),
                replies: Vec::new(),
                permissions: Effects::review(),
            };
            let mut result = PublicationResult {
                final_snapshot: snapshot,
                status: PublicationStatus::NotStarted,
                decision: ReviewDecision::Ready,
                round: 1,
                planned: Vec::new(),
                attempted: Vec::new(),
                verified: Vec::new(),
                retried_creations: Vec::new(),
                skipped: Vec::new(),
                stop: Some(Stop::Read(PortError::new(
                    "read current review",
                    PortError::new("read GitLab", cause),
                ))),
                finalization_stop: None,
            };
            assert_eq!(publication_exit(&result), expected, "{cause:?}");
            result.stop = Some(Stop::Rejected(PortError::new(
                "dispatch GitLab write",
                cause,
            )));
            assert_eq!(publication_exit(&result), expected, "rejected {cause:?}");
            result.finalization_stop = Some(Stop::Unknown(unavailable()));
            assert_eq!(
                publication_exit(&result),
                6,
                "unknown finalization with {cause:?}"
            );
            result.finalization_stop = None;
            result.stop = Some(Stop::Preparation(PlanError::Discovery(
                DiscoveryError::Read(PortError::new("resume publication", cause)),
            )));
            assert_eq!(publication_exit(&result), expected, "resume {cause:?}");
            result.stop = Some(Stop::Preparation(PlanError::WrongReviewer));
            assert_eq!(publication_exit(&result), 4);
        }
        Ok(())
    }

    #[test]
    fn provider_reads_keep_their_exit_distinct_from_conflicts_and_revision_drift() {
        use review_kit_gitlab_adapter::GitLabError;

        assert_eq!(bundle_exit(&BundleError::Changes(unavailable())), 7);
        assert_eq!(
            bundle_exit(&BundleError::Discovery(DiscoveryError::Read(unavailable()))),
            7
        );
        assert_eq!(
            plan_exit(&PlanError::Discovery(DiscoveryError::Read(unavailable()))),
            7
        );
        assert_eq!(
            plan_exit(&PlanError::Discovery(DiscoveryError::Conflict(
                DiscoveryConflict::WrongReviewer
            ))),
            4
        );
        assert_eq!(bundle_exit(&BundleError::WrongSubject), 5);
        assert_eq!(bundle_exit(&BundleError::ChangedRevision), 3);
        for (cause, expected) in [
            (GitLabError::RevisionChanged, 3),
            (GitLabError::Conflict, 5),
            (GitLabError::Transport, 7),
        ] {
            let nested = PortError::new(
                "discover review",
                PortError::new("read GitLab snapshot", cause),
            );
            let discovery = DiscoveryError::Read(nested);
            assert_eq!(discovery_exit(&discovery), expected, "{cause:?}");
            assert_eq!(
                bundle_exit(&BundleError::Discovery(discovery)),
                expected,
                "nested {cause:?}"
            );
        }
    }

    #[test]
    fn managed_history_conflicts_are_state_conflicts_while_provider_reads_keep_their_exit()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::commands::history::{ConflictKind, in_bundle, in_discovery};
        use review_kit_domain::FindingId;
        use review_kit_gitlab_adapter::{FindingConflict, GitLabError};

        let finding = FindingId::try_from(2)?;
        for (cause, kind, named) in [
            (
                GitLabError::FindingHistory {
                    finding,
                    conflict: FindingConflict::MissingRoot,
                },
                ConflictKind::MissingFindingRoot,
                Some(finding),
            ),
            (
                GitLabError::FindingHistory {
                    finding,
                    conflict: FindingConflict::DuplicateRoot,
                },
                ConflictKind::DuplicateFinding,
                Some(finding),
            ),
            (GitLabError::State, ConflictKind::InconsistentHistory, None),
            (
                GitLabError::UnsupportedStateVersion(4),
                ConflictKind::UnsupportedStateVersion(4),
                None,
            ),
        ] {
            let discovery = DiscoveryError::Read(PortError::new(
                "discover review",
                PortError::new("GitLab discovery", cause),
            ));
            let conflict = in_discovery(&discovery).ok_or("not a history conflict")?;
            assert_eq!(
                (conflict.kind, conflict.finding),
                (kind, named),
                "{cause:?}"
            );
            assert_eq!(discovery_exit(&discovery), 5, "{cause:?}");
            assert_eq!(
                bundle_exit(&BundleError::Discovery(discovery)),
                5,
                "{cause:?}"
            );
        }
        for cause in [GitLabError::Transport, GitLabError::Rejected(503)] {
            let discovery = DiscoveryError::Read(PortError::new("GitLab discovery", cause));
            assert!(in_discovery(&discovery).is_none(), "{cause:?}");
            assert_eq!(discovery_exit(&discovery), 7, "{cause:?}");
        }
        let wrong = DiscoveryError::Conflict(DiscoveryConflict::WrongReviewer);
        assert!(in_discovery(&wrong).is_none());
        assert_eq!(discovery_exit(&wrong), 4);
        let missing = DiscoveryError::Conflict(DiscoveryConflict::MissingPublishedFinding(finding));
        assert_eq!(
            in_discovery(&missing).map(|conflict| (conflict.kind, conflict.finding)),
            Some((ConflictKind::MissingFindingRoot, Some(finding)))
        );
        let reconciled = BundleError::FindingState(PlanError::ConflictingReply(finding));
        assert_eq!(
            in_bundle(&reconciled).map(|conflict| (conflict.kind, conflict.finding)),
            Some((ConflictKind::InconsistentHistory, Some(finding)))
        );
        assert_eq!(bundle_exit(&reconciled), 5);
        Ok(())
    }

    #[test]
    fn source_preflight_retains_revision_and_provider_failure_categories() {
        use review_kit_application::use_cases::assessment::{CheckIssue, CheckProblem};
        use review_kit_gitlab_adapter::GitLabError;

        for (cause, expected) in [
            (GitLabError::RevisionChanged, 3),
            (GitLabError::Transport, 7),
            (GitLabError::Authentication, 7),
            (GitLabError::Rejected(503), 7),
            (GitLabError::UnsupportedSource, 2),
            (GitLabError::UnsupportedPosition, 2),
        ] {
            for position in [false, true] {
                let error = PortError::new("validate GitLab source", cause);
                let problem = if position {
                    CheckProblem::Position(error)
                } else {
                    CheckProblem::Source(error)
                };
                let checked = PlanError::Checks(vec![CheckIssue {
                    finding: None,
                    problem,
                }]);
                assert_eq!(plan_exit(&checked), expected, "{cause:?}");
                let nested = PlanError::ShapeChecks(vec![PlanError::ReplyChecks(vec![checked])]);
                assert_eq!(plan_exit(&nested), expected, "nested {cause:?}");
            }
        }
        assert_eq!(
            bundle_exit(&BundleError::Changes(PortError::new(
                "read current GitLab diff",
                GitLabError::RevisionChanged,
            ))),
            3
        );
    }
}
