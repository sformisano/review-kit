mod state;

pub use state::State;

use review_kit_application::publication::{
    Abandonment, ActionId, Answer, Effect, PlannedAction, PolicyName, ProviderId,
    PublicPolicyDeclaration, PublicationLimitation, ReplyKind, ReplyRecord, ResolutionReceipt,
    Round, RoundStatus, TransitionReply, validate_policy_declarations,
};
use review_kit_application::{
    ports::RenderProfileIdentity,
    summary::{HistoricalFinding, PublishedAction},
};
use review_kit_domain::{FindingId, PolicyId, ReviewerId};
use serde::{Deserialize, Serialize};

use super::domain::{AssessmentData, FindingData, Status, valid};
use crate::JsonError;

wire_enum!(
    RoundState,
    RoundStatus,
    [Pending, Partial, Completed, Blocked, Superseded, Abandoned]
);
wire_enum!(
    EffectData,
    Effect,
    [
        CreateSummary,
        UpdateSummary,
        Finding,
        Reply,
        Resolve,
        Reopen
    ]
);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LimitationData {
    Authority { effect: EffectData },
    RevisionChanged {},
    ConcurrentChange {},
    ProviderUnavailable {},
    Rejected {},
    Uncertain {},
    InvalidRendering {},
    PreparationFailed {},
    EvidenceUnavailable {},
}

#[review_kit_observation::operations]
impl From<&PublicationLimitation> for LimitationData {
    #[helper]
    fn from(value: &PublicationLimitation) -> Self {
        match value {
            PublicationLimitation::Authority(effect) => Self::Authority {
                effect: (*effect).into(),
            },
            PublicationLimitation::RevisionChanged => Self::RevisionChanged {},
            PublicationLimitation::ConcurrentChange => Self::ConcurrentChange {},
            PublicationLimitation::ProviderUnavailable => Self::ProviderUnavailable {},
            PublicationLimitation::Rejected => Self::Rejected {},
            PublicationLimitation::Uncertain => Self::Uncertain {},
            PublicationLimitation::InvalidRendering => Self::InvalidRendering {},
            PublicationLimitation::PreparationFailed => Self::PreparationFailed {},
            PublicationLimitation::EvidenceUnavailable => Self::EvidenceUnavailable {},
        }
    }
}

