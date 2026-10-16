#![doc = include_str!("README.md")]

use crate::observation as log;

pub use managed::discover;
pub(crate) use managed::{discover_managed, highest_finding_id, validate_snapshot};

use std::collections::BTreeSet;

use review_kit_domain::{FindingId, ReviewerId, Subject};

use crate::{
    ports::{ManagedReader, PortError, ReviewReader},
    publication::{PlannedAction, Snapshot},
};

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum DiscoveryError {
    /// {0}
    Read(#[source] PortError),
    /// {0}
    Conflict(#[source] DiscoveryConflict),
}

#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum DiscoveryConflict {
    /// discovery conflict: {self:?}
    WrongSubject,
    /// discovery conflict: {self:?}
    WrongReviewer,
    /// discovery conflict: {self:?}
    InvalidFindingState(FindingId),
    /// discovery conflict: {self:?}
    MissingPublishedFinding(FindingId),
    /// discovery conflict: {self:?}
    ConflictingReply(FindingId),
    /// discovery conflict: {self:?}
    MultipleSummaries,
    /// discovery conflict: {self:?}
    DuplicateFinding(FindingId),
    /// discovery conflict: {self:?}
    DuplicateProviderId,
    /// discovery conflict: {self:?}
    InvalidRound,
    /// discovery conflict: {self:?}
    ForeignRound,
}

#[review_kit_observation::operations]
mod managed {
    use super::*;

    #[helper]
    pub(crate) fn highest_finding_id(
        snapshot: &Snapshot,
        reviewer: &ReviewerId,
    ) -> Option<FindingId> {
        snapshot
            .findings
            .iter()
            .filter(|root| &root.author == reviewer)
            .map(|root| root.finding.id())
            .chain(
                snapshot
                    .summaries
                    .iter()
                    .filter(|summary| &summary.author == reviewer)
                    .flat_map(|summary| {
                        summary
                            .history
                            .iter()
                            .chain(std::iter::once(&summary.current))
                    })
                    .flat_map(|round| {
                        round
                            .assessment
                            .findings()
                            .iter()
                            .map(|finding| finding.id())
                            .chain(round.actions.iter().map(|action| match action {
                                PlannedAction::Finding { finding, .. } => finding.id(),
                                PlannedAction::Reply { finding, .. }
                                | PlannedAction::SetResolved { finding, .. } => *finding,
                            }))
                    }),
            )
            .chain(
                snapshot
                    .replies
                    .iter()
                    .filter(|reply| &reply.author == reviewer)
                    .filter_map(|reply| reply.action.as_ref().map(|action| action.finding)),
            )
            .max()
    }

    /// Read observations and verify the requested subject, authenticated reviewer,
    /// owned identities, and summary round structure before returning a snapshot.
    ///
    /// Reader failures and structural conflicts remain distinct. No writes occur.
    /// This operation preserves observed records; reconciled finding judgments and
    /// changed-file context belong to `discovery_bundle::discover_bundle`.
    #[helper]
    pub fn discover(
        reader: &mut dyn ReviewReader,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Snapshot, DiscoveryError> {
        let mut reader = ManagedReader::new(reader);
        discover_managed(&mut reader, subject, reviewer)
    }

    #[operation(
        name = "application.discovery",
        input = log::subject(subject),
        output = |result| log::Completion::result(result, log::snapshot, log::discovery_error)
    )]
    pub(crate) fn discover_managed(
        reader: &mut ManagedReader<'_>,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Snapshot, DiscoveryError> {
        let snapshot = reader.discover(subject).map_err(DiscoveryError::Read)?;
        validate_snapshot(&snapshot, subject, reviewer).map_err(DiscoveryError::Conflict)?;
        Ok(snapshot)
    }

    #[operation(
        name = "application.validation.snapshot",
        input = log::snapshot(snapshot),
        output = |result| log::Completion::result(result, |_| log::Fields::new(), log::discovery_conflict)
    )]
    pub(crate) fn validate_snapshot(
        snapshot: &Snapshot,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<(), DiscoveryConflict> {
        if &snapshot.subject != subject {
            return Err(DiscoveryConflict::WrongSubject);
        }
        if &snapshot.reviewer != reviewer {
            return Err(DiscoveryConflict::WrongReviewer);
        }
        let mut provider_ids = BTreeSet::new();
        let owned_summaries: Vec<_> = snapshot
            .summaries
            .iter()
            .filter(|s| &s.author == reviewer)
            .collect();
        if owned_summaries.len() > 1 {
            return Err(DiscoveryConflict::MultipleSummaries);
        }
        for summary in owned_summaries {
            if !provider_ids.insert(&summary.id) {
                return Err(DiscoveryConflict::DuplicateProviderId);
            }
            let mut previous = 0;
            for round in summary
                .history
                .iter()
                .chain(std::iter::once(&summary.current))
            {
                if round.number <= previous {
                    return Err(DiscoveryConflict::InvalidRound);
                }
                previous = round.number;
                if round.assessment.subject() != subject || round.assessment.reviewer() != reviewer
                {
                    return Err(DiscoveryConflict::ForeignRound);
                }
            }
        }
        let mut finding_ids = BTreeSet::new();
        let mut thread_ids = BTreeSet::new();
        for root in snapshot.findings.iter().filter(|f| &f.author == reviewer) {
            if root.created_in == 0 {
                return Err(DiscoveryConflict::InvalidRound);
            }
            if !finding_ids.insert(root.finding.id()) {
                return Err(DiscoveryConflict::DuplicateFinding(root.finding.id()));
            }
            if !thread_ids.insert(&root.id) || !provider_ids.insert(&root.root_note) {
                return Err(DiscoveryConflict::DuplicateProviderId);
            }
        }
        for reply in snapshot.replies.iter().filter(|r| &r.author == reviewer) {
            if reply
                .action
                .as_ref()
                .is_some_and(|action| action.round == 0)
            {
                return Err(DiscoveryConflict::InvalidRound);
            }
            if !provider_ids.insert(&reply.id) {
                return Err(DiscoveryConflict::DuplicateProviderId);
            }
        }
        crate::summary::validate_retained_snapshots(snapshot, reviewer)?;
        Ok(())
    }
}
