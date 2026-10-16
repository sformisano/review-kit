use review_kit_domain::{ReviewerId, Subject};

use super::{PortError, WriteOutcome};
use crate::{
    observation as log,
    publication::{Effects, Snapshot, Write},
};

#[review_kit_observation::observed_port(
    wrapper = ManagedReader,
    ownership = "borrowed",
    visibility = "pub(crate)"
)]
pub trait ReviewReader {
    #[operation(
        name = "application.provider.discover",
        input = log::subject(subject),
        output = |result| log::Completion::result(result, log::snapshot, log::port_error)
    )]
    #[operation_alias(
        method = readback,
        name = "application.provider.readback",
        input = log::subject(subject),
        output = |result| log::Completion::result(result, log::snapshot, log::port_error)
    )]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError>;
}

/// Bind a publication session to the subject and credential's reviewer.
/// Acquisition does not exclude other provider writers.
#[review_kit_observation::observed_port(
    wrapper = ManagedHost,
    ownership = "borrowed",
    visibility = "pub(crate)"
)]
pub trait PublicationHost {
    #[operation(
        name = "application.provider.acquire",
        input = log::subject(subject).secret("reviewer", reviewer.as_str()),
        output = |result| log::Completion::result(result, |_| log::Fields::new(), log::port_error),
        returns = Result<ManagedSession<'_>, PortError>,
        map = |result| result.map(ManagedSession::new)
    )]
    fn acquire(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError>;
}

#[review_kit_observation::observed_port(
    wrapper = ManagedSession,
    ownership = "boxed",
    reader = ManagedReader,
    visibility = "pub(crate)"
)]
pub trait PublicationSession: ReviewReader {
    /// Actor confirmed by the provider credential used by this session.
    #[helper]
    fn authenticated_reviewer(&self) -> &ReviewerId;

    /// Current trusted host authority. It can narrow during a session.
    #[operation(
        name = "application.provider.authority",
        input = log::Fields::new(),
        output = |result| log::Completion::result(result, log::effects, log::port_error)
    )]
    fn authority(&mut self) -> Result<Effects, PortError>;

    /// Acknowledgement is not proof; every outcome is followed by discovery.
    /// Summary writes check `expected` before replacing the owned summary.
    /// The check does not promise an atomic provider comparison.
    #[helper]
    fn write(&mut self, subject: &Subject, reviewer: &ReviewerId, write: &Write) -> WriteOutcome;

    /// Call the checkpoint once before the first possible send; errors prevent sends.
    /// The default checkpoints before `write`, including its preparation.
    /// Transport adapters override this to finish preparation before checkpointing.
    #[operation(
        name = "application.provider.write",
        input = log::write(write).label("effect_state", "attempted"),
        output = log::write_outcome
    )]
    fn write_with_checkpoint(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
        write: &Write,
        before_dispatch: &mut dyn FnMut() -> Result<(), PortError>,
    ) -> WriteOutcome {
        if let Err(error) = before_dispatch() {
            return WriteOutcome::Rejected(error);
        }
        self.write(subject, reviewer, write)
    }
}
