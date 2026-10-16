#![allow(clippy::unwrap_used, clippy::expect_used)]

use review_kit_application::{
    review_run::{ContentRef, RunId},
    use_cases::checkpoints::*,
};
use review_kit_domain::{
    Disposition, Finding, FindingDraft, FindingId, FindingStatus, LineNumber, PolicyId,
    PolicyJudgment, Revision, Severity, SourceLocation, SourcePath,
};

fn run() -> RunId {
    RunId::try_from("00000000-0000-4000-8000-000000000001".to_owned()).unwrap()
}
fn id(value: &str) -> PolicyId {
    PolicyId::try_from(value.to_owned()).unwrap()
}
fn revision() -> Revision {
    Revision::new("a".repeat(40), vec!["b".repeat(40)]).unwrap()
}
fn reference(digit: char) -> ContentRef {
    ContentRef::try_from(format!("sha256:{}", digit.to_string().repeat(64))).unwrap()
}
fn row(policy: &str, outcome: PolicyOutcome) -> ReviewPolicyDraft {
    ReviewPolicyDraft {
        policy_id: policy.into(),
        name: policy.into(),
        required: false,
        reason: "The changed surface requires this check.".into(),
        outcome: Some(outcome),
    }
}
fn draft() -> ReviewDeclarationsDraft {
    ReviewDeclarationsDraft {
        run_id: run().as_str().into(),
        revision: revision(),
        policy_set: "Review policies".into(),
        supersedes: None,
        reason: "Review the captured source.".into(),
        policies: vec![
            row("baseline", PolicyOutcome::Satisfied),
            row("frontend", PolicyOutcome::Excluded),
            row(
                "unknown",
                PolicyOutcome::Unresolved {
                    limitation: "Definition unavailable.".into(),
                },
            ),
        ],
    }
}
fn check(draft: &ReviewDeclarationsDraft) -> Result<ReviewDeclarations, Vec<ReviewIssue>> {
    validate_declarations(
        draft,
        &ReviewBinding {
            run_id: &run(),
            revision: &revision(),
            carried_findings: &[],
            successor: Successor::Current(None),
        },
    )
}
fn finding(number: u64, policy: &str, status: FindingStatus) -> Finding {
    Finding::new(FindingDraft {
        id: FindingId::try_from(number).unwrap(),
        policy_id: id(policy),
        title: "Validate quantities".into(),
        body: "Zero quantities bypass the constructor.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Derived Deserialize skips Quantity::new.".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/quantity.rs".to_owned()).unwrap(),
            LineNumber::try_from(3).unwrap(),
        ),
        suggestion: None,
    })
    .unwrap()
}

#[test]
fn complete_declaration_keeps_order_and_derives_distinct_outcomes() {
    let result = check(&draft()).unwrap();
    assert_eq!(result.policy_set(), "Review policies");
    assert_eq!(
        result
            .policies()
            .iter()
            .map(|p| p.policy_id().as_str())
            .collect::<Vec<_>>(),
        ["baseline", "frontend", "unknown"]
    );
    assert_eq!(result.policies()[1].status(), ApplicabilityStatus::Excluded);
    assert_eq!(result.policies()[1].judgment(), None);
    assert_eq!(
        result.policies()[2].status(),
        ApplicabilityStatus::Unresolved
    );
    assert!(matches!(
        result.policies()[2].judgment(),
        Some(PolicyJudgment::Blocked { .. })
    ));
    assert_eq!(
        PolicyOutcome::Blocked {
            limitation: "Missing proof".into()
        }
        .status(),
        ApplicabilityStatus::Applicable
    );
}

#[test]
fn all_empty_declarations_are_reported_without_inventing_coverage() {
    let mut d = draft();
    d.policy_set.clear();
    d.policies.clear();
    d.reason.clear();
    let errors = check(&d).unwrap_err();
    for issue in [
        ReviewIssue::BlankPolicySet,
        ReviewIssue::NoPolicies,
        ReviewIssue::BlankReason,
    ] {
        assert!(errors.contains(&issue));
    }
}

#[test]
fn identity_revision_and_successor_are_all_checked() {
    let mut d = draft();
    d.run_id = "other".into();
    d.revision = Revision::new("c".repeat(40), vec![]).unwrap();
    d.supersedes = Some("invalid".into());
    let issues = check(&d).unwrap_err();
    assert!(issues.contains(&ReviewIssue::RevisionBinding));
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, ReviewIssue::RunBinding { .. }))
    );
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, ReviewIssue::StaleSuccessor { .. }))
    );
    let mut d = draft();
    d.supersedes = Some(reference('b').as_str().into());
    assert!(check(&d).is_err());
    let binding = ReviewBinding {
        run_id: &run(),
        revision: &revision(),
        carried_findings: &[],
        successor: Successor::Current(Some(&reference('b'))),
    };
    assert!(validate_declarations(&d, &binding).is_ok());
    let retained = ReviewBinding {
        successor: Successor::Retained,
        ..binding
    };
    assert!(validate_declarations(&d, &retained).is_ok());
    d.supersedes = Some("invalid".into());
    assert!(matches!(
        validate_declarations(&d, &retained).unwrap_err()[0],
        ReviewIssue::InvalidSupersedes(_)
    ));
}

#[test]
fn row_errors_and_duplicates_are_reported_together() {
    let mut d = draft();
    d.policies[0].name.clear();
    d.policies.push(d.policies[0].clone());
    d.policies.push(row("", PolicyOutcome::Satisfied));
    let errors = check(&d).unwrap_err();
    for predicate in [
        errors
            .iter()
            .any(|i| matches!(i, ReviewIssue::BlankPolicyName(_))),
        errors
            .iter()
            .any(|i| matches!(i, ReviewIssue::DuplicatePolicy(_))),
        errors
            .iter()
            .any(|i| matches!(i, ReviewIssue::BlankPolicyId(_))),
    ] {
        assert!(predicate, "{errors:?}");
    }
}

#[test]
fn required_and_missing_outcome_rules_survive_combining_the_row() {
    let mut d = draft();
    d.policies[1].required = true;
    d.policies[2].outcome = None;
    let errors = check(&d).unwrap_err();
    assert!(errors.contains(&ReviewIssue::RequiredPolicyExcluded(id("frontend"))));
    assert!(errors.contains(&ReviewIssue::MissingOutcome(id("unknown"))));
    d.policies[1].required = false;
    d.policies[2].outcome = Some(PolicyOutcome::Excluded);
    assert!(check(&d).is_ok());
}

#[test]
fn active_carried_findings_cannot_disappear_from_declared_coverage() {
    let d = draft();
    for status in [
        FindingStatus::Open,
        FindingStatus::Uncertain,
        FindingStatus::Fixed,
        FindingStatus::Withdrawn,
    ] {
        let findings = vec![finding(7, "carried", status)];
        let result = validate_declarations(
            &d,
            &ReviewBinding {
                run_id: &run(),
                revision: &revision(),
                carried_findings: &findings,
                successor: Successor::Current(None),
            },
        );
        assert_eq!(
            result.is_ok(),
            matches!(status, FindingStatus::Fixed | FindingStatus::Withdrawn)
        );
    }
}
