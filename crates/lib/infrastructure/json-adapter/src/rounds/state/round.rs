use review_kit_application::publication::{
    Abandonment, PlannedAction, ProviderId, PublicPolicyDeclaration, ResolutionProvenance,
    ResolutionReceipt, Round,
};
use review_kit_application::summary::HistoricalFinding;
use review_kit_domain::{Assessment, AssessmentDraft, ReviewerId, Subject};
use serde::{Deserialize, Serialize};

use super::tables::{Tables, ValidatedTables};
use crate::{
    JsonError,
    domain::{Decision, EvaluationData, RevisionData, valid},
    rounds::{
        AnswerData, LimitationData, ProfileIdentityData, PublishedActionData, ReplyData,
        ReplyKindData, ResolutionProvenanceData, RoundState, TransitionData,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactAssessment {
    revision: RevisionData,
    decision: Decision,
    depth: String,
    scope: String,
    limitation: Option<String>,
    selected_policies: Vec<String>,
    evaluations: Vec<EvaluationData>,
    findings: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CompactAction {
    Finding {
        finding: u64,
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
impl CompactAction {
    #[helper]
    fn from_action(action: &PlannedAction, tables: &mut Tables) -> Result<Self, JsonError> {
        Ok(match action {
            PlannedAction::Finding { finding, body } => Self::Finding {
                finding: tables.finding(finding)?,
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
        })
    }

    #[helper]
    fn finding_reference(&self) -> Option<u64> {
        match self {
            Self::Finding { finding, .. } => Some(*finding),
            _ => None,
        }
    }

    #[helper]
    fn into_action(self, tables: &ValidatedTables) -> Result<PlannedAction, JsonError> {
        Ok(match self {
            Self::Finding { finding, body } => {
                crate::rounds::nonblank(&body)?;
                PlannedAction::Finding {
                    finding: Box::new(tables.finding(finding)?),
                    body,
                }
            }
            Self::Reply {
                finding,
                reply,
                body,
            } => {
                crate::rounds::nonblank(&body)?;
                PlannedAction::Reply {
                    finding: valid(finding.try_into())?,
                    kind: reply.into_kind()?,
                    body,
                }
            }
            Self::SetResolved { finding, resolved } => PlannedAction::SetResolved {
                finding: valid(finding.try_into())?,
                resolved,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactAbandonment {
    reason: String,
    verified_actions: Vec<CompactAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactHistoricalFinding {
    finding: u64,
    root_note: String,
    thread: String,
    actions: Vec<PublishedActionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactResolution {
    thread: String,
    resolved: bool,
    provenance: ResolutionProvenanceData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactPolicyDeclaration {
    policy: u64,
    status: crate::checkpoints::ApplicabilityStatusData,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompactRound {
    number: u64,
    assessment: CompactAssessment,
    status: RoundState,
    abandonment: Option<CompactAbandonment>,
    publication_limitation: Option<LimitationData>,
    answers: Vec<AnswerData>,
    transitions: Vec<TransitionData>,
    answer_targets: Vec<ReplyData>,
    actions: Vec<CompactAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review_workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_set: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_names: Vec<u64>,
    policy_declarations: Vec<CompactPolicyDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    renderer: Option<ProfileIdentityData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    communicated_findings: Option<Vec<CompactHistoricalFinding>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    verified_resolutions: Vec<CompactResolution>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    confirmed_validation: bool,
}

#[review_kit_observation::operations]
impl CompactRound {
    #[helper]
    pub fn from_round(round: &Round, tables: &mut Tables) -> Result<Self, JsonError> {
        let assessment = &round.assessment;
        let findings = assessment
            .findings()
            .iter()
            .map(|finding| tables.finding(finding))
            .collect::<Result<_, _>>()?;
        let actions = round
            .actions
            .iter()
            .map(|action| CompactAction::from_action(action, tables))
            .collect::<Result<_, _>>()?;
        let communicated_findings = round
            .communicated_findings
            .as_ref()
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        Ok(CompactHistoricalFinding {
                            finding: tables.finding(&row.finding)?,
                            root_note: row.root_note.as_str().into(),
                            thread: row.thread.as_str().into(),
                            actions: row.actions.iter().map(Into::into).collect(),
                        })
                    })
                    .collect::<Result<_, JsonError>>()
            })
            .transpose()?;
        let abandonment = round
            .abandonment
            .as_ref()
            .map(|receipt| {
                Ok(CompactAbandonment {
                    reason: receipt.reason.clone(),
                    verified_actions: receipt
                        .verified_actions
                        .iter()
                        .map(|action| CompactAction::from_action(action, tables))
                        .collect::<Result<_, JsonError>>()?,
                })
            })
            .transpose()?;
        Ok(Self {
            number: round.number,
            assessment: CompactAssessment {
                revision: assessment.revision().into(),
                decision: assessment.decision().into(),
                depth: assessment.depth().into(),
                scope: assessment.scope().into(),
                limitation: assessment.limitation().map(str::to_owned),
                selected_policies: assessment
                    .selected_policies()
                    .iter()
                    .map(|policy| policy.as_str().to_owned())
                    .collect(),
                evaluations: assessment.evaluations().iter().map(Into::into).collect(),
                findings,
            },
            status: round.status.into(),
            abandonment,
            publication_limitation: round.publication_limitation.as_ref().map(Into::into),
            answers: round.answers.iter().map(Into::into).collect(),
            transitions: round.transitions.iter().map(Into::into).collect(),
            answer_targets: round.answer_targets.iter().map(Into::into).collect(),
            actions,
            review_workflow: round.review_workflow.clone(),
            policy_set: round.policy_set.clone(),
            policy_names: round
                .policy_names
                .iter()
                .map(|name| tables.policy_name(name))
                .collect::<Result<_, _>>()?,
            policy_declarations: round
                .policy_declarations
                .iter()
                .map(|row| {
                    Ok(CompactPolicyDeclaration {
                        policy: tables.policy_name(&row.policy)?,
                        status: row.status.into(),
                        reason: row.reason.clone(),
                    })
                })
                .collect::<Result<_, JsonError>>()?,
            renderer: round.renderer.as_ref().map(Into::into),
            communicated_findings,
            verified_resolutions: round
                .verified_resolutions
                .iter()
                .map(|receipt| CompactResolution {
                    thread: receipt.thread.as_str().into(),
                    resolved: receipt.resolved,
                    provenance: match receipt.provenance {
                        ResolutionProvenance::Acknowledged => {
                            ResolutionProvenanceData::Acknowledged
                        }
                        ResolutionProvenance::Observed => ResolutionProvenanceData::Observed,
                    },
                })
                .collect(),
            confirmed_validation: round.confirmed_validation,
        })
    }

    #[helper]
    pub fn finding_references(&self) -> impl Iterator<Item = u64> + '_ {
        self.assessment
            .findings
            .iter()
            .copied()
            .chain(
                self.actions
                    .iter()
                    .filter_map(CompactAction::finding_reference),
            )
            .chain(
                self.communicated_findings
                    .iter()
                    .flatten()
                    .map(|row| row.finding),
            )
            .chain(
                self.abandonment
                    .iter()
                    .flat_map(|receipt| &receipt.verified_actions)
                    .filter_map(CompactAction::finding_reference),
            )
    }

    #[helper]
    pub fn resolution_count(&self) -> usize {
        self.verified_resolutions.len()
    }

    #[helper]
    pub fn policy_name_references(&self) -> impl Iterator<Item = u64> + '_ {
        self.policy_names
            .iter()
            .copied()
            .chain(self.policy_declarations.iter().map(|row| row.policy))
    }

    #[helper]
    pub fn into_round(
        self,
        subject: &Subject,
        reviewer: &ReviewerId,
        tables: &ValidatedTables,
    ) -> Result<Round, JsonError> {
        for name in self.review_workflow.iter().chain(&self.policy_set) {
            crate::rounds::nonblank(name)?;
        }
        let assessment = self.assessment;
        Ok(Round {
            number: self.number,
            assessment: valid(Assessment::new(AssessmentDraft {
                subject: subject.clone(),
                reviewer: reviewer.clone(),
                revision: assessment.revision.into_revision()?,
                decision: assessment.decision.into(),
                depth: assessment.depth,
                scope: assessment.scope,
                limitation: assessment.limitation,
                selected_policies: assessment
                    .selected_policies
                    .into_iter()
                    .map(|policy| valid(policy.try_into()))
                    .collect::<Result<_, _>>()?,
                evaluations: assessment
                    .evaluations
                    .into_iter()
                    .map(EvaluationData::into_evaluation)
                    .collect::<Result<_, _>>()?,
                findings: assessment
                    .findings
                    .into_iter()
                    .map(|index| tables.finding(index))
                    .collect::<Result<_, _>>()?,
            }))?,
            status: self.status.into(),
            abandonment: self
                .abandonment
                .map(|receipt| {
                    crate::rounds::nonblank(&receipt.reason)?;
                    Ok(Abandonment {
                        reason: receipt.reason,
                        verified_actions: receipt
                            .verified_actions
                            .into_iter()
                            .map(|action| action.into_action(tables))
                            .collect::<Result<_, JsonError>>()?,
                    })
                })
                .transpose()?,
            publication_limitation: self.publication_limitation.map(Into::into),
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
                .map(|action| action.into_action(tables))
                .collect::<Result<_, _>>()?,
            review_workflow: self.review_workflow,
            policy_set: self.policy_set,
            policy_names: self
                .policy_names
                .into_iter()
                .map(|index| tables.policy_name(index))
                .collect::<Result<_, _>>()?,
            policy_declarations: self
                .policy_declarations
                .into_iter()
                .map(|row| {
                    Ok(PublicPolicyDeclaration {
                        policy: tables.policy_name(row.policy)?,
                        status: row.status.into(),
                        reason: row.reason,
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
                        .map(|row| {
                            Ok(HistoricalFinding {
                                finding: tables.finding(row.finding)?,
                                root_note: valid(ProviderId::new(row.root_note))?,
                                thread: valid(ProviderId::new(row.thread))?,
                                actions: row
                                    .actions
                                    .into_iter()
                                    .map(PublishedActionData::into_action)
                                    .collect::<Result<_, JsonError>>()?,
                            })
                        })
                        .collect::<Result<_, JsonError>>()
                })
                .transpose()?,
            verified_resolutions: self
                .verified_resolutions
                .into_iter()
                .map(|receipt| {
                    Ok(ResolutionReceipt {
                        subject: subject.clone(),
                        reviewer: reviewer.clone(),
                        round: self.number,
                        thread: valid(ProviderId::new(receipt.thread))?,
                        resolved: receipt.resolved,
                        provenance: match receipt.provenance {
                            ResolutionProvenanceData::Acknowledged => {
                                ResolutionProvenance::Acknowledged
                            }
                            ResolutionProvenanceData::Observed => ResolutionProvenance::Observed,
                        },
                    })
                })
                .collect::<Result<_, JsonError>>()?,
            confirmed_validation: self.confirmed_validation,
        })
    }
}
