use review_kit_domain::{ReviewerId, Subject};

use crate::publication::{Round, RoundStatus, Snapshot, SummaryRecord};

/// What the provider shows of the review state owned by one reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedHistory {
    /// No owned summary, finding root or managed reply.
    Absent,
    /// The reviewer's only summary shows this round `Completed`, `Blocked`,
    /// `Superseded`, or `Abandoned` with a public reason.
    Finished(u64),
    /// The reviewer's only summary shows this round `Pending` or `Partial`.
    Unfinished(u64),
    /// Owned state without exactly one owned summary, or a current round
    /// abandoned without a public reason.
    Ambiguous,
}

/// The provider's review state owned by the observed reviewer, as the
/// creation of an empty journal reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedHistory {
    subject: Subject,
    reviewer: ReviewerId,
    state: OwnedHistory,
}

#[review_kit_observation::operations]
impl ManagedHistory {
    /// Owned summaries, finding roots and managed replies count as history.
    #[helper]
    pub fn observe(snapshot: &Snapshot) -> Self {
        Self::from(snapshot)
    }

    #[helper]
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    #[helper]
    pub fn reviewer(&self) -> &ReviewerId {
        &self.reviewer
    }

    #[helper]
    pub fn state(&self) -> OwnedHistory {
        self.state
    }

    #[helper]
    pub fn is_absent(&self) -> bool {
        self.state == OwnedHistory::Absent
    }

    /// Whether a caller without retained progress may start from an empty journal.
    ///
    /// An empty journal asserts that no earlier write of this reviewer is
    /// unresolved. Without owned state nothing was written. A finished round
    /// leaves nothing of that round unresolved: progress holds at most one
    /// unresolved write, and the terminal summary is the round's last write.
    /// Neither case shows that no other state root holds an unresolved write;
    /// the caller keeps one authoritative state root.
    #[helper]
    pub fn admits_empty_journal(&self) -> bool {
        matches!(self.state, OwnedHistory::Absent | OwnedHistory::Finished(_))
    }
}

/// The same observation as [`ManagedHistory::observe`], for callers whose
/// production sources reserve the name `observe` for operation lifecycles.
#[review_kit_observation::operations]
impl From<&Snapshot> for ManagedHistory {
    #[helper]
    fn from(snapshot: &Snapshot) -> Self {
        let reviewer = &snapshot.reviewer;
        let present = snapshot
            .summaries
            .iter()
            .any(|summary| &summary.author == reviewer)
            || snapshot
                .findings
                .iter()
                .any(|root| &root.author == reviewer)
            || snapshot
                .replies
                .iter()
                .any(|reply| &reply.author == reviewer && reply.action.is_some());
        let state = if !present {
            OwnedHistory::Absent
        } else {
            match only_summary(snapshot, reviewer) {
                Some(summary) if finished(&summary.current) => {
                    OwnedHistory::Finished(summary.current.number)
                }
                Some(summary)
                    if matches!(
                        summary.current.status,
                        RoundStatus::Pending | RoundStatus::Partial
                    ) =>
                {
                    OwnedHistory::Unfinished(summary.current.number)
                }
                _ => OwnedHistory::Ambiguous,
            }
        };
        Self {
            subject: snapshot.subject.clone(),
            reviewer: reviewer.clone(),
            state,
        }
    }
}

/// The current round of `reviewer`'s only summary when that round is finished.
///
/// These are the terminal states trusted to settle another run's binding and
/// to start an empty journal. An abandoned round also needs its public reason.
#[review_kit_observation::operations]
#[helper]
pub(super) fn finished_round<'a>(
    snapshot: &'a Snapshot,
    reviewer: &ReviewerId,
) -> Option<&'a Round> {
    only_summary(snapshot, reviewer)
        .map(|summary| &summary.current)
        .filter(|round| finished(round))
}

#[review_kit_observation::operations]
#[helper]
fn only_summary<'a>(snapshot: &'a Snapshot, reviewer: &ReviewerId) -> Option<&'a SummaryRecord> {
    let mut owned = snapshot
        .summaries
        .iter()
        .filter(|summary| &summary.author == reviewer);
    match (owned.next(), owned.next()) {
        (Some(summary), None) => Some(summary),
        _ => None,
    }
}

#[review_kit_observation::operations]
#[helper]
fn finished(round: &Round) -> bool {
    match round.status {
        RoundStatus::Completed | RoundStatus::Blocked | RoundStatus::Superseded => true,
        RoundStatus::Abandoned => round
            .abandonment
            .as_ref()
            .is_some_and(|closed| !closed.reason.trim().is_empty()),
        RoundStatus::Pending | RoundStatus::Partial => false,
    }
}
