use review_kit_domain::{ReviewerId, Subject};

use super::{
    managed_history::finished_round,
    planning::PlanError,
    progress::{PublicationProgress, RoundAuthorization},
    settlement::SettledRun,
};
use crate::publication::{RoundStatus, Snapshot};
use crate::review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles};
use crate::use_cases::publication_run::Clock;

/// Accepted inputs that one review run submits for publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRequest {
    pub run_id: RunId,
    /// Reference to the exact accepted assessment bytes.
    pub review_assessment: ContentRef,
    pub snapshot_files: SnapshotFiles,
}

/// The review run whose accepted inputs own the journal's current publication.
/// Settlement keeps it; only a later admitted run replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunBinding {
    pub run_id: RunId,
    /// Round allocated by the plan that installed this binding.
    pub round: u64,
    pub review_assessment: ContentRef,
    pub snapshot_files: SnapshotFiles,
    /// The round grant that existed immediately before installation. Releasing
    /// a binding that never reached the provider restores it, so the unused
    /// grant of that attempt neither caps nor widens the replacing run.
    pub prior_authorization: Option<RoundAuthorization>,
}

/// The journal state refuses this binding or settlement.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum BindingError {
    /// review run {0} owns unfinished publication for this subject and reviewer
    PendingOwner(RunId),
    /// the journal binds review run {0} to different accepted inputs or another round
    Mismatch(RunId),
    /// review run {0} already has a settled publication outcome
    AlreadySettled(RunId),
    /// review run {0} already has a different settled publication outcome
    SettlementConflict(RunId),
    /// this journal binds publication to review runs; a binding request is required
    BindingRequired,
    /// a publication write is unresolved, so the binding or outcome cannot change
    Unresolved,
    /// an abandoned settlement requires a nonblank public reason
    MissingReason,
}

/// How a publication invocation relates to the journal's current binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Admission {
    /// No binding was requested and the journal has none.
    Unbound,
    /// The journal already binds the requesting run to the same inputs.
    Resume,
    /// The requesting run's binding is installed before its first prepared write.
    Replaceable,
    /// Another run's unsettled binding had no possible provider effect.
    /// It is released, without settlement, before replacement.
    ReleaseOwner(RunBinding),
    /// Another run's unsettled binding dispatched a write and has no unresolved
    /// write; only a terminal provider round for it permits replacement.
    VerifyOwner(RunBinding),
}

/// A binding request and the clock that timestamps provider-verified settlement.
pub(crate) struct BoundInvocation<'a> {
    pub(crate) request: &'a BindingRequest,
    pub(crate) clock: &'a mut dyn Clock,
}

#[review_kit_observation::operations]
impl BoundInvocation<'_> {
    /// The binding to install; installation records its prior grant.
    #[helper]
    pub(super) fn binding(&self, round: u64) -> RunBinding {
        RunBinding {
            run_id: self.request.run_id.clone(),
            round,
            review_assessment: self.request.review_assessment.clone(),
            snapshot_files: self.request.snapshot_files.clone(),
            prior_authorization: None,
        }
    }

    /// A terminal owned round matching the owner's bound round settles that owner.
    #[helper]
    fn verify_owner(
        &mut self,
        owner: &RunBinding,
        snapshot: &Snapshot,
        reviewer: &ReviewerId,
    ) -> Option<SettledRun> {
        let round =
            finished_round(snapshot, reviewer).filter(|round| round.number == owner.round)?;
        let (outcome, reason) = match &round.abandonment {
            Some(closed) if round.status == RoundStatus::Abandoned => {
                (SettledOutcome::Abandoned, Some(closed.reason.clone()))
            }
            _ => (SettledOutcome::Published, None),
        };
        Some(SettledRun {
            run_id: owner.run_id.clone(),
            review_assessment: owner.review_assessment.clone(),
            snapshot_files: owner.snapshot_files.clone(),
            round: owner.round,
            outcome,
            reason,
            verified_at: self.clock.now(),
        })
    }
}

