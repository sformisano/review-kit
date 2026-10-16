#![doc = include_str!("README.md")]

use crate::observation as log;

use review_kit_domain::{Finding, FindingId, ReviewerId, Revision, SourcePath, Subject};

use super::{
    discovery::{DiscoveryError, discover_managed, highest_finding_id},
    publishing::{
        PlanError, changed_answer_targets, previous_finding, validate_completed_actions,
        validate_historical_roots, validate_retained_structure,
    },
};
use crate::{
    ports::{PortError, ReviewReader},
    publication::{ProviderId, ReplyRecord, RoundStatus, Snapshot},
};

/// A changed path's role in the reviewed diff, including both paths of a rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedFile {
    Added(SourcePath),
    Modified(SourcePath),
    Deleted(SourcePath),
    Renamed {
        previous: SourcePath,
        current: SourcePath,
    },
}

/// Complete changed paths and exact diff text bound to one subject and revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedChanges {
    pub subject: Subject,
    pub revision: Revision,
    pub files: Vec<ChangedFile>,
    pub unified_diff: String,
}

/// Supplies revision-bound change context for drafting and reassessment.
#[review_kit_observation::observed_port(wrapper = ManagedChanges, ownership = "borrowed", visibility = "pub(crate)")]
pub trait ReviewChanges {
    /// Read the complete changed-file list and unified diff at these exact refs.
    #[operation(name = "application.source.changes", input = log::subject(subject), output = |result| log::Completion::result(result, |value| log::Fields::new().count("files", value.files.len()), log::port_error))]
    fn read_changes(
        &mut self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<ObservedChanges, PortError>;
}

/// Review context with reconciled owned findings, relevant conversation, and a
/// finding ID above every retained owned identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryBundle {
    reviewer: ReviewerId,
    snapshot: Snapshot,
    changes: ObservedChanges,
    findings: Vec<Finding>,
    foreign_replies: Vec<ReplyRecord>,
    next_finding_id: FindingId,
    recovery_conflicts: Vec<RecoveryConflict>,
}

/// A retained answer target changed or disappeared in the current unfinished revision.
/// The bundle stays readable, while publication rejects replay against that target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryConflict {
    ChangedAnswerTarget {
        finding: FindingId,
        target: ProviderId,
    },
}

#[review_kit_observation::operations]
impl DiscoveryBundle {
    #[helper]
    pub fn reviewer(&self) -> &ReviewerId {
        &self.reviewer
    }
    #[helper]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    #[helper]
    pub fn changes(&self) -> &ObservedChanges {
        &self.changes
    }
    #[helper]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
    #[helper]
    pub fn foreign_replies(&self) -> &[ReplyRecord] {
        &self.foreign_replies
    }
    #[helper]
    pub fn next_finding_id(&self) -> FindingId {
        self.next_finding_id
    }
    #[helper]
    pub fn recovery_conflicts(&self) -> &[RecoveryConflict] {
        &self.recovery_conflicts
    }
}

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum BundleError {
    /// {0}
    Discovery(#[source] DiscoveryError),
    /// {0}
    Changes(#[source] PortError),
    /// changed files belong to another review subject
    WrongSubject,
    /// review revision changed while collecting discovery
    ChangedRevision,
    /// {0}
    FindingState(#[source] PlanError),
    /// no finding number remains after the current maximum
    FindingIdExhausted,
}

#[review_kit_observation::operations]
#[operation(name = "application.discovery_bundle", input = log::subject(subject), output = |result| {
            log::Completion::result(
                result,
                |value| {
                    log::Fields::new()
                        .object("snapshot", log::snapshot(value.snapshot()))
                        .count("findings", value.findings().len())
                        .count("recovery_conflicts", value.recovery_conflicts().len())
                },
                log::bundle_error,
            )
        })]
/// Collect exact change context and reconcile finding history for one reviewer.
///
/// Discovery brackets the change read; revision drift, wrong subjects, invalid
/// retained history, and exhausted finding IDs reject the bundle. Changed answer
/// targets in a current unfinished round are retained as recovery conflicts.
/// Ports can perform reads through the network; no provider writes occur.
pub fn discover_bundle(
    reader: &mut dyn ReviewReader,
    changes: &mut dyn ReviewChanges,
    subject: &Subject,
    reviewer: &ReviewerId,
) -> Result<DiscoveryBundle, BundleError> {
    let mut reader = crate::ports::ManagedReader::new(reader);
    let mut changes = ManagedChanges::new(changes);
    let initial =
        discover_managed(&mut reader, subject, reviewer).map_err(BundleError::Discovery)?;
    let observed = changes
        .read_changes(subject, &initial.revision)
        .map_err(BundleError::Changes)?;
    if &observed.subject != subject {
        return Err(BundleError::WrongSubject);
    }
    if observed.revision != initial.revision {
        return Err(BundleError::ChangedRevision);
    }
    let snapshot =
        discover_managed(&mut reader, subject, reviewer).map_err(BundleError::Discovery)?;
    if snapshot.revision != initial.revision {
        return Err(BundleError::ChangedRevision);
    }
    let mut recovery_conflicts = Vec::new();
    for summary in snapshot
        .summaries
        .iter()
        .filter(|summary| &summary.author == reviewer)
    {
        for round in summary
            .history
            .iter()
            .chain(std::iter::once(&summary.current))
        {
            validate_historical_roots(round, &snapshot).map_err(BundleError::FindingState)?;
            if round.status == RoundStatus::Completed {
                validate_completed_actions(round, &snapshot).map_err(BundleError::FindingState)?;
            }
        }
        if matches!(
            summary.current.status,
            RoundStatus::Pending | RoundStatus::Partial
        ) {
            validate_retained_structure(&summary.current, &snapshot)
                .map_err(BundleError::FindingState)?;
            if summary.current.assessment.revision() == &snapshot.revision {
                recovery_conflicts.extend(
                    changed_answer_targets(&summary.current, &snapshot)
                        .into_iter()
                        .map(|answer| RecoveryConflict::ChangedAnswerTarget {
                            finding: answer.finding,
                            target: answer.in_reply_to,
                        }),
                );
            }
        }
    }
    let mut findings = snapshot
        .findings
        .iter()
        .filter(|root| &root.author == reviewer)
        .map(|root| previous_finding(&snapshot, root, reviewer).map_err(BundleError::FindingState))
        .collect::<Result<Vec<_>, _>>()?;
    findings.sort_by_key(Finding::id);
    let highest = highest_finding_id(&snapshot, reviewer);
    let next_finding_id = match highest {
        Some(highest) => highest
            .next()
            .map_err(|_| BundleError::FindingIdExhausted)?,
        None => FindingId::try_from(1).map_err(|_| BundleError::FindingIdExhausted)?,
    };
    let foreign_replies = snapshot
        .replies
        .iter()
        .filter(|reply| {
            &reply.author != reviewer
                && snapshot
                    .findings
                    .iter()
                    .any(|root| &root.author == reviewer && root.id == reply.thread)
        })
        .cloned()
        .collect();
    Ok(DiscoveryBundle {
        reviewer: reviewer.clone(),
        snapshot,
        changes: observed,
        findings,
        foreign_replies,
        next_finding_id,
        recovery_conflicts,
    })
}
