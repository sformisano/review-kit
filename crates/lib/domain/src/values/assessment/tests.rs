use std::error::Error;

use super::*;
use crate::{FindingDraft, LineNumber, Severity, SourceLocation, SourcePath, Suggestion};

type TestResult = Result<(), Box<dyn Error>>;

fn policy(raw: &str) -> Result<PolicyId, Box<dyn Error>> {
    Ok(PolicyId::try_from(raw.to_owned())?)
}

fn draft() -> Result<AssessmentDraft, Box<dyn Error>> {
    Ok(AssessmentDraft {
        subject: Subject::new("gitlab".into(), "host".into(), "repo".into(), 1)?,
        reviewer: ReviewerId::try_from("reviewer".to_owned())?,
        revision: Revision::new("head".into(), vec!["base".into()])?,
        decision: ReviewDecision::Ready,
        depth: "Changed functions and their callers".into(),
        scope: "Ownership checks in the request path".into(),
        limitation: None,
        selected_policies: vec![policy("safety")?, policy("proof")?],
        evaluations: vec![
            PolicyEvaluation::new(policy("safety")?, PolicyJudgment::Satisfied)?,
            PolicyEvaluation::new(policy("proof")?, PolicyJudgment::Satisfied)?,
        ],
        findings: vec![],
    })
}

fn finding(status: FindingStatus, disposition: Disposition) -> Result<Finding, Box<dyn Error>> {
    Ok(Finding::new(FindingDraft {
        id: FindingId::try_from(1)?,
        policy_id: policy("safety")?,
        title: "Unsafe access".into(),
        body: "Missing ownership check.".into(),
        severity: Severity::P2,
        disposition,
        status,
        evidence: "Observed on the cross-account path.".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/main.rs".to_owned())?,
            LineNumber::try_from(2)?,
        ),
        suggestion: None,
    })?)
}

#[test]
fn satisfied_policies_need_no_extra_evidence() -> TestResult {
    assert_eq!(
        Assessment::new(draft()?).map(|assessment| assessment.decision()),
        Ok(ReviewDecision::Ready)
    );
    assert_eq!(
        PolicyEvaluation::new(
            policy("safety")?,
            PolicyJudgment::Violated {
                evidence: " ".into()
            }
        ),
        Err(PolicyEvaluationError::MissingViolationEvidence)
    );
    assert_eq!(
        PolicyEvaluation::new(
            policy("proof")?,
            PolicyJudgment::Blocked {
                limitation: "\n".into()
            }
        ),
        Err(PolicyEvaluationError::MissingLimitation)
    );
    Ok(())
}

#[test]
fn blocked_assessment_retains_proven_violations() -> TestResult {
    let mut input = draft()?;
    input.decision = ReviewDecision::Blocked;
    input.limitation = Some("Integration environment unavailable.".into());
    input.evaluations = vec![
        PolicyEvaluation::new(
            policy("safety")?,
            PolicyJudgment::Violated {
                evidence: "Cross-account access reproduced.".into(),
            },
        )?,
        PolicyEvaluation::new(
            policy("proof")?,
            PolicyJudgment::Blocked {
                limitation: "Integration environment unavailable.".into(),
            },
        )?,
    ];
    let result = Assessment::new(input);
    assert!(result.is_ok());
    if let Ok(assessment) = result {
        assert_eq!(assessment.decision(), ReviewDecision::Blocked);
        assert!(matches!(
            assessment.evaluations()[0].judgment(),
            PolicyJudgment::Violated { .. }
        ));
    }
    Ok(())
}

#[test]
fn supplied_decisions_accept_consistent_finding_judgments_without_severity_inference() -> TestResult
{
    for (status, disposition, decision) in [
        (
            FindingStatus::Open,
            Disposition::Blocking,
            ReviewDecision::ChangesRequired,
        ),
        (
            FindingStatus::Open,
            Disposition::Advisory,
            ReviewDecision::Ready,
        ),
        (
            FindingStatus::Fixed,
            Disposition::Blocking,
            ReviewDecision::Ready,
        ),
        (
            FindingStatus::Withdrawn,
            Disposition::Blocking,
            ReviewDecision::Ready,
        ),
        (
            FindingStatus::Uncertain,
            Disposition::Advisory,
            ReviewDecision::Blocked,
        ),
    ] {
        let mut input = draft()?;
        input.decision = decision;
        if decision == ReviewDecision::Blocked {
            input.limitation = Some("Finding remains uncertain.".into());
        }
        input.findings.push(finding(status, disposition)?);
        assert_eq!(
            Assessment::new(input).map(|assessment| assessment.decision()),
            Ok(decision)
        );
    }
    Ok(())
}

