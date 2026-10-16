use std::collections::HashSet;

use crate::{
    Disposition, Finding, FindingId, FindingStatus, PolicyId, ReviewerId, Revision, Subject,
};

/// The agent's conclusion about one policy in the review's coverage.
/// Findings are assessed separately; their presence does not derive this judgment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyJudgment {
    /// The policy is satisfied without requiring additional prose.
    Satisfied,
    /// An established violation, with evidence required by [`PolicyEvaluation::new`].
    Violated { evidence: String },
    /// Evaluation is unavailable; its limitation must be nonblank.
    Blocked { limitation: String },
}

/// One policy's result in an assessment, so included coverage has an explicit judgment.
/// Construction checks the required explanation; it does not establish the judgment's truth.
/// The value has no independent identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEvaluation {
    policy_id: PolicyId,
    judgment: PolicyJudgment,
}

impl PolicyEvaluation {
    /// Requires evidence for violations and a limitation for unavailable checks.
    /// Policy membership is checked when constructing the containing assessment.
    pub fn new(
        policy_id: PolicyId,
        judgment: PolicyJudgment,
    ) -> Result<Self, PolicyEvaluationError> {
        match &judgment {
            PolicyJudgment::Violated { evidence } if evidence.trim().is_empty() => {
                return Err(PolicyEvaluationError::MissingViolationEvidence);
            }
            PolicyJudgment::Blocked { limitation } if limitation.trim().is_empty() => {
                return Err(PolicyEvaluationError::MissingLimitation);
            }
            _ => {}
        }
        Ok(Self {
            policy_id,
            judgment,
        })
    }
    pub fn policy_id(&self) -> &PolicyId {
        &self.policy_id
    }
    pub fn judgment(&self) -> &PolicyJudgment {
        &self.judgment
    }
}

/// Missing explanation required by a policy judgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum PolicyEvaluationError {
    /// The violation evidence is empty or whitespace-only.
    #[displaydoc("violated policy requires evidence")]
    MissingViolationEvidence,
    /// The unavailable evaluation has no nonblank limitation.
    #[displaydoc("blocked policy requires a limitation")]
    MissingLimitation,
}

/// The agent's overall conclusion, retained after [`Assessment::new`] checks consistency.
/// It applies to the reviewed revision; individual concerns keep their own finding status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    /// No violated policy or open blocking finding remains, and required judgments are complete.
    Ready,
    /// The agent requires changes; severity does not derive this decision.
    ChangesRequired,
    /// The agent cannot reach a complete conclusion; an overall limitation is required.
    Blocked,
}

/// Unchecked construction input; changing it does not change an existing [`Assessment`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentDraft {
    pub subject: Subject,
    pub reviewer: ReviewerId,
    pub revision: Revision,
    pub decision: ReviewDecision,
    /// Describes how far the agent reviewed; the domain defines no depth vocabulary.
    pub depth: String,
    /// Describes what the review covers; the domain defines no scope vocabulary.
    pub scope: String,
    /// Explains a constraint on the overall conclusion, including missing context.
    pub limitation: Option<String>,
    pub selected_policies: Vec<PolicyId>,
    pub evaluations: Vec<PolicyEvaluation>,
    pub findings: Vec<Finding>,
}

/// An immutable snapshot of explicit judgments for one reviewed revision.
///
/// It joins who reviewed which change with policy coverage, individual concerns,
/// and the agent's overall conclusion. A follow-up uses another snapshot and retains
/// the identities of concerns reassessed from the earlier review.
///
/// It has no independent identity; equality compares all content, including finding
/// state. Construction checks consistency while preserving the agent's decision.
/// Coverage sufficiency, historical ownership and review truth remain caller facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    data: AssessmentDraft,
}

impl Assessment {
    /// Collects independent consistency failures without producing a partial assessment.
    /// Each selected policy needs one evaluation, and finding IDs must be unique.
    /// Fixed and withdrawn findings can retain historical policies and suggestions;
    /// open findings must reference selected policies and the reviewed suggestion revision.
    pub fn new(data: AssessmentDraft) -> Result<Self, Vec<AssessmentError>> {
        let mut errors = Vec::new();
        if data.depth.trim().is_empty() {
            errors.push(AssessmentError::BlankDepth);
        }
        if data.scope.trim().is_empty() {
            errors.push(AssessmentError::BlankScope);
        }
        if data
            .limitation
            .as_ref()
            .is_some_and(|text| text.trim().is_empty())
        {
            errors.push(AssessmentError::BlankLimitation);
        }
        validate_decision(&data, &mut errors);
        let mut selected = HashSet::new();
        for id in &data.selected_policies {
            if !selected.insert(id) {
                errors.push(AssessmentError::DuplicateSelectedPolicy(id.clone()));
            }
        }
        let mut evaluated = HashSet::new();
        for evaluation in &data.evaluations {
            let id = evaluation.policy_id();
            if !selected.contains(id) {
                errors.push(AssessmentError::UnselectedEvaluation(id.clone()));
            }
            if !evaluated.insert(id) {
                errors.push(AssessmentError::DuplicateEvaluation(id.clone()));
            }
        }
        for id in &data.selected_policies {
            if !evaluated.contains(id) {
                errors.push(AssessmentError::MissingEvaluation(id.clone()));
            }
        }
        let mut finding_ids = HashSet::new();
        for finding in &data.findings {
            if !finding_ids.insert(finding.id()) {
                errors.push(AssessmentError::DuplicateFinding(finding.id()));
            }
            if !selected.contains(finding.policy_id())
                && !matches!(
                    finding.status(),
                    FindingStatus::Fixed | FindingStatus::Withdrawn
                )
            {
                errors.push(AssessmentError::UnselectedFindingPolicy(finding.id()));
            }
            if finding.status() == FindingStatus::Open
                && finding
                    .suggestion()
                    .is_some_and(|suggestion| suggestion.revision() != &data.revision)
            {
                errors.push(AssessmentError::SuggestionRevisionMismatch(finding.id()));
            }
        }
        if errors.is_empty() {
            Ok(Self { data })
        } else {
            Err(errors)
        }
    }
    pub fn subject(&self) -> &Subject {
        &self.data.subject
    }
    pub fn reviewer(&self) -> &ReviewerId {
        &self.data.reviewer
    }
    pub fn revision(&self) -> &Revision {
        &self.data.revision
    }
    pub fn selected_policies(&self) -> &[PolicyId] {
        &self.data.selected_policies
    }
    pub fn evaluations(&self) -> &[PolicyEvaluation] {
        &self.data.evaluations
    }
    pub fn findings(&self) -> &[Finding] {
        &self.data.findings
    }