#[review_kit_observation::operations]
impl From<LimitationData> for PublicationLimitation {
    #[helper]
    fn from(value: LimitationData) -> Self {
        match value {
            LimitationData::Authority { effect } => Self::Authority(effect.into()),
            LimitationData::RevisionChanged {} => Self::RevisionChanged,
            LimitationData::ConcurrentChange {} => Self::ConcurrentChange,
            LimitationData::ProviderUnavailable {} => Self::ProviderUnavailable,
            LimitationData::Rejected {} => Self::Rejected,
            LimitationData::Uncertain {} => Self::Uncertain,
            LimitationData::InvalidRendering {} => Self::InvalidRendering,
            LimitationData::PreparationFailed {} => Self::PreparationFailed,
            LimitationData::EvidenceUnavailable {} => Self::EvidenceUnavailable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplyKindData {
    Transition { status: Status },
    Answer { note: String },
}

#[review_kit_observation::operations]
impl From<&ReplyKind> for ReplyKindData {
    #[helper]
    fn from(value: &ReplyKind) -> Self {
        match value {
            ReplyKind::Transition(status) => Self::Transition {
                status: (*status).into(),
            },
            ReplyKind::Answer(note) => Self::Answer {
                note: note.as_str().into(),
            },
        }
    }
}

#[review_kit_observation::operations]
impl ReplyKindData {
    #[helper]
    pub fn into_kind(self) -> Result<ReplyKind, JsonError> {
        match self {
            Self::Transition { status } => Ok(ReplyKind::Transition(status.into())),
            Self::Answer { note } => Ok(ReplyKind::Answer(valid(ProviderId::new(note))?)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionIdData {
    round: u64,
    finding: u64,
    kind: ReplyKindData,
}

#[review_kit_observation::operations]
impl From<&ActionId> for ActionIdData {
    #[helper]
    fn from(value: &ActionId) -> Self {
        Self {
            round: value.round,
            finding: value.finding.get(),
            kind: (&value.kind).into(),
        }
    }
}

#[review_kit_observation::operations]
impl ActionIdData {
    #[helper]
    pub fn into_action(self) -> Result<ActionId, JsonError> {
        if self.round == 0 {
            return Err(JsonError::InvalidValue);
        }
        Ok(ActionId {
            round: self.round,
            finding: valid(FindingId::try_from(self.finding))?,
            kind: self.kind.into_kind()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyData {
    id: String,
    author: String,
    thread: String,
    action: Option<ActionIdData>,
    body: String,
}

#[review_kit_observation::operations]
impl From<&ReplyRecord> for ReplyData {
    #[helper]
    fn from(value: &ReplyRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            author: value.author.as_str().into(),
            thread: value.thread.as_str().into(),
            action: value.action.as_ref().map(ActionIdData::from),
            body: value.body.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl ReplyData {
    #[helper]
    pub fn into_reply(self) -> Result<ReplyRecord, JsonError> {
        Ok(ReplyRecord {
            id: valid(ProviderId::new(self.id))?,
            author: valid(ReviewerId::try_from(self.author))?,
            thread: valid(ProviderId::new(self.thread))?,
            action: self.action.map(ActionIdData::into_action).transpose()?,
            body: self.body,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerData {
    finding: u64,
    in_reply_to: String,
    body: String,
}

#[review_kit_observation::operations]
impl From<&Answer> for AnswerData {
    #[helper]
    fn from(value: &Answer) -> Self {
        Self {
            finding: value.finding.get(),
            in_reply_to: value.in_reply_to.as_str().into(),
            body: value.body.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl AnswerData {
    #[helper]
    pub fn finding_number(&self) -> u64 {
        self.finding
    }
    #[helper]
    pub fn into_answer(self) -> Result<Answer, JsonError> {
        nonblank(&self.body)?;
        Ok(Answer {
            finding: valid(FindingId::try_from(self.finding))?,
            in_reply_to: valid(ProviderId::new(self.in_reply_to))?,
            body: self.body,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionData {
    finding: u64,
    from: Status,
    to: Status,
    body: String,
}

#[review_kit_observation::operations]
impl From<&TransitionReply> for TransitionData {
    #[helper]
    fn from(value: &TransitionReply) -> Self {
        Self {
            finding: value.finding.get(),
            from: value.from.into(),
            to: value.to.into(),
            body: value.body.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl TransitionData {
    #[helper]
    pub fn finding_number(&self) -> u64 {
        self.finding
    }
    #[helper]
    pub fn into_transition(self) -> Result<TransitionReply, JsonError> {
        nonblank(&self.body)?;
        Ok(TransitionReply {
            finding: valid(FindingId::try_from(self.finding))?,
            from: self.from.into(),
            to: self.to.into(),
            body: self.body,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlannedData {
    Finding {
        finding: Box<FindingData>,
        body: String,
    },
    Reply {
        finding: u64,
        reply: ReplyKindData,
        body: String,
    },
    SetResolved {
        finding: u64,
        resolved: bool,
    },
}

#[review_kit_observation::operations]
impl From<&PlannedAction> for PlannedData {
    #[helper]
    fn from(value: &PlannedAction) -> Self {
        match value {
            PlannedAction::Finding { finding, body } => Self::Finding {
                finding: Box::new(finding.as_ref().into()),
                body: body.clone(),
            },
            PlannedAction::Reply {
                finding,
                kind,
                body,
            } => Self::Reply {
                finding: finding.get(),
                reply: kind.into(),
                body: body.clone(),
            },
            PlannedAction::SetResolved { finding, resolved } => Self::SetResolved {
                finding: finding.get(),
                resolved: *resolved,
            },
        }
    }
}

#[review_kit_observation::operations]
impl PlannedData {
    #[helper]
    pub fn into_planned(self) -> Result<PlannedAction, JsonError> {
        match self {
            Self::Finding { finding, body } => {
                nonblank(&body)?;
                let finding = finding.into_finding()?;
                Ok(PlannedAction::Finding {
                    finding: Box::new(finding),
                    body,
                })
            }
            Self::Reply {
                finding,
                reply,
                body,
            } => {
                nonblank(&body)?;
                Ok(PlannedAction::Reply {
                    finding: valid(FindingId::try_from(finding))?,
                    kind: reply.into_kind()?,
                    body,
                })
            }
            Self::SetResolved { finding, resolved } => Ok(PlannedAction::SetResolved {
                finding: valid(FindingId::try_from(finding))?,
                resolved,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbandonmentData {
    reason: String,
    verified_actions: Vec<PlannedData>,
}

#[review_kit_observation::operations]
impl From<&Abandonment> for AbandonmentData {
    #[helper]
    fn from(value: &Abandonment) -> Self {
        Self {
            reason: value.reason.clone(),
            verified_actions: value
                .verified_actions
                .iter()
                .map(PlannedData::from)
                .collect(),
        }
    }
}

#[review_kit_observation::operations]
impl AbandonmentData {
    #[helper]
    pub fn into_abandonment(self) -> Result<Abandonment, JsonError> {
        nonblank(&self.reason)?;
        Ok(Abandonment {
            reason: self.reason,
            verified_actions: self
                .verified_actions
                .into_iter()
                .map(PlannedData::into_planned)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundData {
    number: u64,
    assessment: AssessmentData,
    status: RoundState,
    abandonment: Option<AbandonmentData>,
    publication_limitation: Option<LimitationData>,
    answers: Vec<AnswerData>,
    transitions: Vec<TransitionData>,
    answer_targets: Vec<ReplyData>,
    actions: Vec<PlannedData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review_workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_set: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    policy_names: Vec<PolicyNameData>,
    policy_declarations: Vec<PublicPolicyDeclarationData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renderer: Option<ProfileIdentityData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    communicated_findings: Option<Vec<HistoricalFindingData>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    verified_resolutions: Vec<ResolutionReceiptData>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    confirmed_validation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicPolicyDeclarationData {
    pub policy: PolicyNameData,
    pub status: crate::checkpoints::ApplicabilityStatusData,
    pub reason: String,
}

#[review_kit_observation::operations]
impl From<&PublicPolicyDeclaration> for PublicPolicyDeclarationData {
    #[helper]
    fn from(value: &PublicPolicyDeclaration) -> Self {
        Self {
            policy: PolicyNameData {
                id: value.policy.id.as_str().into(),
                name: value.policy.name.clone(),
            },
            status: value.status.into(),
            reason: value.reason.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl PublicPolicyDeclarationData {
    #[helper]
    fn into_declaration(self) -> Result<PublicPolicyDeclaration, JsonError> {
        Ok(PublicPolicyDeclaration {
            policy: PolicyName {
                id: valid(PolicyId::try_from(self.policy.id))?,
                name: self.policy.name,
            },
            status: self.status.into(),
            reason: self.reason,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionReceiptData {
    subject: super::domain::SubjectData,
    reviewer: String,
    round: u64,
    thread: String,
    resolved: bool,
    #[serde(default)]
    provenance: ResolutionProvenanceData,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResolutionProvenanceData {
    #[default]
    Acknowledged,
    Observed,
}

#[review_kit_observation::operations]
impl From<&ResolutionReceipt> for ResolutionReceiptData {
    #[helper]
    fn from(value: &ResolutionReceipt) -> Self {
        Self {
            subject: (&value.subject).into(),
            reviewer: value.reviewer.as_str().into(),
            round: value.round,
            thread: value.thread.as_str().into(),
            resolved: value.resolved,
            provenance: match value.provenance {
                review_kit_application::publication::ResolutionProvenance::Acknowledged => {
                    ResolutionProvenanceData::Acknowledged
                }
                review_kit_application::publication::ResolutionProvenance::Observed => {
                    ResolutionProvenanceData::Observed
                }
            },
        }
    }
}

#[review_kit_observation::operations]
impl ResolutionReceiptData {
    #[helper]
    pub(crate) fn into_receipt(self) -> Result<ResolutionReceipt, JsonError> {
        if self.round == 0 {
            return Err(JsonError::InvalidValue);
        }
        Ok(ResolutionReceipt {
            subject: self.subject.into_subject()?,
            reviewer: valid(ReviewerId::try_from(self.reviewer))?,
            round: self.round,
            thread: valid(ProviderId::new(self.thread))?,
            resolved: self.resolved,
            provenance: match self.provenance {
                ResolutionProvenanceData::Acknowledged => {
                    review_kit_application::publication::ResolutionProvenance::Acknowledged
                }
                ResolutionProvenanceData::Observed => {
                    review_kit_application::publication::ResolutionProvenance::Observed
                }
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyNameData {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIdentityData {
    id: String,
    input_contract: String,
    fingerprint: String,
}

#[review_kit_observation::operations]
impl From<&RenderProfileIdentity> for ProfileIdentityData {
    #[helper]
    fn from(value: &RenderProfileIdentity) -> Self {
        Self {
            id: value.id.clone(),
            input_contract: value.input_contract.clone(),
            fingerprint: value.fingerprint.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl ProfileIdentityData {
    #[helper]
    pub(crate) fn into_identity(self) -> Result<RenderProfileIdentity, JsonError> {
        nonblank(&self.id)?;
        nonblank(&self.input_contract)?;
        nonblank(&self.fingerprint)?;
        Ok(RenderProfileIdentity {
            id: self.id,
            input_contract: self.input_contract,
            fingerprint: self.fingerprint,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishedActionData {
    Created,
    Transition { from: Status, to: Status },
    Answered { in_reply_to: String },
    Resolved,
    Reopened,
    ObservedResolved,
    ObservedReopened,
}

#[review_kit_observation::operations]
impl From<&PublishedAction> for PublishedActionData {
    #[helper]
    fn from(value: &PublishedAction) -> Self {
        match value {
            PublishedAction::Created => Self::Created,
            PublishedAction::Transition { from, to } => Self::Transition {
                from: (*from).into(),
                to: (*to).into(),
            },
            PublishedAction::Answered { in_reply_to } => Self::Answered {
                in_reply_to: in_reply_to.as_str().into(),
            },
            PublishedAction::Resolved => Self::Resolved,
            PublishedAction::Reopened => Self::Reopened,
            PublishedAction::ObservedResolved => Self::ObservedResolved,
            PublishedAction::ObservedReopened => Self::ObservedReopened,
        }
    }
}

#[review_kit_observation::operations]
impl PublishedActionData {
    #[helper]
    fn into_action(self) -> Result<PublishedAction, JsonError> {
        Ok(match self {
            Self::Created => PublishedAction::Created,
            Self::Transition { from, to } => PublishedAction::Transition {
                from: from.into(),
                to: to.into(),
            },
            Self::Answered { in_reply_to } => PublishedAction::Answered {
                in_reply_to: valid(ProviderId::new(in_reply_to))?,
            },
            Self::Resolved => PublishedAction::Resolved,
            Self::Reopened => PublishedAction::Reopened,
            Self::ObservedResolved => PublishedAction::ObservedResolved,
            Self::ObservedReopened => PublishedAction::ObservedReopened,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalFindingData {
    finding: FindingData,
    root_note: String,
    thread: String,
    actions: Vec<PublishedActionData>,
}

#[review_kit_observation::operations]
impl From<&HistoricalFinding> for HistoricalFindingData {
    #[helper]
    fn from(value: &HistoricalFinding) -> Self {
        Self {
            finding: (&value.finding).into(),
            root_note: value.root_note.as_str().into(),
            thread: value.thread.as_str().into(),
            actions: value.actions.iter().map(Into::into).collect(),
        }
    }
}

#[review_kit_observation::operations]
impl HistoricalFindingData {
    #[helper]
    fn into_finding(self) -> Result<HistoricalFinding, JsonError> {
        Ok(HistoricalFinding {
            finding: self.finding.into_finding()?,
            root_note: valid(ProviderId::new(self.root_note))?,
            thread: valid(ProviderId::new(self.thread))?,
            actions: self
                .actions
                .into_iter()
                .map(PublishedActionData::into_action)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[review_kit_observation::operations]
impl From<&Round> for RoundData {
    #[helper]
    fn from(value: &Round) -> Self {
        Self {
            number: value.number,
            assessment: (&value.assessment).into(),
            status: value.status.into(),
            abandonment: value.abandonment.as_ref().map(AbandonmentData::from),
            publication_limitation: value
                .publication_limitation
                .as_ref()
                .map(LimitationData::from),
            answers: value.answers.iter().map(AnswerData::from).collect(),
            transitions: value.transitions.iter().map(TransitionData::from).collect(),
            answer_targets: value.answer_targets.iter().map(ReplyData::from).collect(),
            actions: value.actions.iter().map(PlannedData::from).collect(),
            review_workflow: value.review_workflow.clone(),
            policy_set: value.policy_set.clone(),
            policy_declarations: value.policy_declarations.iter().map(Into::into).collect(),
            policy_names: value
                .policy_names
                .iter()
                .map(|policy| PolicyNameData {
                    id: policy.id.as_str().into(),
                    name: policy.name.clone(),
                })
                .collect(),
            renderer: value.renderer.as_ref().map(Into::into),
            communicated_findings: value
                .communicated_findings
                .as_ref()
                .map(|rows| rows.iter().map(Into::into).collect()),
            verified_resolutions: value.verified_resolutions.iter().map(Into::into).collect(),
            confirmed_validation: value.confirmed_validation,
        }
    }
}

#[review_kit_observation::operations]
impl RoundData {
    #[helper]
    pub fn into_round(self) -> Result<Round, JsonError> {
        if let Some(workflow) = &self.review_workflow {
            nonblank(workflow)?;
        }
        if let Some(set) = &self.policy_set {
            nonblank(set)?;
        }
        let round = Round {
            number: self.number,
            assessment: self.assessment.into_assessment()?,
            status: self.status.into(),
            abandonment: self
                .abandonment
                .map(AbandonmentData::into_abandonment)
                .transpose()?,
            publication_limitation: self.publication_limitation.map(PublicationLimitation::from),
            answers: self
                .answers
                .into_iter()
                .map(AnswerData::into_answer)
                .collect::<Result<_, _>>()?,
            transitions: self
                .transitions
                .into_iter()
                .map(TransitionData::into_transition)
                .collect::<Result<_, _>>()?,
            answer_targets: self
                .answer_targets
                .into_iter()
                .map(ReplyData::into_reply)
                .collect::<Result<_, _>>()?,
            actions: self
                .actions
                .into_iter()
                .map(PlannedData::into_planned)
                .collect::<Result<_, _>>()?,
            review_workflow: self.review_workflow,
            policy_set: self.policy_set,
            policy_declarations: self
                .policy_declarations
                .into_iter()
                .map(PublicPolicyDeclarationData::into_declaration)
                .collect::<Result<_, _>>()?,
            policy_names: self
                .policy_names
                .into_iter()
                .map(|policy| {
                    nonblank(&policy.name)?;
                    Ok(PolicyName {
                        id: valid(PolicyId::try_from(policy.id))?,
                        name: policy.name,
                    })
                })
                .collect::<Result<_, JsonError>>()?,
            renderer: self
                .renderer
                .map(ProfileIdentityData::into_identity)
                .transpose()?,
            communicated_findings: self
                .communicated_findings
                .map(|rows| {
                    rows.into_iter()
                        .map(HistoricalFindingData::into_finding)
                        .collect::<Result<_, _>>()
                })
                .transpose()?,
            verified_resolutions: self
                .verified_resolutions
                .into_iter()
                .map(ResolutionReceiptData::into_receipt)
                .collect::<Result<_, _>>()?,
            confirmed_validation: self.confirmed_validation,
        };
        validate_policy_declarations(
            &round.policy_declarations,
            &round.assessment,
            &round.policy_names,
        )
        .map_err(|_| JsonError::InvalidValue)?;
        Ok(round)
    }
}

#[review_kit_observation::operations]
#[helper]
pub fn validate_history(current: &Round, history: &[Round]) -> Result<(), JsonError> {
    let mut previous = 0;
    for round in history.iter().chain(std::iter::once(current)) {
        validate_policy_declarations(
            &round.policy_declarations,
            &round.assessment,
            &round.policy_names,
        )
        .map_err(|_| JsonError::InvalidValue)?;
        if round.number <= previous
            || round.assessment.subject() != current.assessment.subject()
            || round.assessment.reviewer() != current.assessment.reviewer()
            || (round.status == RoundStatus::Abandoned) != round.abandonment.is_some()
        {
            return Err(JsonError::InvalidValue);
        }
        if let Some(receipt) = &round.abandonment {
            nonblank(&receipt.reason)?;
            for (index, action) in receipt.verified_actions.iter().enumerate() {
                if !round.actions.contains(action)
                    || receipt.verified_actions[..index].contains(action)
                {
                    return Err(JsonError::InvalidValue);
                }
            }
        }
        previous = round.number;
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn nonblank(body: &str) -> Result<(), JsonError> {
    if body.trim().is_empty() {
        Err(JsonError::InvalidValue)
    } else {
        Ok(())
    }
}
