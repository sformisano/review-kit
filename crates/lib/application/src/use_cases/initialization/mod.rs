#![doc = include_str!("README.md")]

use crate::observation as log;

use super::{
    assessment::{AssessmentPolicies, AssessmentTemplate, prepare},
    discovery::DiscoveryConflict,
    discovery_bundle::DiscoveryBundle,
};
use crate::ports::PortError;

/// A prepared draft whose coverage is declared by the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializedAssessment {
    pub template: AssessmentTemplate,
}

#[review_kit_observation::operations]
impl InitializedAssessment {
    #[helper]
    pub fn policies(&self) -> &AssessmentPolicies {
        self.template.coverage()
    }
}

/// Whether the writer created a destination or preserved an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftCreation {
    Created,
    Existing,
}

/// Stores a draft without replacing the agent's existing work.
#[review_kit_observation::observed_port(wrapper = ManagedDraftWriter, ownership = "borrowed", visibility = "pub(crate)")]
pub trait AssessmentDraftWriter {
    /// Atomically create the draft only when its destination is absent.
    /// Existing returns without modifying any destination bytes.
    #[operation(name = "application.draft.create", input = log::Fields::new().count("policies", draft.policies().policies().len()), output = |result| log::Completion::result(result, |outcome| log::Fields::new().label("destination", match outcome { DraftCreation::Created => "created", DraftCreation::Existing => "existing" }), log::port_error))]
    fn create_new(&mut self, draft: &InitializedAssessment) -> Result<DraftCreation, PortError>;
}

/// Newly prepared input and the destination creation outcome.
/// When the destination exists, `draft` does not describe its stored contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializationResult {
    pub draft: InitializedAssessment,
    pub outcome: DraftCreation,
}

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum InitializationError {
    /// {0}
    Discovery(#[source] DiscoveryConflict),
    /// {0}
    Write(#[source] PortError),
}

#[review_kit_observation::operations]
#[operation(name = "application.initialization", input = log::Fields::new().count("policies", policies.policies().len()), output = |result| {
            log::Completion::result(
                result,
                |value| {
                    log::Fields::new().label(
                        "destination",
                        match value.outcome {
                            DraftCreation::Created => "created",
                            DraftCreation::Existing => "existing",
                        },
                    )
                },
                |error| match error {
                    InitializationError::Discovery(error) => log::discovery_conflict(error),
                    InitializationError::Write(error) => log::port_error(error),
                },
            )
        })]
/// Prepare fresh judgment slots for agent-declared coverage and request
/// atomic draft creation through the supplied writer.
///
/// Discovery conflicts prevent the writer call. An existing
/// destination stays unchanged under the writer contract. A writer failure returns
/// an error without claiming creation. This operation makes no provider writes.
pub fn initialize(
    bundle: &DiscoveryBundle,
    policies: AssessmentPolicies,
    writer: &mut dyn AssessmentDraftWriter,
) -> Result<InitializationResult, InitializationError> {
    let mut writer = ManagedDraftWriter::new(writer);
    let template = prepare(
        bundle.snapshot().clone(),
        bundle.reviewer().clone(),
        policies,
    )
    .map_err(InitializationError::Discovery)?;
    let draft = InitializedAssessment { template };
    let outcome = writer
        .create_new(&draft)
        .map_err(InitializationError::Write)?;
    Ok(InitializationResult { draft, outcome })
}
