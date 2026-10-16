use std::collections::HashSet;

use review_kit_application::{
    publication::{Effects, PolicyName, PublicPolicyDeclaration},
    review_run::RunId,
    use_cases::{
        checkpoints::{
            ApplicabilityStatus, CONTEXT_FILE, PolicyOutcome, ReviewBinding,
            ReviewDeclarationsDraft, ReviewPolicyDraft, Successor, reusable_confirmations,
            validate_declarations,
        },
        publishing::PublishRequest,
    },
};
use review_kit_domain::FindingStatus;
use review_kit_observation::{Completion, Fields};
use serde::{Deserialize, Serialize};

use super::{
    diagnostics, parse,
    verdicts::{self, ConfirmationData, ReassessmentData},
    wire::nullable,
};
use crate::{
    ContextDocument, Diagnostic, DraftDocument, DraftKind, FindingSlotData, JsonError,
    PolicySlotData,
    domain::{Decision, FindingData, Judgment, RevisionData},
    rounds::{AnswerData, TransitionData},
};

checkpoint_kind!(AssessmentKind, Assessment, "assessment");

/// The assessment format this release reads and writes.
pub const ASSESSMENT_VERSION: u64 = 4;

/// Longest accepted private context, in characters: pointers and decisive facts only.
pub const LOCAL_CONTEXT_LIMIT: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolicyOutcomeData {
    Excluded {},
    Unresolved { limitation: String },
    Satisfied {},
    Violated { evidence: String },
    Blocked { limitation: String },
}

#[review_kit_observation::operations]
impl From<PolicyOutcomeData> for PolicyOutcome {
    #[helper]
    fn from(value: PolicyOutcomeData) -> Self {
        match value {
            PolicyOutcomeData::Excluded {} => Self::Excluded,
            PolicyOutcomeData::Unresolved { limitation } => Self::Unresolved { limitation },
            PolicyOutcomeData::Satisfied {} => Self::Satisfied,
            PolicyOutcomeData::Violated { evidence } => Self::Violated { evidence },
            PolicyOutcomeData::Blocked { limitation } => Self::Blocked { limitation },
        }
    }
}

/// The single authored and frozen review document; captured identity stays in context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentDocument {
    pub version: u64,
    pub kind: AssessmentKind,
    pub run_id: String,
    pub revision: RevisionData,
    #[serde(deserialize_with = "nullable")]
    pub supersedes: Option<String>,
    pub reason: String,
    pub policy_set: String,
    pub policies: Vec<AssessmentPolicyData>,
    pub findings: Vec<CarriedFindingData>,
    pub new_findings: Vec<FindingData>,
    #[serde(deserialize_with = "nullable")]
    pub decision: Option<Decision>,
    #[serde(deserialize_with = "nullable")]
    pub depth: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub scope: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub limitation: Option<String>,
    pub answers: Vec<AnswerData>,
    pub transitions: Vec<TransitionData>,
    pub confirmations: Vec<ConfirmationData>,
    pub local_context: String,
}

/// One selected policy's declaration, applicability and judgment, each written once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentPolicyData {
    pub policy_id: String,
    pub name: String,
    pub required: bool,
    pub reason: String,
    #[serde(deserialize_with = "nullable")]
    pub outcome: Option<PolicyOutcomeData>,
}

/// The current decision for one carried finding; the previous version comes only from context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarriedFindingData {
    pub id: u64,
    /// Generated: whether a `retained` confirmation can reuse the captured version's recorded confirmation.
    pub reusable_confirmation: bool,
    pub reassessment: ReassessmentData,
}

#[derive(Debug, Clone, Copy)]
pub struct AssessmentInputs<'a> {
    pub run_id: &'a RunId,
    pub context: &'a ContextDocument,
    pub successor: Successor<'a>,
}

#[review_kit_observation::operations]
impl AssessmentDocument {
    /// Start from captured identity and carried IDs, without default policy judgments.
    #[helper]
    pub fn initial(run_id: &RunId, context: &ContextDocument) -> Result<Self, JsonError> {
        let bundle = context.bundle()?;
        let reusable =
            reusable_confirmations(bundle.snapshot(), bundle.reviewer(), bundle.findings());
        Ok(Self {
            version: ASSESSMENT_VERSION,
            kind: AssessmentKind::Assessment,
            run_id: run_id.as_str().into(),
            revision: (&bundle.snapshot().revision).into(),
            supersedes: None,
            reason: "Initial review assessment.".into(),
            policy_set: String::new(),
            policies: Vec::new(),
            findings: bundle
                .findings()
                .iter()
                .map(|finding| CarriedFindingData {
                    id: finding.id().get(),
                    reusable_confirmation: reusable
                        .iter()
                        .any(|captured| captured.id() == finding.id()),
                    reassessment: ReassessmentData::Pending,
                })
                .collect(),
            new_findings: Vec::new(),
            decision: None,
            depth: None,
            scope: None,
            limitation: None,
            answers: Vec::new(),
            transitions: Vec::new(),
            confirmations: Vec::new(),
            local_context: String::new(),
        })
    }