#[review_kit_observation::operations]
impl PublicationProgress {
    /// Decide, from local journal state alone, whether `request` may publish.
    #[helper]
    pub(super) fn admit(
        &self,
        request: Option<&BindingRequest>,
    ) -> Result<Admission, BindingError> {
        let Some(request) = request else {
            return match &self.coverage_binding {
                Some(_) => Err(BindingError::BindingRequired),
                None => Ok(Admission::Unbound),
            };
        };
        if self.settled(&request.run_id).is_some() {
            return Err(BindingError::AlreadySettled(request.run_id.clone()));
        }
        match &self.coverage_binding {
            Some(binding) if binding.run_id == request.run_id => {
                if binding.review_assessment == request.review_assessment
                    && binding.snapshot_files == request.snapshot_files
                {
                    Ok(Admission::Resume)
                } else {
                    Err(BindingError::Mismatch(request.run_id.clone()))
                }
            }
            Some(binding)
                if self.settled(&binding.run_id).is_some() && self.unresolved.is_none() =>
            {
                Ok(Admission::Replaceable)
            }
            Some(binding)
                if self.settled(&binding.run_id).is_none() && self.unsent_binding().is_some() =>
            {
                Ok(Admission::ReleaseOwner(binding.clone()))
            }
            Some(binding) if self.unresolved.is_some() => {
                Err(BindingError::PendingOwner(binding.run_id.clone()))
            }
            Some(binding) => Ok(Admission::VerifyOwner(binding.clone())),
            None if self.unresolved.is_some() => Err(BindingError::Unresolved),
            None => Ok(Admission::Replaceable),
        }
    }

    /// Release an effectless owner or record its provider-verified settlement,
    /// and checkpoint the change before continuing.
    #[helper]
    pub(super) fn settle_owner(
        &mut self,
        admission: Admission,
        owner: Option<SettledRun>,
        subject: &Subject,
        reviewer: &ReviewerId,
        evidence: &mut crate::use_cases::publication_run::ManagedEvidence<'_>,
    ) -> Result<Admission, PlanError> {
        if owner.is_none() && !matches!(admission, Admission::ReleaseOwner(_)) {
            return Ok(admission);
        }
        let previous = self.clone();
        match (&admission, owner) {
            (Admission::ReleaseOwner(binding), _) => {
                if !self.release_unsent(&binding.run_id)? {
                    return Err(BindingError::PendingOwner(binding.run_id.clone()).into());
                }
            }
            (_, Some(entry)) => {
                self.record_settlement(entry)?;
            }
            (_, None) => {}
        }
        if let Err(error) = evidence.record(
            crate::use_cases::publication_run::ExecutionEvent::Progress {
                subject,
                reviewer,
                progress: self,
            },
        ) {
            *self = previous;
            return Err(PlanError::Evidence(error));
        }
        Ok(Admission::Replaceable)
    }
}

/// Settle another run's binding from provider evidence, or refuse replacement.
#[review_kit_observation::operations]
#[helper]
pub(super) fn owner_settlement(
    admission: &Admission,
    binding: Option<&mut BoundInvocation<'_>>,
    snapshot: &Snapshot,
    reviewer: &ReviewerId,
) -> Result<Option<SettledRun>, PlanError> {
    let Admission::VerifyOwner(owner) = admission else {
        return Ok(None);
    };
    binding
        .and_then(|bound| bound.verify_owner(owner, snapshot, reviewer))
        .map(Some)
        .ok_or_else(|| BindingError::PendingOwner(owner.run_id.clone()).into())
}

/// The binding to install before this invocation's first prepared write.
/// A resumed binding must still describe the planned round.
#[review_kit_observation::operations]
#[helper]
pub(super) fn installation(
    admission: &Admission,
    binding: Option<&BoundInvocation<'_>>,
    progress: &PublicationProgress,
    round: u64,
) -> Result<Option<RunBinding>, PlanError> {
    match admission {
        Admission::Unbound => Ok(None),
        Admission::Replaceable => Ok(binding.map(|bound| bound.binding(round))),
        Admission::Resume => match &progress.coverage_binding {
            Some(current) if current.round == round => Ok(None),
            Some(current) => Err(BindingError::Mismatch(current.run_id.clone()).into()),
            None => Err(BindingError::BindingRequired.into()),
        },
        Admission::ReleaseOwner(owner) | Admission::VerifyOwner(owner) => {
            Err(BindingError::PendingOwner(owner.run_id.clone()).into())
        }
    }
}