#[test]
fn construction_reports_independent_completeness_errors_together() -> TestResult {
    let mut input = draft()?;
    input.selected_policies.push(policy("safety")?);
    input.evaluations = vec![
        PolicyEvaluation::new(policy("other")?, PolicyJudgment::Satisfied)?,
        PolicyEvaluation::new(policy("other")?, PolicyJudgment::Satisfied)?,
    ];
    let first = finding(FindingStatus::Open, Disposition::Blocking)?;
    input.findings = vec![first.clone(), first];
    let result = Assessment::new(input);
    assert!(result.is_err());
    if let Err(errors) = result {
        assert!(errors.contains(&AssessmentError::DuplicateSelectedPolicy(policy("safety")?)));
        assert!(errors.contains(&AssessmentError::UnselectedEvaluation(policy("other")?)));
        assert!(errors.contains(&AssessmentError::DuplicateEvaluation(policy("other")?)));
        assert!(errors.contains(&AssessmentError::MissingEvaluation(policy("proof")?)));
        assert!(errors.contains(&AssessmentError::DuplicateFinding(FindingId::try_from(1)?)));
    }
    Ok(())
}

#[test]
fn empty_policy_selection_does_not_invent_requirements() -> TestResult {
    let mut input = draft()?;
    input.selected_policies.clear();
    input.evaluations.clear();
    assert_eq!(
        Assessment::new(input).map(|assessment| assessment.decision()),
        Ok(ReviewDecision::Ready)
    );
    Ok(())
}

#[test]
fn closed_history_does_not_require_a_current_policy_evaluation() -> TestResult {
    for status in [FindingStatus::Fixed, FindingStatus::Withdrawn] {
        let mut input = draft()?;
        input.selected_policies.clear();
        input.evaluations.clear();
        input.findings.push(finding(status, Disposition::Blocking)?);
        assert!(Assessment::new(input).is_ok());
    }
    let mut input = draft()?;
    input.decision = ReviewDecision::ChangesRequired;
    input.selected_policies.clear();
    input.evaluations.clear();
    input
        .findings
        .push(finding(FindingStatus::Open, Disposition::Blocking)?);
    assert_eq!(
        Assessment::new(input),
        Err(vec![AssessmentError::UnselectedFindingPolicy(
            FindingId::try_from(1)?
        )])
    );
    Ok(())
}

#[test]
fn suggestion_revision_must_match_the_assessment() -> TestResult {
    let mut input = draft()?;
    input.decision = ReviewDecision::ChangesRequired;
    let location = SourceLocation::new(
        SourcePath::try_from("src/lib.rs".to_owned())?,
        LineNumber::try_from(1)?,
    );
    let suggestion = Suggestion::new(
        location.clone(),
        Revision::new("stale-head".into(), vec![])?,
        LineNumber::try_from(1)?,
        vec!["old".into()],
        vec!["new".into()],
        "correction proof".into(),
    )?;
    input.findings.push(Finding::new(FindingDraft {
        id: FindingId::try_from(1)?,
        policy_id: policy("safety")?,
        title: "Unsafe access".into(),
        body: "Missing check.".into(),
        severity: Severity::P1,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Reproduction.".into(),
        validated: true,
        location,
        suggestion: Some(suggestion),
    })?);
    assert_eq!(
        Assessment::new(input.clone()),
        Err(vec![AssessmentError::SuggestionRevisionMismatch(
            FindingId::try_from(1)?
        )])
    );
    for status in [
        FindingStatus::Fixed,
        FindingStatus::Withdrawn,
        FindingStatus::Uncertain,
    ] {
        let mut reassessed = input.clone();
        reassessed.findings[0]
            .transition(status, "Fresh reassessment at the new revision.".into())?;
        if status == FindingStatus::Uncertain {
            reassessed.decision = ReviewDecision::Blocked;
            reassessed.limitation = Some("Finding remains uncertain.".into());
        }
        let historical = reassessed.findings[0].suggestion().cloned();
        let assessment = Assessment::new(reassessed.clone());
        assert!(assessment.is_ok());
        if let Ok(assessment) = assessment {
            assert_eq!(assessment.findings()[0].suggestion(), historical.as_ref());
        }
        reassessed.findings[0].transition(
            FindingStatus::Open,
            "The defect is actionable again.".into(),
        )?;
        reassessed.decision = ReviewDecision::ChangesRequired;
        assert_eq!(
            Assessment::new(reassessed),
            Err(vec![AssessmentError::SuggestionRevisionMismatch(
                FindingId::try_from(1)?
            )])
        );
    }
    Ok(())
}

