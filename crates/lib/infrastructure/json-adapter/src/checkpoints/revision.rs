use std::collections::HashSet;

use review_kit_application::{review_run::RunId, use_cases::checkpoints::Accepted};

use super::{
    ASSESSMENT_VERSION, AssessmentDocument, PolicyOutcomeData, ReassessmentData, candidates,
    diagnostics, parse,
};
use crate::{ContextDocument, Diagnostic, JsonError};

/// The agent's revision request: a reason, and whether to discard unaccepted edits.
#[derive(Debug, Clone, Copy)]
pub struct RevisionRequest<'a> {
    pub reason: &'a str,
    /// Rebuild from accepted state and parents instead of the current draft.
    /// This is the only way past a malformed draft.
    pub regenerate: bool,
}

/// A successor draft and the problems revision found but kept for the agent to resolve.
#[derive(Debug, Clone)]
pub struct Revised<T> {
    pub document: T,
    pub diagnostics: Vec<Diagnostic>,
}

/// Revision produced no successor draft.
#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum RevisionError {
    /// current draft is malformed; fix it or regenerate from accepted state
    MalformedDraft(Diagnostic),
    /// revision reason must not be blank
    BlankReason,
    /// retained context cannot supply the generated assessment
    Context(#[source] JsonError),
    /// a candidate finding cannot be re-encoded
    Candidate(#[source] JsonError),
}

#[review_kit_observation::operations]
impl RevisionError {
    #[helper]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            Self::MalformedDraft(diagnostic) => vec![diagnostic.clone()],
            Self::BlankReason => vec![Diagnostic::new(
                "blank_reason",
                "Revision reason must not be blank.",
            )],
            Self::Context(error) | Self::Candidate(error) => error.diagnostics(),
        }
    }
}

/// Retained state that an assessment revision keeps bound to.
#[derive(Debug, Clone, Copy)]
pub struct AssessmentRevisionInputs<'a> {
    pub run_id: &'a RunId,
    /// The run's retained `review-context.json`.
    pub context: &'a ContextDocument,
    /// The manifest's current accepted assessment, if one exists.
    pub accepted: Option<&'a Accepted<AssessmentDocument>>,
}

/// Prepare one successor and reset conclusions that need current confirmation.
///
/// Validation claims reset and recorded confirmations are not carried forward.
#[review_kit_observation::operations]
#[operation(
    name = "json.checkpoints.revise_assessment",
    input = review_kit_observation::Fields::new().flag("regenerate", request.regenerate),
    output = |result| review_kit_observation::Completion::result(result, |_| review_kit_observation::Fields::new(), |error| review_kit_observation::ErrorInfo::new("RevisionError", match error { RevisionError::MalformedDraft(_) => "malformed_draft", RevisionError::BlankReason => "blank_reason", RevisionError::Context(_) => "context", RevisionError::Candidate(_) => "candidate" })),
)]
pub fn revise_assessment(
    draft: Option<&[u8]>,
    inputs: &AssessmentRevisionInputs<'_>,
    request: RevisionRequest<'_>,
) -> Result<Revised<AssessmentDocument>, RevisionError> {
    if request.reason.trim().is_empty() {
        return Err(RevisionError::BlankReason);
    }
    let current: Option<AssessmentDocument> = match draft.filter(|_| !request.regenerate) {
        Some(bytes) => {
            let document: AssessmentDocument =
                parse(bytes).map_err(RevisionError::MalformedDraft)?;
            if document.version != ASSESSMENT_VERSION {
                return Err(RevisionError::MalformedDraft(
                    diagnostics::unsupported_version(document.version),
                ));
            }
            Some(document)
        }
        None => None,
    };
    let mut document = AssessmentDocument::initial(inputs.run_id, inputs.context)
        .map_err(RevisionError::Context)?;
    let mut diagnostics = Vec::new();
    if let Some(base) = current.or_else(|| inputs.accepted.map(|accepted| accepted.content.clone()))
    {
        document.policy_set = base.policy_set.clone();
        document.policies = base.policies.clone();
        for row in &mut document.policies {
            if !matches!(
                row.outcome,
                Some(PolicyOutcomeData::Excluded {} | PolicyOutcomeData::Unresolved { .. })
            ) {
                row.outcome = None;
            }
        }
        keep_candidates(&mut document, base, &mut diagnostics)?;
    }
    document.run_id = inputs.run_id.as_str().into();
    document.supersedes = inputs
        .accepted
        .map(|accepted| accepted.reference.as_str().into());
    document.reason = request.reason.into();
    Ok(Revised {
        document,
        diagnostics,
    })
}

