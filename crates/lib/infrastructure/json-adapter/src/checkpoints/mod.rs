//! Review-run documents: workspace selection, run manifest, one assessment checkpoint with its
//! carried-finding decisions and confirmations, acceptance results and draft revision.
//!
//! Every document rejects unknown fields. Required nullable fields must be present.
//! These conversions perform no file or network I/O; the host reads and writes bytes.

macro_rules! checkpoint_kind {
    ($name:ident, $variant:ident, $text:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            #[serde(rename = $text)]
            $variant,
        }
    };
}

mod acceptance;
mod assessment;
mod candidates;
mod diagnostics;
mod revision;
mod run;
mod verdicts;
mod wire;
mod workspace;

pub use acceptance::{AcceptanceResultDocument, AcceptanceStatus};
pub use assessment::{
    ASSESSMENT_VERSION, AssessmentDocument, AssessmentInputs, AssessmentKind, AssessmentPolicyData,
    CarriedFindingData, LOCAL_CONTEXT_LIMIT, PolicyOutcomeData, check_assessment,
};
pub use revision::{
    AssessmentRevisionInputs, Revised, RevisionError, RevisionRequest, revise_assessment,
};
pub use run::{ReviewRunDocument, RunKind, RunLifecycle, UtcTimestamp};
pub use verdicts::{ConfirmationData, ConfirmationResultData, ReassessmentData};
pub use wire::InvalidDocument;
pub use workspace::{RepositoryBinding, ReviewWorkspaceDocument, WorkspaceKind};

wire_enum!(
    ApplicabilityStatusData,
    review_kit_application::use_cases::checkpoints::ApplicabilityStatus,
    [Applicable, Excluded, Unresolved]
);

use crate::{Diagnostic, JsonError};

/// Decode draft bytes, reporting any syntax or shape failure as `malformed_draft`.
#[review_kit_observation::operations]
#[helper]
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Diagnostic> {
    crate::decode(bytes).map_err(|error| match &error {
        JsonError::Syntax(source) => diagnostics::malformed(source),
        other => Diagnostic::new("malformed_draft", &other.to_string()),
    })
}
