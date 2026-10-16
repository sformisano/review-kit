use super::{
    binding::{BindingError, RunBinding},
    progress::{PublicationProgress, WritePhase},
};
use crate::review_run::RunId;

#[review_kit_observation::operations]
impl PublicationProgress {
    /// The current binding when none of its writes could have reached the provider.
    ///
    /// The dispatch checkpoint persists `DispatchPossible` and the dispatch flag
    /// before any provider call, so a false flag with no unresolved write, or
    /// with only a `Prepared` one, proves the binding sent nothing. Settlement
    /// does not affect this fact.
    #[helper]
    pub fn unsent_binding(&self) -> Option<&RunBinding> {
        let unsent = !self.dispatch_may_have_started
            && self
                .unresolved
                .as_ref()
                .is_none_or(|pending| pending.phase == WritePhase::Prepared);
        self.coverage_binding.as_ref().filter(|_| unsent)
    }

    /// The journal as the admission of `admitted` would leave it after
    /// releasing another run's unsettled binding that sent nothing.
    ///
    /// The result is a copy for read-only use, such as planning; `self` is
    /// unchanged and nothing is persisted. `None` when the binding has a
    /// possible provider effect, is settled, is absent, or belongs to
    /// `admitted`, because an owner never releases its own binding.
    #[helper]
    pub fn released_for_admission(&self, admitted: &RunId) -> Option<Self> {
        let owner = self
            .unsent_binding()
            .filter(|binding| &binding.run_id != admitted)?
            .run_id
            .clone();
        let mut view = self.clone();
        view.release_unsent(&owner).ok()?.then_some(view)
    }

    /// Release the binding of `run_id` for the admission of another run, when
    /// none of its writes could have reached the provider.
    ///
    /// Drops its `Prepared` intent and restores the round grant that existed
    /// before its installation. The binding becomes the one described by the
    /// most recent settlement, with the dispatch flag set, or none with a clear
    /// flag when nothing is settled. Retries, receipts and settlements stay
    /// unchanged; a binding that sent nothing consumed no retry. Returns `false`
    /// without a change when `run_id` does not own the binding or one of its
    /// writes could have reached the provider; a settled run is `AlreadySettled`.
    #[helper]
    pub(crate) fn release_unsent(&mut self, run_id: &RunId) -> Result<bool, BindingError> {
        if self.settled(run_id).is_some() {
            return Err(BindingError::AlreadySettled(run_id.clone()));
        }
        let Some(released) = self
            .unsent_binding()
            .filter(|binding| &binding.run_id == run_id)
            .cloned()
        else {
            return Ok(false);
        };
        self.unresolved = None;
        self.authorization = released.prior_authorization;
        self.coverage_binding = self.settled_runs.last().map(|entry| RunBinding {
            run_id: entry.run_id.clone(),
            round: entry.round,
            review_assessment: entry.review_assessment.clone(),
            snapshot_files: entry.snapshot_files.clone(),
            prior_authorization: None,
        });
        self.dispatch_may_have_started = self.coverage_binding.is_some();
        Ok(true)
    }
}
