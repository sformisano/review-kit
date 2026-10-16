use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, ReviewerId, Revision,
    Severity, SourceLocation, SourcePath, Subject, Suggestion,
};
use serde::{Deserialize, Serialize};

use crate::JsonError;

pub fn valid<T, E>(value: Result<T, E>) -> Result<T, JsonError> {
    value.map_err(|_| JsonError::InvalidValue)
}

wire_enum!(Status, FindingStatus, [Open, Fixed, Withdrawn, Uncertain]);
wire_enum!(Tier, Severity, [P1, P2, P3, P4, P5]);
wire_enum!(DispositionData, Disposition, [Blocking, Advisory]);
wire_enum!(Decision, ReviewDecision, [Ready, ChangesRequired, Blocked]);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectData {
    provider: String,
    host: String,
    repository: String,
    request: u64,
}

impl From<&Subject> for SubjectData {
    fn from(value: &Subject) -> Self {
        Self {
            provider: value.provider().into(),
            host: value.host().into(),
            repository: value.repository().into(),
            request: value.request(),
        }
    }
}

impl SubjectData {
    pub fn into_subject(self) -> Result<Subject, JsonError> {
        valid(Subject::new(
            self.provider,
            self.host,
            self.repository,
            self.request,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionData {
    head: String,
    comparison: Vec<String>,
}

impl From<&Revision> for RevisionData {
    fn from(value: &Revision) -> Self {
        Self {
            head: value.head().into(),
            comparison: value.comparison().to_vec(),
        }
    }
}

impl RevisionData {
    pub fn into_revision(self) -> Result<Revision, JsonError> {
        valid(Revision::new(self.head, self.comparison))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationData {
    path: String,
    line: u64,
}

impl From<&SourceLocation> for LocationData {
    fn from(value: &SourceLocation) -> Self {
        Self {
            path: value.path().as_str().into(),
            line: value.line().get(),
        }
    }
}

impl LocationData {
    pub fn into_location(self) -> Result<SourceLocation, JsonError> {
        Ok(SourceLocation::new(
            valid(SourcePath::try_from(self.path))?,
            valid(LineNumber::try_from(self.line))?,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionData {
    location: LocationData,
    revision: RevisionData,
    start_line: u64,
    original: Vec<String>,
    replacement: Vec<String>,
    evidence: String,
}

impl From<&Suggestion> for SuggestionData {
    fn from(value: &Suggestion) -> Self {
        Self {
            location: value.location().into(),
            revision: value.revision().into(),
            start_line: value.start_line().get(),
            original: value.original().to_vec(),
            replacement: value.replacement().to_vec(),
            evidence: value.evidence().into(),
        }
    }
}

impl SuggestionData {
    pub fn into_suggestion(self) -> Result<Suggestion, JsonError> {
        valid(Suggestion::new(
            self.location.into_location()?,
            self.revision.into_revision()?,
            valid(LineNumber::try_from(self.start_line))?,
            self.original,
            self.replacement,
            self.evidence,
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingData {
    id: u64,
    policy_id: String,
    title: String,
    body: String,
    severity: Tier,
    disposition: DispositionData,
    status: Status,
    evidence: String,
    validated: bool,
    location: LocationData,
    suggestion: Option<SuggestionData>,
}

impl From<&Finding> for FindingData {
    fn from(value: &Finding) -> Self {
        Self {
            id: value.id().get(),
            policy_id: value.policy_id().as_str().into(),
            title: value.title().into(),
            body: value.body().into(),
            severity: value.severity().into(),
            disposition: value.disposition().into(),
            status: value.status().into(),
            evidence: value.evidence().into(),
            validated: value.validated(),
            location: value.location().into(),
            suggestion: value.suggestion().map(SuggestionData::from),
        }
    }
}

impl FindingData {
    pub fn finding_number(&self) -> u64 {
        self.id
    }
    /// The same record with its validation claim replaced.
    #[must_use]
    pub(crate) fn with_validated(mut self, validated: bool) -> Self {
        self.validated = validated;
        self
    }
    pub fn into_finding(self) -> Result<Finding, JsonError> {
        Finding::new(FindingDraft {
            id: valid(FindingId::try_from(self.id))?,
            policy_id: valid(PolicyId::try_from(self.policy_id))?,
            title: self.title,
            body: self.body,
            severity: self.severity.into(),
            disposition: self.disposition.into(),
            status: self.status.into(),
            evidence: self.evidence,
            validated: self.validated,
            location: self.location.into_location()?,
            suggestion: self
                .suggestion
                .map(SuggestionData::into_suggestion)
                .transpose()?,
        })
        .map_err(JsonError::Finding)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Judgment {
    Satisfied {},
    Violated { evidence: String },
    Blocked { limitation: String },
}

impl From<&PolicyJudgment> for Judgment {
    fn from(value: &PolicyJudgment) -> Self {
        match value {
            PolicyJudgment::Satisfied => Self::Satisfied {},
            PolicyJudgment::Violated { evidence } => Self::Violated {
                evidence: evidence.clone(),
            },
            PolicyJudgment::Blocked { limitation } => Self::Blocked {
                limitation: limitation.clone(),
            },
        }
    }
}

impl From<Judgment> for PolicyJudgment {
    fn from(value: Judgment) -> Self {
        match value {
            Judgment::Satisfied {} => Self::Satisfied,
            Judgment::Violated { evidence } => Self::Violated { evidence },
            Judgment::Blocked { limitation } => Self::Blocked { limitation },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationData {
    policy_id: String,
    judgment: Judgment,
}

impl From<&PolicyEvaluation> for EvaluationData {
    fn from(value: &PolicyEvaluation) -> Self {
        Self {
            policy_id: value.policy_id().as_str().into(),
            judgment: value.judgment().into(),
        }
    }
}

impl EvaluationData {
    pub fn into_evaluation(self) -> Result<PolicyEvaluation, JsonError> {
        valid(PolicyEvaluation::new(
            valid(PolicyId::try_from(self.policy_id))?,
            self.judgment.into(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentData {
    subject: SubjectData,
    reviewer: String,
    revision: RevisionData,
    decision: Decision,
    depth: String,
    scope: String,
    limitation: Option<String>,
    selected_policies: Vec<String>,
    evaluations: Vec<EvaluationData>,
    findings: Vec<FindingData>,
}

impl From<&Assessment> for AssessmentData {
    fn from(value: &Assessment) -> Self {
        Self {
            subject: value.subject().into(),
            reviewer: value.reviewer().as_str().into(),
            revision: value.revision().into(),
            decision: value.decision().into(),
            depth: value.depth().into(),
            scope: value.scope().into(),
            limitation: value.limitation().map(str::to_owned),
            selected_policies: value
                .selected_policies()
                .iter()
                .map(|value| value.as_str().into())
                .collect(),
            evaluations: value
                .evaluations()
                .iter()
                .map(EvaluationData::from)
                .collect(),
            findings: value.findings().iter().map(FindingData::from).collect(),
        }
    }
}

impl AssessmentData {
    pub fn into_assessment(self) -> Result<Assessment, JsonError> {
        valid(Assessment::new(AssessmentDraft {
            subject: self.subject.into_subject()?,
            reviewer: valid(ReviewerId::try_from(self.reviewer))?,
            revision: self.revision.into_revision()?,
            decision: self.decision.into(),
            depth: self.depth,
            scope: self.scope,
            limitation: self.limitation,
            selected_policies: self
                .selected_policies
                .into_iter()
                .map(|value| valid(PolicyId::try_from(value)))
                .collect::<Result<_, _>>()?,
            evaluations: self
                .evaluations
                .into_iter()
                .map(EvaluationData::into_evaluation)
                .collect::<Result<_, _>>()?,
            findings: self
                .findings
                .into_iter()
                .map(FindingData::into_finding)
                .collect::<Result<_, _>>()?,
        }))
    }
}