    /// Validate the single declaration and complete its assessment from retained context.
    #[operation(
        name = "json.checkpoints.complete_assessment",
        input = Fields::new().count("policies", self.policies.len()).count("findings", self.findings.len()),
        output = |result| Completion::result(result, |_| Fields::new(), |issues| diagnostics::diagnostics_error(issues)),
    )]
    pub fn complete(
        &self,
        inputs: &AssessmentInputs<'_>,
        effects: Effects,
        allow_colocated: bool,
    ) -> Result<PublishRequest, Vec<Diagnostic>> {
        if self.version != ASSESSMENT_VERSION {
            return Err(vec![diagnostics::unsupported_version(self.version)]);
        }
        let bundle = inputs
            .context
            .bundle()
            .map_err(|error| error.diagnostics())?;
        let revision = self
            .revision
            .clone()
            .into_revision()
            .map_err(|error| error.diagnostics())?;
        let mut problems = Vec::new();
        if self.local_context.chars().count() > LOCAL_CONTEXT_LIMIT {
            problems.push(Diagnostic::input(
                "local_context_too_long",
                "local_context".into(),
                None,
                &format!(
                    "Private context exceeds {LOCAL_CONTEXT_LIMIT} characters; keep file pointers and decisive facts only."
                ),
            ));
        }
        let declarations = validate_declarations(
            &ReviewDeclarationsDraft {
                run_id: self.run_id.clone(),
                revision,
                policy_set: self.policy_set.clone(),
                supersedes: self.supersedes.clone(),
                reason: self.reason.clone(),
                policies: self
                    .policies
                    .iter()
                    .map(|row| ReviewPolicyDraft {
                        policy_id: row.policy_id.clone(),
                        name: row.name.clone(),
                        required: row.required,
                        reason: row.reason.clone(),
                        outcome: row.outcome.clone().map(Into::into),
                    })
                    .collect(),
            },
            &ReviewBinding {
                run_id: inputs.run_id,
                revision: &bundle.snapshot().revision,
                carried_findings: bundle.findings(),
                successor: inputs.successor,
            },
        )
        .map_err(|issues| problems.extend(issues.iter().map(diagnostics::review)))
        .ok();
        let reusable =
            reusable_confirmations(bundle.snapshot(), bundle.reviewer(), bundle.findings());
        let mut seen = HashSet::new();
        for (index, row) in self.findings.iter().enumerate() {
            let message = if !seen.insert(row.id) {
                Some((
                    "duplicate_carried_finding",
                    "Carried finding appears more than once.",
                ))
            } else if !bundle
                .findings()
                .iter()
                .any(|finding| finding.id().get() == row.id)
            {
                Some((
                    "unknown_carried_finding",
                    "Retained context does not carry this finding.",
                ))
            } else if matches!(&row.reassessment, ReassessmentData::Reassessed(finding) if finding.finding_number() != row.id)
            {
                Some((
                    "changed_finding_id",
                    "Reassessment must keep its carried finding ID.",
                ))
            } else if row.reusable_confirmation
                != reusable
                    .iter()
                    .any(|captured| captured.id().get() == row.id)
            {
                Some((
                    "changed_reusable_confirmation",
                    "reusable_confirmation is generated from retained context; keep rkc's value.",
                ))
            } else {
                None
            };
            if let Some((code, text)) = message {
                problems.push(Diagnostic::input(
                    code,
                    format!("findings[{index}]"),
                    Some(row.id),
                    text,
                ));
            }
        }
        let carried: Vec<(FindingData, Option<FindingData>)> = bundle
            .findings()
            .iter()
            .map(|previous| {
                let row = self
                    .findings
                    .iter()
                    .find(|row| row.id == previous.id().get());
                if row.is_none() {
                    problems.push(Diagnostic::finding(
                        "missing_carried_finding",
                        previous.id(),
                        "Every carried finding needs a current reassessment row.",
                    ));
                }
                let current = row.and_then(|row| match &row.reassessment {
                    ReassessmentData::Pending => None,
                    ReassessmentData::Unchanged => Some(verdicts::unchanged(previous)),
                    ReassessmentData::Reassessed(finding) => Some(finding.as_ref().clone()),
                });
                (previous.into(), current)
            })
            .collect();
        let (carried, new_findings) = verdicts::admit(
            carried,
            &self.new_findings,
            &self.confirmations,
            &reusable,
            &mut problems,
        );
        let content = DraftDocument {
            version: 1,
            kind: DraftKind::Assessment,
            context: CONTEXT_FILE.into(),
            subject: (&bundle.snapshot().subject).into(),
            reviewer: bundle.reviewer().as_str().into(),
            revision: (&bundle.snapshot().revision).into(),
            policy_set: self.policy_set.clone(),
            requested_policies: Vec::new(),
            policies: self
                .policies
                .iter()
                .filter(|row| !matches!(row.outcome, Some(PolicyOutcomeData::Excluded {})))
                .map(|row| PolicySlotData {
                    policy_id: row.policy_id.clone(),
                    name: Some(row.name.clone()),
                    judgment: row
                        .outcome
                        .clone()
                        .map(PolicyOutcome::from)
                        .and_then(|outcome| outcome.judgment())
                        .as_ref()
                        .map(Judgment::from),
                })
                .collect(),
            findings: carried
                .into_iter()
                .map(|(previous, reassessment)| FindingSlotData {
                    previous,
                    reassessment,
                })
                .collect(),
            new_findings,
            decision: self.decision.clone(),
            depth: self.depth.clone(),
            scope: self.scope.clone(),
            limitation: self.limitation.clone(),
            answers: self.answers.clone(),
            transitions: self.transitions.clone(),
            local_context: self.local_context.clone(),
        }
        .complete(inputs.context, effects, allow_colocated)
        .map_err(|error| problems.extend(error.diagnostics()))
        .ok();
        if !problems.is_empty() {
            return Err(problems);
        }
        let (Some(declarations), Some(content)) = (declarations, content) else {
            return Err(vec![Diagnostic::new(
                "incomplete_assessment",
                "Review completion did not produce valid declarations and content.",
            )]);
        };
        for previous in bundle.findings().iter().filter(|finding| {
            matches!(
                finding.status(),
                FindingStatus::Open | FindingStatus::Uncertain
            )
        }) {
            if declarations.policies().iter().any(|row| {
                row.policy_id() == previous.policy_id()
                    && row.status() == ApplicabilityStatus::Excluded
            }) {
                let closed = content
                    .assessment
                    .findings()
                    .iter()
                    .find(|finding| finding.id() == previous.id())
                    .filter(|finding| {
                        matches!(
                            finding.status(),
                            FindingStatus::Fixed | FindingStatus::Withdrawn
                        )
                    });
                if !closed.is_some_and(|finding| {
                    content.transitions.iter().any(|transition| {
                        transition.finding == previous.id()
                            && transition.from == previous.status()
                            && transition.to == finding.status()
                    })
                }) {
                    return Err(vec![Diagnostic::finding(
                        "excluded_active_finding",
                        previous.id(),
                        "Excluding an active carried policy requires fixed or withdrawn reassessment and its transition explanation.",
                    )]);
                }
            }
        }
        content
            .with_policy_declarations(
                declarations
                    .policies()
                    .iter()
                    .map(|row| PublicPolicyDeclaration {
                        policy: PolicyName {
                            id: row.policy_id().clone(),
                            name: row.name().into(),
                        },
                        status: row.status(),
                        reason: row.reason().into(),
                    })
                    .collect(),
            )
            .map(|request| PublishRequest {
                confirmed_validation: true,
                ..request
            })
            .map_err(|error| error.diagnostics())
    }
}

/// Decode and complete one current-format review; no earlier format is accepted.
#[review_kit_observation::operations]
#[helper]
pub fn check_assessment(
    bytes: &[u8],
    inputs: &AssessmentInputs<'_>,
    effects: Effects,
    allow_colocated: bool,
) -> Result<PublishRequest, Vec<Diagnostic>> {
    let document: AssessmentDocument = parse(bytes).map_err(|diagnostic| vec![diagnostic])?;
    document.complete(inputs, effects, allow_colocated)
}
