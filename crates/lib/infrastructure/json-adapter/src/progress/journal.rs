//! Version 3 fields that bind publication progress to review runs.

use review_kit_application::{
    review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles},
    use_cases::publishing::{PublicationProgress, RunBinding, SettledRun},
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{JsonError, domain::valid};

/// Hashes of the four files a run captured, keyed by their run-relative names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotFilesData {
    #[serde(rename = "review-context.json")]
    pub context: String,
    #[serde(rename = "reviewed-source.json")]
    pub source: String,
    #[serde(rename = "merge-request.diff")]
    pub diff: String,
    #[serde(rename = "review-conversation.json")]
    pub conversation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBindingData {
    pub run_id: String,
    pub round: u64,
    pub review_assessment: String,
    pub snapshot_files: SnapshotFilesData,
    /// The round grant that existed before the binding was installed; absent when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_authorization: Option<super::RoundAuthorizationData>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettledOutcomeData {
    Published,
    NoChanges,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettledRunData {
    pub run_id: String,
    pub review_assessment: String,
    pub round: u64,
    pub outcome: SettledOutcomeData,
    pub reason: Option<String>,
    /// RFC 3339 observation time.
    pub verified_at: String,
    pub snapshot_files: SnapshotFilesData,
}

/// Distinguishes an explicit `null` from an absent field.
#[review_kit_observation::operations]
#[helper]
pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[review_kit_observation::operations]
impl From<&SnapshotFiles> for SnapshotFilesData {
    #[helper]
    fn from(value: &SnapshotFiles) -> Self {
        Self {
            context: value.context.as_str().into(),
            source: value.source.as_str().into(),
            diff: value.diff.as_str().into(),
            conversation: value.conversation.as_str().into(),
        }
    }
}

#[review_kit_observation::operations]
impl SnapshotFilesData {
    #[helper]
    fn into_files(self) -> Result<SnapshotFiles, JsonError> {
        Ok(SnapshotFiles {
            context: content_ref(self.context)?,
            source: content_ref(self.source)?,
            diff: content_ref(self.diff)?,
            conversation: content_ref(self.conversation)?,
        })
    }
}

#[review_kit_observation::operations]
impl From<&RunBinding> for RunBindingData {
    #[helper]
    fn from(value: &RunBinding) -> Self {
        Self {
            run_id: value.run_id.as_str().into(),
            round: value.round,
            review_assessment: value.review_assessment.as_str().into(),
            snapshot_files: (&value.snapshot_files).into(),
            prior_authorization: value.prior_authorization.as_ref().map(Into::into),
        }
    }
}

#[review_kit_observation::operations]
impl RunBindingData {
    #[helper]
    pub(super) fn into_binding(self) -> Result<RunBinding, JsonError> {
        if self.round == 0 {
            return Err(JsonError::InvalidValue);
        }
        Ok(RunBinding {
            run_id: valid(RunId::try_from(self.run_id))?,
            round: self.round,
            review_assessment: content_ref(self.review_assessment)?,
            snapshot_files: self.snapshot_files.into_files()?,
            prior_authorization: self
                .prior_authorization
                .map(super::RoundAuthorizationData::into_authorization)
                .transpose()?,
        })
    }
}

#[review_kit_observation::operations]
impl From<&SettledRun> for SettledRunData {
    #[helper]
    fn from(value: &SettledRun) -> Self {
        Self {
            run_id: value.run_id.as_str().into(),
            review_assessment: value.review_assessment.as_str().into(),
            round: value.round,
            outcome: match value.outcome {
                SettledOutcome::Published => SettledOutcomeData::Published,
                SettledOutcome::NoChanges => SettledOutcomeData::NoChanges,
                SettledOutcome::Abandoned => SettledOutcomeData::Abandoned,
            },
            reason: value.reason.clone(),
            verified_at: super::timestamp::format(value.verified_at),
            snapshot_files: (&value.snapshot_files).into(),
        }
    }
}

#[review_kit_observation::operations]
impl SettledRunData {
    #[helper]
    pub(super) fn into_settled(self) -> Result<SettledRun, JsonError> {
        let outcome = match self.outcome {
            SettledOutcomeData::Published => SettledOutcome::Published,
            SettledOutcomeData::NoChanges => SettledOutcome::NoChanges,
            SettledOutcomeData::Abandoned => SettledOutcome::Abandoned,
        };
        let blank = self
            .reason
            .as_deref()
            .is_some_and(|reason| reason.trim().is_empty());
        if self.round == 0
            || blank
            || (outcome == SettledOutcome::Abandoned && self.reason.is_none())
        {
            return Err(JsonError::InvalidValue);
        }
        Ok(SettledRun {
            run_id: valid(RunId::try_from(self.run_id))?,
            review_assessment: content_ref(self.review_assessment)?,
            snapshot_files: self.snapshot_files.into_files()?,
            round: self.round,
            outcome,
            reason: self.reason,
            verified_at: super::timestamp::parse(&self.verified_at)?,
        })
    }
}

/// Check the version 3 relations between binding, settlements and unresolved intent.
///
/// A null binding carries no unresolved write, settlement or dispatch flag; it
/// can retain a grant, consumed retries and receipts. A settled
/// current binding has no unresolved write and its entry describes that binding.
#[review_kit_observation::operations]
#[helper]
pub(super) fn validate(progress: &PublicationProgress) -> Result<(), JsonError> {
    let settled = &progress.settled_runs;
    if settled.iter().enumerate().any(|(index, entry)| {
        settled[..index]
            .iter()
            .any(|prior| prior.run_id == entry.run_id)
    }) {
        return Err(JsonError::InvalidValue);
    }
    match &progress.coverage_binding {
        None => {
            if progress.unresolved.is_some()
                || !settled.is_empty()
                || progress.dispatch_may_have_started
            {
                return Err(JsonError::InvalidValue);
            }
        }
        Some(binding) => {
            if let Some(entry) = settled.iter().find(|entry| entry.run_id == binding.run_id)
                && (progress.unresolved.is_some()
                    || entry.round != binding.round
                    || entry.review_assessment != binding.review_assessment
                    || entry.snapshot_files != binding.snapshot_files)
            {
                return Err(JsonError::InvalidValue);
            }
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn content_ref(value: String) -> Result<ContentRef, JsonError> {
    valid(ContentRef::try_from(value))
}
