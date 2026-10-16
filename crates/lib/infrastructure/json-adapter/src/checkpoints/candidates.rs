//! Field access on serialized candidate findings.
//!
//! Revision keeps every candidate finding's content unchanged apart from its
//! validation flag, including candidates that would fail domain conversion. It
//! therefore reads fields on the serialized record instead of converting it.

use serde_json::Value;

use crate::{JsonError, domain::FindingData};

/// A candidate's policy ID and status as written in the draft.
pub(crate) struct CandidateFields {
    pub(crate) policy_id: String,
    pub(crate) status: String,
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn fields(finding: &FindingData) -> Result<CandidateFields, JsonError> {
    let value = serde_json::to_value(finding).map_err(JsonError::Syntax)?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(JsonError::InvalidValue)
    };
    Ok(CandidateFields {
        policy_id: text("policy_id")?,
        status: text("status")?,
    })
}
