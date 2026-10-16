use std::time::SystemTime;

use super::{
    PublicationResult, PublicationStatus,
    binding::{Admission, BindingError, BindingRequest, RunBinding},
    progress::PublicationProgress,
};
use crate::publication::{RoundStatus, Snapshot};
use crate::review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles};

/// A verified final publication outcome retained for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledRun {
    pub run_id: RunId,
    pub review_assessment: ContentRef,
    pub snapshot_files: SnapshotFiles,
    pub round: u64,
    pub outcome: SettledOutcome,
    /// Public abandonment reason; required when the outcome is `Abandoned`.
    pub reason: Option<String>,
    pub verified_at: SystemTime,
}

/// What the journal records about one run's publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPublication<'a> {
    Settled(&'a SettledRun),
    /// The run owns the journal's current, unfinished publication, and one of
    /// its writes could have reached the provider.
    BoundUnsettled(&'a RunBinding),
    /// The run owns the current binding, but none of its writes could have
    /// reached the provider. The run keeps the binding until the admission of
    /// another run's publication releases it; this state is diagnostic only.
    BoundUnsent(&'a RunBinding),
    Unbound,
}

#[review_kit_observation::operations]
impl PublicationProgress {
    /// Report whether `run_id` is settled, owns the current binding, or is unbound.
    #[helper]
    pub fn run_publication(&self, run_id: &RunId) -> RunPublication<'_> {
        if let Some(entry) = self.settled(run_id) {
            return RunPublication::Settled(entry);
        }
        match &self.coverage_binding {
            Some(binding) if &binding.run_id == run_id => {
                if self.unsent_binding().is_some() {
                    RunPublication::BoundUnsent(binding)
                } else {
                    RunPublication::BoundUnsettled(binding)
                }
            }
            _ => RunPublication::Unbound,
        }
    }

    /// Record a verified outcome for the current binding.
    ///
    /// Returns `false` when an identical entry already exists; `verified_at`
    /// does not participate in that comparison. A different entry for the same
    /// run is a conflict and leaves the journal unchanged.
    #[helper]
    pub fn record_settlement(&mut self, entry: SettledRun) -> Result<bool, BindingError> {
        if self.unresolved.is_some() {
            return Err(BindingError::Unresolved);
        }
        let blank_reason = entry
            .reason
            .as_deref()
            .is_some_and(|reason| reason.trim().is_empty());
        if blank_reason || (entry.outcome == SettledOutcome::Abandoned && entry.reason.is_none()) {
            return Err(BindingError::MissingReason);
        }
        if let Some(existing) = self.settled(&entry.run_id) {
            return if same_settlement(existing, &entry) {
                Ok(false)
            } else {
                Err(BindingError::SettlementConflict(entry.run_id))
            };
        }
        let bound = self.coverage_binding.as_ref().is_some_and(|binding| {
            binding.run_id == entry.run_id
                && binding.round == entry.round
                && binding.review_assessment == entry.review_assessment
                && binding.snapshot_files == entry.snapshot_files
        });
        if !bound {
            return Err(BindingError::Mismatch(entry.run_id));
        }
        self.settled_runs.push(entry);
        Ok(true)
    }

    /// Record the settlement implied by a finished publication or abandonment.
    ///
    /// Returns `None` when the result leaves work unfinished: its final snapshot
    /// must show the reviewer's summary at `result.round` as `Completed`,
    /// `Blocked`, `Superseded` or `Abandoned`. An abandonment settles only the run
    /// that owns the binding, because this call resumed it or installed it with
    /// the verified abandonment summary. An `Unchanged` outcome reached without
    /// any write of this run installs its binding first, so every entry describes
    /// a binding; another run's unsettled binding prevents that unless none of
    /// that run's writes could have reached the provider, in which case it is
    /// released first.
    #[helper]
    pub fn settle_publication(
        &mut self,
        request: &BindingRequest,
        result: &PublicationResult,
        verified_at: SystemTime,
    ) -> Result<Option<SettledRun>, BindingError> {
        let bound = self
            .coverage_binding
            .as_ref()
            .is_some_and(|binding| binding.run_id == request.run_id);
        let dispatched = bound && self.dispatch_may_have_started;
        let Some(outcome) = settled_outcome(result.status, dispatched) else {
            return Ok(None);
        };
        if result.status == PublicationStatus::Superseded
            && (result.finalization_stop.is_some() || result.verified.is_empty())
        {
            return Ok(None);
        }
        if self.unresolved.is_some() {
            return Err(BindingError::Unresolved);
        }
        if !observed_terminal(&result.final_snapshot, result.round) {
            return Ok(None);
        }
        let reason = match outcome {
            SettledOutcome::Abandoned => Some(
                abandonment_reason(&result.final_snapshot, result.round)
                    .ok_or(BindingError::MissingReason)?,
            ),
            SettledOutcome::Published | SettledOutcome::NoChanges => None,
        };
        let entry = SettledRun {
            run_id: request.run_id.clone(),
            review_assessment: request.review_assessment.clone(),
            snapshot_files: request.snapshot_files.clone(),
            round: result.round,
            outcome,
            reason,
            verified_at,
        };
        if let Some(existing) = self.settled(&request.run_id) {
            return if same_settlement(existing, &entry) {
                Ok(Some(existing.clone()))
            } else {
                Err(BindingError::SettlementConflict(request.run_id.clone()))
            };
        }
        if !bound && outcome == SettledOutcome::Abandoned {
            return Ok(None);
        }
        let previous = self.clone();
        let recorded = self.install_unbound(bound, request, result.round);
        match recorded.and_then(|()| self.record_settlement(entry.clone())) {
            Ok(_) => Ok(Some(entry)),
            Err(error) => {
                *self = previous;
                Err(error)
            }
        }
    }

    /// Install the binding of a run that settles without having written.
    #[helper]
    fn install_unbound(
        &mut self,
        bound: bool,
        request: &BindingRequest,
        round: u64,
    ) -> Result<(), BindingError> {
        if bound {
            return Ok(());
        }
        match self.admit(Some(request))? {
            Admission::Replaceable => {}
            Admission::ReleaseOwner(owner) => {
                if !self.release_unsent(&owner.run_id)? {
                    return Err(BindingError::PendingOwner(owner.run_id));
                }
            }
            Admission::VerifyOwner(owner) => {
                return Err(BindingError::PendingOwner(owner.run_id));
            }
            Admission::Unbound | Admission::Resume => {
                return Err(BindingError::Mismatch(request.run_id.clone()));
            }
        }
        self.coverage_binding = Some(RunBinding {
            run_id: request.run_id.clone(),
            round,
            review_assessment: request.review_assessment.clone(),
            snapshot_files: request.snapshot_files.clone(),
            prior_authorization: self.authorization.clone(),
        });
        self.dispatch_may_have_started = false;
        Ok(())
    }

    #[helper]
    pub(super) fn settled(&self, run_id: &RunId) -> Option<&SettledRun> {
        self.settled_runs
            .iter()
            .find(|entry| &entry.run_id == run_id)
    }
}

/// Entries agree on everything except their observation time.
#[review_kit_observation::operations]
#[helper]
fn same_settlement(left: &SettledRun, right: &SettledRun) -> bool {
    left.run_id == right.run_id
        && left.review_assessment == right.review_assessment
        && left.snapshot_files == right.snapshot_files
        && left.round == right.round
        && left.outcome == right.outcome
        && left.reason == right.reason
}

/// Map a publication status to the outcome a run settles with.
///
/// `dispatched` reports whether a write of this run's binding could have
/// reached the provider. Unfinished statuses return `None`.
#[review_kit_observation::operations]
#[helper]
pub fn settled_outcome(status: PublicationStatus, dispatched: bool) -> Option<SettledOutcome> {
    match status {
        PublicationStatus::Completed
        | PublicationStatus::Blocked
        | PublicationStatus::Superseded => Some(SettledOutcome::Published),
        PublicationStatus::Unchanged if dispatched => Some(SettledOutcome::Published),
        PublicationStatus::Unchanged => Some(SettledOutcome::NoChanges),
        PublicationStatus::Abandoned => Some(SettledOutcome::Abandoned),
        PublicationStatus::Partial
        | PublicationStatus::Uncertain
        | PublicationStatus::Denied
        | PublicationStatus::NotStarted => None,
    }
}

/// The reviewer's only summary shows `round` in a terminal status.
#[review_kit_observation::operations]
#[helper]
fn observed_terminal(snapshot: &Snapshot, round: u64) -> bool {
    let mut owned = snapshot
        .summaries
        .iter()
        .filter(|summary| summary.author == snapshot.reviewer);
    match (owned.next(), owned.next()) {
        (Some(summary), None) => {
            summary.current.number == round
                && matches!(
                    summary.current.status,
                    RoundStatus::Completed
                        | RoundStatus::Blocked
                        | RoundStatus::Superseded
                        | RoundStatus::Abandoned
                )
        }
        _ => false,
    }
}

#[review_kit_observation::operations]
#[helper]
fn abandonment_reason(snapshot: &Snapshot, round: u64) -> Option<String> {
    snapshot
        .summaries
        .iter()
        .filter(|summary| summary.author == snapshot.reviewer)
        .map(|summary| &summary.current)
        .find(|current| current.number == round && current.status == RoundStatus::Abandoned)
        .and_then(|current| current.abandonment.as_ref())
        .map(|closed| closed.reason.clone())
        .filter(|reason| !reason.trim().is_empty())
}
