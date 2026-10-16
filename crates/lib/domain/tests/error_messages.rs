use std::error::Error;

use review_kit_domain::{
    AssessmentError, BlankPolicyId, FindingError, FindingId, FindingIdExhausted, IdentityError,
    InvalidFindingId, InvalidLineNumber, PolicyEvaluationError, PolicyId, ReviewDecision,
    SourceError,
};

#[test]
fn finding_errors_use_unprefixed_numeric_identifiers() -> Result<(), InvalidFindingId> {
    for number in [1, u64::MAX] {
        let id = FindingId::try_from(number)?;
        assert_eq!(id.to_string(), number.to_string());
        for (error, expected) in [
            (
                AssessmentError::ReadyWithBlockingFinding(id),
                format!("ready decision conflicts with open blocking finding {number}"),
            ),
            (
                AssessmentError::DuplicateFinding(id),
                format!("finding {number} appears more than once"),
            ),
            (
                AssessmentError::UnselectedFindingPolicy(id),
                format!("finding {number} names an unselected policy"),
            ),
            (
                AssessmentError::SuggestionRevisionMismatch(id),
                format!("finding {number} suggestion uses a different revision"),
            ),
        ] {
            assert_eq!(error.to_string(), expected);
            assert!(error.source().is_none());
        }
    }
    Ok(())
}

#[test]
fn policy_errors_preserve_exact_identifier_spelling() -> Result<(), BlankPolicyId> {
    let id = PolicyId::try_from(" Team/règle.v2 ".to_owned())?;
    assert_eq!(id.to_string(), " Team/règle.v2 ");
    for (error, expected) in [
        (
            AssessmentError::ReadyWithViolation(id.clone()),
            "ready decision conflicts with violated policy  Team/règle.v2 ",
        ),
        (
            AssessmentError::DuplicateSelectedPolicy(id.clone()),
            "policy  Team/règle.v2  is selected more than once",
        ),
        (
            AssessmentError::UnselectedEvaluation(id.clone()),
            "evaluation names unselected policy  Team/règle.v2 ",
        ),
        (
            AssessmentError::DuplicateEvaluation(id.clone()),
            "policy  Team/règle.v2  is evaluated more than once",
        ),
        (
            AssessmentError::MissingEvaluation(id.clone()),
            "selected policy  Team/règle.v2  has no evaluation",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
    }
    Ok(())
}

#[test]
fn rejections_preserve_field_names_and_debug_decisions() {
    assert_eq!(
        IdentityError::Blank("comparison reference").to_string(),
        "comparison reference identity must not be blank"
    );
    assert_eq!(
        FindingError::Blank("transition evidence").to_string(),
        "finding transition evidence must not be blank"
    );
    for (decision, expected) in [
        (
            ReviewDecision::Ready,
            "decision Ready requires complete policy and finding judgments",
        ),
        (
            ReviewDecision::ChangesRequired,
            "decision ChangesRequired requires complete policy and finding judgments",
        ),
        (
            ReviewDecision::Blocked,
            "decision Blocked requires complete policy and finding judgments",
        ),
    ] {
        assert_eq!(
            AssessmentError::IncompleteDecision(decision).to_string(),
            expected
        );
    }
}

#[test]
fn domain_rejections_have_no_source_errors() -> Result<(), BlankPolicyId> {
    let id = PolicyId::try_from("policy".to_owned())?;
    let errors: [&dyn Error; 9] = [
        &FindingIdExhausted,
        &InvalidFindingId,
        &BlankPolicyId,
        &InvalidLineNumber,
        &IdentityError::Blank("head"),
        &SourceError::OriginalMismatch,
        &FindingError::Blank("body"),
        &PolicyEvaluationError::MissingLimitation,
        &AssessmentError::ReadyWithViolation(id.clone()),
    ];
    for error in errors {
        assert!(error.source().is_none(), "{error}");
    }
    Ok(())
}