#[test]
fn overall_agent_judgments_are_preserved_without_inference() -> TestResult {
    for decision in [ReviewDecision::ChangesRequired, ReviewDecision::Blocked] {
        let mut input = draft()?;
        input.decision = decision;
        input.depth = "project-specific review depth".into();
        input.scope = "  source and related contracts  ".into();
        input.limitation =
            Some("Review context outside these policy results is unavailable.".into());
        let result = Assessment::new(input.clone());
        assert!(result.is_ok());
        if let Ok(assessment) = result {
            assert_eq!(assessment.decision(), decision);
            assert_eq!(assessment.depth(), input.depth);
            assert_eq!(assessment.scope(), input.scope);
            assert_eq!(assessment.limitation(), input.limitation.as_deref());
        }
    }
    Ok(())
}

#[test]
fn metadata_and_decision_errors_are_collected_together() -> TestResult {
    let mut input = draft()?;
    input.depth = " \n".into();
    input.scope = "\t".into();
    input.limitation = Some(" ".into());
    input.evaluations[0] = PolicyEvaluation::new(
        policy("safety")?,
        PolicyJudgment::Violated {
            evidence: "The bypass is reproduced.".into(),
        },
    )?;
    input
        .findings
        .push(finding(FindingStatus::Open, Disposition::Blocking)?);
    assert_eq!(
        Assessment::new(input),
        Err(vec![
            AssessmentError::BlankDepth,
            AssessmentError::BlankScope,
            AssessmentError::BlankLimitation,
            AssessmentError::ReadyWithViolation(policy("safety")?),
            AssessmentError::ReadyWithBlockingFinding(FindingId::try_from(1)?),
        ])
    );
    let mut blocked = draft()?;
    blocked.decision = ReviewDecision::Blocked;
    assert_eq!(
        Assessment::new(blocked),
        Err(vec![AssessmentError::MissingBlockedLimitation])
    );
    Ok(())
}

#[test]
fn incomplete_evaluations_reject_both_nonblocked_decisions() -> TestResult {
    for decision in [ReviewDecision::Ready, ReviewDecision::ChangesRequired] {
        let mut input = draft()?;
        input.decision = decision;
        input.evaluations[0] = PolicyEvaluation::new(
            policy("safety")?,
            PolicyJudgment::Blocked {
                limitation: "Required trace is missing.".into(),
            },
        )?;
        assert_eq!(
            Assessment::new(input),
            Err(vec![AssessmentError::IncompleteDecision(decision)])
        );
        let mut input = draft()?;
        input.decision = decision;
        input
            .findings
            .push(finding(FindingStatus::Uncertain, Disposition::Advisory)?);
        assert_eq!(
            Assessment::new(input),
            Err(vec![AssessmentError::IncompleteDecision(decision)])
        );
    }
    Ok(())
}

#[test]
fn unvalidated_findings_require_an_explicit_blocked_decision() -> TestResult {
    let unvalidated = Finding::new(FindingDraft {
        id: FindingId::try_from(1)?,
        policy_id: policy("safety")?,
        title: "Possible ownership gap".into(),
        body: "An ownership guard may be missing.".into(),
        severity: Severity::P2,
        disposition: Disposition::Advisory,
        status: FindingStatus::Open,
        evidence: "The relevant request path was identified.".into(),
        validated: false,
        location: SourceLocation::new(
            SourcePath::try_from("src/main.rs".to_owned())?,
            LineNumber::try_from(2)?,
        ),
        suggestion: None,
    })?;
    for decision in [ReviewDecision::Ready, ReviewDecision::ChangesRequired] {
        let mut input = draft()?;
        input.decision = decision;
        input.findings.push(unvalidated.clone());
        assert_eq!(
            Assessment::new(input),
            Err(vec![AssessmentError::IncompleteDecision(decision)])
        );
    }
    let mut input = draft()?;
    input.decision = ReviewDecision::Blocked;
    input.limitation = Some("Independent finding check is unavailable.".into());
    input.findings.push(unvalidated);
    assert!(Assessment::new(input).is_ok());
    Ok(())
}