#[review_kit_observation::operations]
#[helper]
fn keep_candidates(
    document: &mut AssessmentDocument,
    base: AssessmentDocument,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), RevisionError> {
    let mut merged = HashSet::new();
    let mut extra = Vec::new();
    for (index, mut slot) in base.findings.into_iter().enumerate() {
        if let ReassessmentData::Reassessed(finding) = slot.reassessment {
            slot.reassessment =
                ReassessmentData::Reassessed(Box::new(finding.with_validated(false)));
        }
        let number = slot.id;
        let target = document
            .findings
            .iter_mut()
            .find(|target| target.id == number);
        match target {
            Some(target) if merged.insert(number) => target.reassessment = slot.reassessment,
            target => {
                let (code, message) = match target {
                    Some(_) => (
                        "duplicate_carried_finding",
                        "Carried finding appears more than once; revision kept the extra slot.",
                    ),
                    None => (
                        "unknown_carried_finding",
                        "Retained context does not carry this finding; revision kept the slot.",
                    ),
                };
                diagnostics.push(Diagnostic::input(
                    code,
                    format!("findings[{index}]"),
                    Some(number),
                    message,
                ));
                extra.push(slot);
            }
        }
    }
    document.findings.extend(extra);
    document.new_findings = base
        .new_findings
        .into_iter()
        .map(|finding| finding.with_validated(false))
        .collect();
    document.answers = base.answers;
    document.transitions = base.transitions;
    document.depth = base.depth;
    document.scope = base.scope;
    document.limitation = base.limitation;
    document.local_context = base.local_context;
    report_candidates(document, diagnostics)
}

#[review_kit_observation::operations]
#[helper]
fn report_candidates(
    document: &AssessmentDocument,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), RevisionError> {
    let slots: HashSet<&str> = document
        .policies
        .iter()
        .filter(|slot| !matches!(slot.outcome, Some(PolicyOutcomeData::Excluded {})))
        .map(|slot| slot.policy_id.as_str())
        .collect();
    let incompatible = |input: String, number: u64, policy: String| Diagnostic {
        policy: Some(policy),
        ..Diagnostic::input(
            "incompatible_finding_policy",
            input,
            Some(number),
            "Finding policy is neither applicable nor unresolved; revision kept the finding.",
        )
    };
    for (index, slot) in document.findings.iter().enumerate() {
        let ReassessmentData::Reassessed(reassessment) = &slot.reassessment else {
            continue;
        };
        let current = candidates::fields(reassessment).map_err(RevisionError::Candidate)?;
        let active = matches!(current.status.as_str(), "open" | "uncertain");
        if !slots.contains(current.policy_id.as_str()) && active {
            diagnostics.push(incompatible(
                format!("findings[{index}].reassessment"),
                reassessment.finding_number(),
                current.policy_id,
            ));
        }
    }
    for (index, finding) in document.new_findings.iter().enumerate() {
        let fields = candidates::fields(finding).map_err(RevisionError::Candidate)?;
        if !slots.contains(fields.policy_id.as_str()) {
            diagnostics.push(incompatible(
                format!("new_findings[{index}]"),
                finding.finding_number(),
                fields.policy_id,
            ));
        }
    }
    let known: HashSet<u64> = document
        .findings
        .iter()
        .map(|slot| slot.id)
        .chain(
            document
                .new_findings
                .iter()
                .map(|finding| finding.finding_number()),
        )
        .collect();
    let unknown = |input: String, number: u64| {
        Diagnostic::input(
            "unknown_finding_reference",
            input,
            Some(number),
            "No carried or new finding has this ID; revision kept the entry.",
        )
    };
    for (index, answer) in document.answers.iter().enumerate() {
        if !known.contains(&answer.finding_number()) {
            diagnostics.push(unknown(
                format!("answers[{index}]"),
                answer.finding_number(),
            ));
        }
    }
    for (index, transition) in document.transitions.iter().enumerate() {
        if !known.contains(&transition.finding_number()) {
            diagnostics.push(unknown(
                format!("transitions[{index}]"),
                transition.finding_number(),
            ));
        }
    }
    Ok(())
}
