use crate::observation::json_error;
use review_kit_observation::{Completion, Fields};

use review_kit_application::{
    publication::{Effects, ProviderId, Write},
    use_cases::publishing::{PublicationProgress, RoundAuthorization, UnresolvedWrite, WritePhase},
};
use review_kit_domain::{ReviewerId, Subject};
use serde::{Deserialize, Serialize};

mod journal;
mod timestamp;

pub use journal::{RunBindingData, SettledOutcomeData, SettledRunData, SnapshotFilesData};

use crate::{
    JsonError, SnapshotData, SummaryData,
    domain::{FindingData, RevisionData, SubjectData, valid},
    effect_data,
    rounds::{
        ActionIdData, EffectData, ProfileIdentityData, ResolutionReceiptData, RoundData,
        validate_history,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WriteData {
    Summary {
        expected: Option<Box<SummaryData>>,
        round: Box<RoundData>,
        history: Vec<RoundData>,
        body: String,
    },
    Finding {
        round: u64,
        revision: RevisionData,
        finding: Box<FindingData>,
        body: String,
    },
    Reply {
        thread: String,
        action: ActionIdData,
        body: String,
    },
    SetResolved {
        thread: String,
        resolved: bool,
    },
}

#[review_kit_observation::operations]
impl From<&Write> for WriteData {
    #[helper]
    fn from(value: &Write) -> Self {
        match value {
            Write::Summary {
                expected,
                round,
                history,
                body,
            } => Self::Summary {
                expected: expected.as_deref().map(|value| Box::new(value.into())),
                round: Box::new(round.as_ref().into()),
                history: history.iter().map(Into::into).collect(),
                body: body.clone(),
            },
            Write::Finding {
                round,
                revision,
                finding,
                body,
            } => Self::Finding {
                round: *round,
                revision: revision.into(),
                finding: Box::new(finding.as_ref().into()),
                body: body.clone(),
            },
            Write::Reply {
                thread,
                action,
                body,
            } => Self::Reply {
                thread: thread.as_str().into(),
                action: action.into(),
                body: body.clone(),
            },
            Write::SetResolved { thread, resolved } => Self::SetResolved {
                thread: thread.as_str().into(),
                resolved: *resolved,
            },
        }
    }
}

#[review_kit_observation::operations]
impl WriteData {
    #[helper]
    pub fn into_write(self) -> Result<Write, JsonError> {
        Ok(match self {
            Self::Summary {
                expected,
                round,
                history,
                body,
            } => {
                let round = round.into_round()?;
                let history = history
                    .into_iter()
                    .map(RoundData::into_round)
                    .collect::<Result<Vec<_>, _>>()?;
                validate_history(&round, &history)?;
                Write::Summary {
                    expected: expected
                        .map(|value| value.into_summary().map(Box::new))
                        .transpose()?,
                    round: Box::new(round),
                    history,
                    body,
                }
            }
            Self::Finding {
                round,
                revision,
                finding,
                body,
            } => {
                if round == 0 {
                    return Err(JsonError::InvalidValue);
                }
                Write::Finding {
                    round,
                    revision: revision.into_revision()?,
                    finding: Box::new(finding.into_finding()?),
                    body,
                }
            }
            Self::Reply {
                thread,
                action,
                body,
            } => Write::Reply {
                thread: valid(ProviderId::new(thread))?,
                action: action.into_action()?,
                body,
            },
            Self::SetResolved { thread, resolved } => Write::SetResolved {
                thread: valid(ProviderId::new(thread))?,
                resolved,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnresolvedData {
    pub subject: SubjectData,
    pub reviewer: String,
    pub write: WriteData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<ProfileIdentityData>,
    #[serde(default)]
    pub phase: WritePhaseData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<SnapshotData>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePhaseData {
    Prepared,
    DispatchPossible,
    #[default]
    LegacyAmbiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundAuthorizationData {
    pub round: u64,
    pub effects: Vec<EffectData>,
}

#[review_kit_observation::operations]
impl From<&RoundAuthorization> for RoundAuthorizationData {
    #[helper]
    fn from(grant: &RoundAuthorization) -> Self {
        Self {
            round: grant.round,
            effects: effect_data(&grant.effects),
        }
    }
}

#[review_kit_observation::operations]
impl RoundAuthorizationData {
    /// A grant names round 1 or later.
    #[helper]
    pub(crate) fn into_authorization(self) -> Result<RoundAuthorization, JsonError> {
        if self.round == 0 {
            return Err(JsonError::InvalidValue);
        }
        Ok(RoundAuthorization {
            round: self.round,
            effects: Effects::new(self.effects.into_iter().map(Into::into)),
        })
    }
}

document_kind!(ProgressKind, Progress, "progress");

/// Publication progress. Versions 1 and 2 are the legacy `progress.json`
/// journal; version 3 adds the review-run binding fields, which are absent from
/// earlier versions and required in version 3.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressDocument {
    pub version: u64,
    pub kind: ProgressKind,
    pub subject: SubjectData,
    pub reviewer: String,
    pub unresolved: Option<UnresolvedData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified_resolutions: Vec<ResolutionReceiptData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<RoundAuthorizationData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retried_creations: Vec<WriteData>,
    /// Outer `None` means absent; `Some(None)` is an explicit null binding.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "journal::present"
    )]
    pub coverage_binding: Option<Option<RunBindingData>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_runs: Option<Vec<SettledRunData>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_may_have_started: Option<bool>,
}

#[review_kit_observation::operations]
impl ProgressDocument {
    /// Write version 2 unless the progress carries run-binding state, which
    /// only version 3 can represent.
    #[helper]
    pub fn new(subject: &Subject, reviewer: &ReviewerId, progress: &PublicationProgress) -> Self {
        if progress.coverage_binding.is_some()
            || !progress.settled_runs.is_empty()
            || progress.dispatch_may_have_started
        {
            return Self::journal(subject, reviewer, progress);
        }
        Self::encode(2, subject, reviewer, progress)
    }

    /// Write version 3 for `publication-progress.json`.
    #[helper]
    pub fn journal(
        subject: &Subject,
        reviewer: &ReviewerId,
        progress: &PublicationProgress,
    ) -> Self {
        let mut document = Self::encode(3, subject, reviewer, progress);
        document.coverage_binding = Some(progress.coverage_binding.as_ref().map(Into::into));
        document.settled_runs = Some(progress.settled_runs.iter().map(Into::into).collect());
        document.dispatch_may_have_started = Some(progress.dispatch_may_have_started);
        document
    }

    #[helper]
    fn encode(
        version: u64,
        subject: &Subject,
        reviewer: &ReviewerId,
        progress: &PublicationProgress,
    ) -> Self {
        Self {
            version,
            kind: ProgressKind::Progress,
            subject: subject.into(),
            reviewer: reviewer.as_str().into(),
            unresolved: progress.unresolved.as_ref().map(|value| UnresolvedData {
                subject: (&value.subject).into(),
                reviewer: value.reviewer.as_str().into(),
                write: (&value.write).into(),
                renderer: value.renderer.as_ref().map(Into::into),
                phase: match value.phase {
                    WritePhase::Prepared => WritePhaseData::Prepared,
                    WritePhase::DispatchPossible => WritePhaseData::DispatchPossible,
                    WritePhase::LegacyAmbiguous => WritePhaseData::LegacyAmbiguous,
                },
                baseline: value.baseline.as_ref().map(Into::into),
            }),
            verified_resolutions: progress
                .verified_resolutions
                .iter()
                .map(Into::into)
                .collect(),
            authorization: progress.authorization.as_ref().map(Into::into),
            retried_creations: progress.retried_creations.iter().map(Into::into).collect(),
            coverage_binding: None,
            settled_runs: None,
            dispatch_may_have_started: None,
        }
    }

    /// Restore a legacy version 1 or 2 document.
    #[operation(
        name = "json.restore_progress",
        input = Fields::new()
        .flag("unresolved", self.unresolved.is_some())
        .count("verified_resolutions", self.verified_resolutions.len()),
        output = |result| {
            Completion::result(
                result,
                |progress| {
                    Fields::new()
                        .flag("unresolved", progress.unresolved.is_some())
                        .count("verified_resolutions", progress.verified_resolutions.len())
                },
                json_error,
            )
        },
    )]
    pub fn into_progress(
        self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<PublicationProgress, JsonError> {
        if !matches!(self.version, 1 | 2) {
            return Err(JsonError::Version(self.version));
        }
        if self.coverage_binding.is_some()
            || self.settled_runs.is_some()
            || self.dispatch_may_have_started.is_some()
        {
            return Err(JsonError::InvalidValue);
        }
        self.restore(subject, reviewer)
    }

    /// Restore a version 3 journal with its run binding and settlements.
    #[operation(
        name = "json.restore_journal",
        input = Fields::new()
        .flag("unresolved", self.unresolved.is_some())
        .flag("bound", matches!(self.coverage_binding, Some(Some(_))))
        .count("settled_runs", self.settled_runs.as_ref().map_or(0, Vec::len)),
        output = |result| {
            Completion::result(
                result,
                |progress| {
                    Fields::new()
                        .flag("unresolved", progress.unresolved.is_some())
                        .flag("bound", progress.coverage_binding.is_some())
                        .count("settled_runs", progress.settled_runs.len())
                        .flag("dispatch_may_have_started", progress.dispatch_may_have_started)
                },
                json_error,
            )
        },
    )]
    pub fn into_journal(
        mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<PublicationProgress, JsonError> {
        if self.version != 3 {
            return Err(JsonError::Version(self.version));
        }
        let (Some(binding), Some(settled), Some(dispatched)) = (
            self.coverage_binding.take(),
            self.settled_runs.take(),
            self.dispatch_may_have_started.take(),
        ) else {
            return Err(JsonError::InvalidValue);
        };
        let mut progress = self.restore(subject, reviewer)?;
        progress.coverage_binding = binding.map(RunBindingData::into_binding).transpose()?;
        progress.settled_runs = settled
            .into_iter()
            .map(SettledRunData::into_settled)
            .collect::<Result<_, _>>()?;
        progress.dispatch_may_have_started = dispatched;
        journal::validate(&progress)?;
        Ok(progress)
    }

    #[helper]
    fn restore(
        self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<PublicationProgress, JsonError> {
        if self.version == 1
            && (self.authorization.is_some()
                || !self.retried_creations.is_empty()
                || self.unresolved.as_ref().is_some_and(|pending| {
                    !matches!(pending.phase, WritePhaseData::LegacyAmbiguous)
                        || pending.baseline.is_some()
                }))
        {
            return Err(JsonError::InvalidValue);
        }
        if self.subject.into_subject()? != *subject || self.reviewer != reviewer.as_str() {
            return Err(JsonError::ContextBinding);
        }
        let unresolved = self
            .unresolved
            .map(|value| {
                let bound_subject = value.subject.into_subject()?;
                let bound_reviewer = valid(ReviewerId::try_from(value.reviewer))?;
                if bound_subject != *subject || bound_reviewer != *reviewer {
                    return Err(JsonError::ContextBinding);
                }
                let write = value.write.into_write()?;
                if let Write::Summary { round, .. } = &write
                    && (round.assessment.subject() != subject
                        || round.assessment.reviewer() != reviewer)
                {
                    return Err(JsonError::ContextBinding);
                }
                Ok(UnresolvedWrite {
                    subject: bound_subject,
                    reviewer: bound_reviewer,
                    write,
                    renderer: value
                        .renderer
                        .map(ProfileIdentityData::into_identity)
                        .transpose()?,
                    phase: match value.phase {
                        WritePhaseData::Prepared => WritePhase::Prepared,
                        WritePhaseData::DispatchPossible => WritePhase::DispatchPossible,
                        WritePhaseData::LegacyAmbiguous => WritePhase::LegacyAmbiguous,
                    },
                    baseline: value
                        .baseline
                        .map(SnapshotData::into_snapshot)
                        .transpose()?,
                })
            })
            .transpose()?;
        let verified_resolutions = self
            .verified_resolutions
            .into_iter()
            .map(ResolutionReceiptData::into_receipt)
            .collect::<Result<Vec<_>, _>>()?;
        if verified_resolutions
            .iter()
            .enumerate()
            .any(|(index, receipt)| {
                &receipt.subject != subject
                    || &receipt.reviewer != reviewer
                    || verified_resolutions[..index]
                        .iter()
                        .any(|prior| prior.round == receipt.round && prior.thread == receipt.thread)
            })
        {
            return Err(JsonError::ContextBinding);
        }
        if self
            .authorization
            .as_ref()
            .is_some_and(|grant| grant.round == 0)
        {
            return Err(JsonError::InvalidValue);
        }
        if let Some(pending) = &unresolved {
            if let Some(baseline) = &pending.baseline {
                if &baseline.subject != subject || &baseline.reviewer != reviewer {
                    return Err(JsonError::ContextBinding);
                }
                review_kit_application::use_cases::discovery::discover(
                    &mut RetainedSnapshot(baseline),
                    subject,
                    reviewer,
                )
                .map_err(|_| JsonError::InvalidValue)?;
            }
            if pending.phase != WritePhase::LegacyAmbiguous
                && (pending.baseline.is_none()
                    || (self.authorization.is_none() && pending.abandonment_round().is_none()))
            {
                return Err(JsonError::InvalidValue);
            }
        }
        let retried_creations = self
            .retried_creations
            .into_iter()
            .map(WriteData::into_write)
            .collect::<Result<Vec<_>, _>>()?;
        if retried_creations.iter().enumerate().any(|(index, write)| {
            !matches!(
                write,
                Write::Summary { expected: None, .. } | Write::Finding { .. } | Write::Reply { .. }
            ) || retried_creations[..index].contains(write)
        }) {
            return Err(JsonError::InvalidValue);
        }
        let authorization = self.authorization.map(|grant| RoundAuthorization {
            round: grant.round,
            effects: Effects::new(grant.effects.into_iter().map(Into::into)),
        });
        if let Some(pending) = &unresolved
            && pending.phase != WritePhase::LegacyAmbiguous
            && !pending.abandonment_round().map_or_else(
                || {
                    authorization
                        .as_ref()
                        .is_some_and(|grant| pending.is_bound_to_round(grant))
                },
                |round| {
                    pending.is_bound_to_round(&RoundAuthorization {
                        round,
                        effects: Effects::none(),
                    })
                },
            )
        {
            return Err(JsonError::ContextBinding);
        }
        Ok(PublicationProgress {
            unresolved,
            verified_resolutions,
            authorization,
            retried_creations,
            recovery_policy: Default::default(),
            coverage_binding: None,
            settled_runs: Vec::new(),
            dispatch_may_have_started: false,
        })
    }
}

struct RetainedSnapshot<'a>(&'a review_kit_application::publication::Snapshot);

#[review_kit_observation::operations]
impl review_kit_application::ports::ReviewReader for RetainedSnapshot<'_> {
    #[helper]
    fn discover(
        &mut self,
        _: &Subject,
    ) -> Result<
        review_kit_application::publication::Snapshot,
        review_kit_application::ports::PortError,
    > {
        Ok(self.0.clone())
    }
}