    /// The agent supplies the decision; construction rejects contradictory facts.
    pub fn decision(&self) -> ReviewDecision {
        self.data.decision
    }
    pub fn depth(&self) -> &str {
        &self.data.depth
    }
    pub fn scope(&self) -> &str {
        &self.data.scope
    }
    pub fn limitation(&self) -> Option<&str> {
        self.data.limitation.as_deref()
    }
}

fn validate_decision(data: &AssessmentDraft, errors: &mut Vec<AssessmentError>) {
    if data.decision == ReviewDecision::Blocked && data.limitation.is_none() {
        errors.push(AssessmentError::MissingBlockedLimitation);
    }
    let incomplete =
        data.evaluations
            .iter()
            .any(|evaluation| matches!(evaluation.judgment(), PolicyJudgment::Blocked { .. }))
            || data.findings.iter().any(|finding| {
                finding.status() == FindingStatus::Uncertain || !finding.validated()
            });
    if incomplete && data.decision != ReviewDecision::Blocked {
        errors.push(AssessmentError::IncompleteDecision(data.decision));
    }
    if data.decision == ReviewDecision::Ready {
        for evaluation in &data.evaluations {
            if matches!(evaluation.judgment(), PolicyJudgment::Violated { .. }) {
                errors.push(AssessmentError::ReadyWithViolation(
                    evaluation.policy_id().clone(),
                ));
            }
        }
        for finding in &data.findings {
            if finding.status() == FindingStatus::Open
                && finding.disposition() == Disposition::Blocking
            {
                errors.push(AssessmentError::ReadyWithBlockingFinding(finding.id()));
            }
        }
    }
}

/// Independent contradictions collected by [`Assessment::new`].
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum AssessmentError {
    /// Review depth is empty or whitespace-only.
    #[displaydoc("review depth must not be blank")]
    BlankDepth,
    /// Review scope is empty or whitespace-only.
    #[displaydoc("review scope must not be blank")]
    BlankScope,
    /// A supplied overall limitation is empty or whitespace-only.
    #[displaydoc("review limitation must not be blank when supplied")]
    BlankLimitation,
    /// A blocked decision has no overall limitation.
    #[displaydoc("blocked decision requires an overall limitation")]
    MissingBlockedLimitation,
    /// Unavailable policy checks, uncertain findings or unvalidated findings require blocked.
    #[displaydoc("decision {0:?} requires complete policy and finding judgments")]
    IncompleteDecision(ReviewDecision),
    /// Ready conflicts with an established violation of the named policy.
    #[displaydoc("ready decision conflicts with violated policy {0}")]
    ReadyWithViolation(PolicyId),
    /// Ready conflicts with the named open blocking finding.
    #[displaydoc("ready decision conflicts with open blocking finding {0}")]
    ReadyWithBlockingFinding(FindingId),
    /// The selected policy list repeats this ID.
    #[displaydoc("policy {0} is selected more than once")]
    DuplicateSelectedPolicy(PolicyId),
    /// An evaluation refers to a policy outside the selected list.
    #[displaydoc("evaluation names unselected policy {0}")]
    UnselectedEvaluation(PolicyId),
    /// The same policy has multiple evaluations.
    #[displaydoc("policy {0} is evaluated more than once")]
    DuplicateEvaluation(PolicyId),
    /// A selected policy has no evaluation.
    #[displaydoc("selected policy {0} has no evaluation")]
    MissingEvaluation(PolicyId),
    /// Multiple finding records use the same local ID.
    #[displaydoc("finding {0} appears more than once")]
    DuplicateFinding(FindingId),
    /// An active finding refers to an unselected policy.
    #[displaydoc("finding {0} names an unselected policy")]
    UnselectedFindingPolicy(FindingId),
    /// An open finding's suggestion refers to another reviewed revision.
    #[displaydoc("finding {0} suggestion uses a different revision")]
    SuggestionRevisionMismatch(FindingId),
}

#[cfg(test)]
mod tests;
