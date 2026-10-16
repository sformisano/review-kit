use crate::observation::json_error;
use review_kit_observation::{Completion, Fields};

use std::collections::HashSet;
use std::path::{Component, Path};

use review_kit_application::{
    publication::{Effects, PublicPolicyDeclaration, validate_policy_declarations},
    use_cases::{
        assessment::{AssessmentPolicies, DraftIssue, PolicySlot, complete, prepare},
        initialization::InitializedAssessment,
        publishing::PublishRequest,
    },
};
use review_kit_domain::{Finding, FindingId, PolicyId};
use serde::{Deserialize, Serialize};

use crate::{
    ContextDocument, Diagnostic, JsonError,
    domain::{Decision, FindingData, Judgment, RevisionData, SubjectData, valid},
    rounds::{AnswerData, TransitionData},
    validate_version,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySlotData {
    pub policy_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub judgment: Option<Judgment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingSlotData {
    pub previous: FindingData,
    pub reassessment: Option<FindingData>,
}

document_kind!(DraftKind, Assessment, "assessment");

/// Completed assessment content before accepted inventory and applicability are attached.
#[derive(Debug, Clone)]
pub struct CompletedAssessment {
    pub assessment: review_kit_domain::Assessment,
    pub review_workflow: Option<String>,
    pub policies: AssessmentPolicies,
    pub effects: Effects,
    pub answers: Vec<review_kit_application::publication::Answer>,
    pub answer_targets: Vec<review_kit_application::publication::ReplyRecord>,
    pub transitions: Vec<review_kit_application::publication::TransitionReply>,
    pub local_context: String,
    pub allow_colocated: bool,
}

#[review_kit_observation::operations]
impl CompletedAssessment {
    #[helper]
    pub fn with_policy_declarations(
        self,
        policy_declarations: Vec<PublicPolicyDeclaration>,
    ) -> Result<PublishRequest, JsonError> {
        validate_policy_declarations(
            &policy_declarations,
            &self.assessment,
            self.policies.policies(),
        )
        .map_err(|_| JsonError::InvalidValue)?;
        Ok(PublishRequest {
            assessment: self.assessment,
            review_workflow: self.review_workflow,
            policies: self.policies,
            policy_declarations,
            effects: self.effects,
            answers: self.answers,
            answer_targets: self.answer_targets,
            transitions: self.transitions,
            local_context: self.local_context,
            allow_colocated: self.allow_colocated,
            confirmed_validation: false,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDocument {
    pub version: u64,
    pub kind: DraftKind,
    pub context: String,
    pub subject: SubjectData,
    pub reviewer: String,
    pub revision: RevisionData,
    pub policy_set: String,
    #[serde(default, skip_serializing)]
    pub requested_policies: Vec<String>,
    pub policies: Vec<PolicySlotData>,
    pub findings: Vec<FindingSlotData>,
    pub new_findings: Vec<FindingData>,
    pub decision: Option<Decision>,
    pub depth: Option<String>,
    pub scope: Option<String>,
    pub limitation: Option<String>,
    pub answers: Vec<AnswerData>,
    pub transitions: Vec<TransitionData>,
    pub local_context: String,
}

#[review_kit_observation::operations]
impl DraftDocument {
    #[helper]
    pub fn from_initialized(draft: &InitializedAssessment, context: &str) -> Self {
        let template = &draft.template;
        Self {
            version: 1,
            kind: DraftKind::Assessment,
            context: context.into(),
            subject: (&template.snapshot().subject).into(),
            reviewer: template.reviewer().as_str().into(),
            revision: (&template.snapshot().revision).into(),
            policy_set: draft.policies().name().into(),
            requested_policies: Vec::new(),
            policies: template
                .policies
                .iter()
                .map(|slot| PolicySlotData {
                    policy_id: slot.policy_id.as_str().into(),
                    name: draft
                        .policies()
                        .policies()
                        .iter()
                        .find(|policy| policy.id == slot.policy_id)
                        .map(|policy| policy.name.clone()),
                    judgment: slot.judgment.as_ref().map(Into::into),
                })
                .collect(),
            findings: template
                .findings
                .iter()
                .map(|slot| FindingSlotData {
                    previous: (&slot.previous).into(),
                    reassessment: slot.reassessment.as_ref().map(Into::into),
                })
                .collect(),
            new_findings: Vec::new(),
            decision: template.decision.map(Into::into),
            depth: template.depth.clone(),
            scope: template.scope.clone(),
            limitation: template.limitation.clone(),
            answers: Vec::new(),
            transitions: Vec::new(),
            local_context: template.local_context.clone(),
        }
    }

    #[helper]
    pub fn validate_reference(&self) -> Result<(), JsonError> {
        validate_version(self.version)?;
        let path = Path::new(&self.context);
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(JsonError::ContextBinding);
        }
        Ok(())
    }

    #[operation(
        name = "json.complete_draft",
        input = Fields::new()
        .count("policies", self.policies.len())
        .count("findings", self.findings.len())
        .count("new_findings", self.new_findings.len())
        .count("answers", self.answers.len())
        .count("transitions", self.transitions.len())
        .flag("allow_colocated", allow_colocated),
        output = |result| {
            Completion::result(
                result,
                |request| {
                    Fields::new()
                        .count("findings", request.assessment.findings().len())
                        .count("answers", request.answers.len())
                        .count("transitions", request.transitions.len())
                },
                json_error,
            )
        },
    )]
    pub fn complete(
        &self,
        context: &ContextDocument,
        effects: Effects,
        allow_colocated: bool,
    ) -> Result<CompletedAssessment, JsonError> {
        self.validate_reference()?;
        let bundle = context.bundle()?;
        if self.subject.clone().into_subject()? != bundle.snapshot().subject
            || self.reviewer != bundle.reviewer().as_str()
            || self.revision.clone().into_revision()? != bundle.snapshot().revision
        {
            return Err(JsonError::ContextBinding);
        }
        let policies = AssessmentPolicies::new(
            self.policy_set.clone(),
            self.policies
                .iter()
                .map(|slot| {
                    Ok(review_kit_application::publication::PolicyName {
                        id: valid(PolicyId::try_from(slot.policy_id.clone()))?,
                        name: slot.name.clone().unwrap_or_else(|| slot.policy_id.clone()),
                    })
                })
                .collect::<Result<Vec<_>, JsonError>>()?,
        )
        .map_err(JsonError::Policies)?;
        let mut template = prepare(
            bundle.snapshot().clone(),
            bundle.reviewer().clone(),
            policies.clone(),
        )
        .map_err(JsonError::Preparation)?;
        let mut diagnostics = Vec::new();
        template.policies = self
            .policies
            .iter()
            .filter_map(|slot| match PolicyId::try_from(slot.policy_id.clone()) {
                Ok(policy_id) => Some(PolicySlot {
                    policy_id,
                    judgment: slot.judgment.clone().map(Into::into),
                }),
                Err(_) => {
                    diagnostics.push(Diagnostic::new(
                        "invalid_policy_id",
                        "Policy ID must not be blank.",
                    ));
                    None
                }
            })
            .collect();
        template.decision = self.decision.clone().map(Into::into);
        template.depth = self.depth.clone();
        template.scope = self.scope.clone();
        template.limitation = self.limitation.clone();
        template.local_context = self.local_context.clone();
        let mut seen = HashSet::new();
        let mut invalid_reassessments = HashSet::new();
        for (index, slot) in self.findings.iter().enumerate() {
            let previous = match slot.previous.clone().into_finding() {
                Ok(previous) => previous,
                Err(error) => {
                    if let Ok(id) = FindingId::try_from(slot.previous.finding_number()) {
                        invalid_reassessments.insert(id);
                    }
                    diagnostics.push(Diagnostic::input(
                        "invalid_previous_finding",
                        format!("findings[{index}].previous"),
                        Some(slot.previous.finding_number()),
                        &crate::error_chain(&error),
                    ));
                    continue;
                }
            };
            if !seen.insert(previous.id()) {
                diagnostics.push(Diagnostic::finding(
                    "duplicate_carried_finding",
                    previous.id(),
                    "Carried finding appears more than once.",
                ));
                continue;
            }
            match template
                .findings
                .iter_mut()
                .find(|candidate| candidate.previous.id() == previous.id())
            {
                Some(candidate) if candidate.previous == previous => {
                    match slot
                        .reassessment
                        .clone()
                        .map(FindingData::into_finding)
                        .transpose()
                    {
                        Ok(reassessment) => candidate.reassessment = reassessment,
                        Err(error) => {
                            invalid_reassessments.insert(previous.id());
                            diagnostics.push(Diagnostic::input(
                                "invalid_finding",
                                format!("findings[{index}].reassessment"),
                                Some(previous.id().get()),
                                &crate::error_chain(&error),
                            ));
                        }
                    }
                }
                _ => diagnostics.push(Diagnostic::finding(
                    "changed_previous_finding",
                    previous.id(),
                    "Carried finding differs from retained context.",
                )),
            }
        }
        let new_findings: Vec<Finding> = self
            .new_findings
            .iter()
            .cloned()
            .enumerate()
            .filter_map(|(index, finding)| {
                let number = finding.finding_number();
                match finding.into_finding() {
                    Ok(finding) => Some(finding),
                    Err(error) => {
                        diagnostics.push(Diagnostic::input(
                            "invalid_new_finding",
                            format!("new_findings[{index}]"),
                            Some(number),
                            &crate::error_chain(&error),
                        ));
                        None
                    }
                }
            })
            .collect();
        let answers: Vec<_> = self.answers.iter().cloned().enumerate().filter_map(|(index, answer)| {
                let finding = answer.finding_number();
                match answer.into_answer() {
                    Ok(answer) => Some(answer),
                    Err(_) => {
                        diagnostics.push(Diagnostic::input("invalid_answer", format!("answers[{index}]"), Some(finding), "Answer requires a positive finding ID, a nonblank target identity, and a nonblank body."));
                        None
                    }
                }
            }).collect();
        let transitions = self
            .transitions
            .iter()
            .cloned()
            .enumerate()
            .filter_map(|(index, transition)| {
                let finding = transition.finding_number();
                match transition.into_transition() {
                    Ok(transition) => Some(transition),
                    Err(_) => {
                        diagnostics.push(Diagnostic::input(
                            "invalid_transition",
                            format!("transitions[{index}]"),
                            Some(finding),
                            "Transition requires a positive finding ID and a nonblank explanation.",
                        ));
                        None
                    }
                }
            })
            .collect();
        let assessment = match complete(template, new_findings) {
            Ok(assessment) => Some(assessment),
            Err(issues) => {
                diagnostics.extend(issues.iter().filter(|issue| !matches!(issue, DraftIssue::MissingFindingReassessment(id) if invalid_reassessments.contains(id))).map(Diagnostic::from_draft_issue));
                None
            }
        };
        if !diagnostics.is_empty() {
            return Err(JsonError::Draft(diagnostics));
        }
        let assessment = assessment.ok_or(JsonError::InvalidValue)?;
        let answer_targets = bundle
            .snapshot()
            .replies
            .iter()
            .filter(|target| answers.iter().any(|answer| answer.in_reply_to == target.id))
            .cloned()
            .collect();
        Ok(CompletedAssessment {
            assessment,
            review_workflow: None,
            policies,
            effects,
            answers,
            answer_targets,
            transitions,
            local_context: self.local_context.clone(),
            allow_colocated,
        })
    }
}
