#![doc = include_str!("README.md")]

mod abandonment;
mod action_write;
mod binding;
mod checking;
mod execution;
mod history;
mod managed_history;
mod planning;
mod progress;
mod reconciliation;
mod recovery;
mod release;
mod reply_checks;
mod settlement;
mod shape_checks;
pub use abandonment::{AbandonRequest, abandon, abandon_bound, abandon_recorded};
pub(crate) use binding::BoundInvocation;
pub use binding::{BindingError, BindingRequest, RunBinding};
pub use checking::{check_review, check_review_with_progress};
pub use managed_history::{ManagedHistory, OwnedHistory};
pub(crate) use reconciliation::{
    changed_answer_targets, previous_finding, previous_finding_through, validate_completed_actions,
    validate_historical_roots, validate_retained_structure,
};
pub use recovery::ResendBlocked;
pub use settlement::{RunPublication, SettledRun, settled_outcome};

pub use execution::{
    PublicationResult, PublicationStatus, Stop, VerifiedEffect, publish, publish_recorded,
};
pub use planning::{
    PlanError, PublishRequest, plan, plan_from_snapshot, plan_from_snapshot_with_progress,
};
pub use progress::{
    PublicationProgress, RecoveryPolicy, RoundAuthorization, UnresolvedWrite, WritePhase,
};

pub(crate) use execution::publish_recorded_managed;
pub(crate) use planning::{finding_effects_allowed, plan_from_snapshot_with_progress_managed};
