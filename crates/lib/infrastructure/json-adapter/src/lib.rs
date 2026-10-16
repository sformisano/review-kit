//! Versioned local review documents with validated conversions into the core.

use crate::observation::json_error;
use review_kit_observation::{Completion, Fields};

macro_rules! wire_enum {
    ($name:ident, $core:path, [$($variant:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        #[review_kit_observation::operations]
        impl From<$core> for $name {
            #[helper]
            fn from(value: $core) -> Self {
                match value { $(<$core>::$variant => Self::$variant),+ }
            }
        }
        #[review_kit_observation::operations]
        impl From<$name> for $core {
            #[helper]
            fn from(value: $name) -> Self {
                match value { $($name::$variant => Self::$variant),+ }
            }
        }
    };
}

macro_rules! document_kind {
    ($name:ident, $variant:ident, $text:literal) => {
        #[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            #[serde(rename = $text)]
            $variant,
        }
    };
}

pub mod checkpoints;
mod context;
mod conversation;
mod diagnostics;
pub mod domain;
mod draft;
mod observation;
mod progress;
mod reporting;
pub mod rounds;

pub use context::*;
pub use conversation::*;
pub use diagnostics::*;
pub use draft::*;
pub use progress::*;
pub use reporting::*;

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum JsonError {
    /// invalid JSON document
    Syntax(#[source] serde_json::Error),
    /// unsupported review document version {0}
    Version(u64),
    /// retained state exceeds its safe restoration limits
    RetainedStateLimit,
    /// document contains an invalid review value
    InvalidValue,
    /// document contains an invalid finding
    Finding(#[source] review_kit_domain::FindingError),
    /// document context differs from the assessment binding
    ContextBinding,
    /// document contains invalid retained review state
    Discovery(#[source] review_kit_application::use_cases::discovery_bundle::BundleError),
    /// cannot prepare assessment from retained context
    Preparation(#[source] review_kit_application::use_cases::discovery::DiscoveryConflict),
    /// assessment policy metadata is invalid
    Policies(#[source] review_kit_application::use_cases::assessment::InvalidAssessmentPolicies),
    /// assessment contains incomplete or invalid judgments
    Draft(Vec<Diagnostic>),
}

#[review_kit_observation::operations]
#[helper]
pub fn validate_version(version: u64) -> Result<(), JsonError> {
    if version == 1 {
        Ok(())
    } else {
        Err(JsonError::Version(version))
    }
}

#[review_kit_observation::operations]
#[operation(
    name = "json.decode",
    input = Fields::new().count("bytes", bytes.len()),
    output = |result| Completion::result(result, |_| Fields::new(), json_error),
)]
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, JsonError> {
    serde_json::from_slice(bytes).map_err(JsonError::Syntax)
}

#[review_kit_observation::operations]
#[operation(
    name = "json.encode",
    input = Fields::new(),
    output = |result| {
        Completion::result(
            result,
            |bytes| Fields::new().count("bytes", bytes.len()),
            json_error,
        )
    },
)]
pub fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>, JsonError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(JsonError::Syntax)?;
    bytes.push(b'\n');
    Ok(bytes)
}
