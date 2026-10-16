#![allow(clippy::unwrap_used, clippy::expect_used)]

use review_kit_application::use_cases::checkpoints::{
    Confirmation, ConfirmationIssue, ConfirmationResult, admit_findings,
};
use review_kit_domain::{
    Disposition, Finding, FindingDraft, FindingId, FindingStatus, LineNumber, PolicyId, Severity,
    SourceLocation, SourcePath,
};

fn number(value: u64) -> FindingId {
    FindingId::try_from(value).unwrap()
}

fn draft(id: u64) -> FindingDraft {
    FindingDraft {
        id: number(id),
        policy_id: PolicyId::try_from("money".to_owned()).unwrap(),
        title: "Round fees half away from zero".into(),
        body: "A 28.5-cent fee rounds down to 28 cents.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "percentage_fee(Cents(5_000), 0.57) returns Cents(28).".into(),
        validated: false,
        location: SourceLocation::new(
            SourcePath::try_from("src/fees.rs".to_owned()).unwrap(),
            LineNumber::try_from(5).unwrap(),
        ),
        suggestion: None,
    }
}

fn finding(id: u64) -> Finding {
    Finding::new(draft(id)).unwrap()
}

fn with(id: u64, change: impl FnOnce(&mut FindingDraft)) -> Finding {
    let mut data = draft(id);
    change(&mut data);
    Finding::new(data).unwrap()
}

fn checked(found: Finding, result: ConfirmationResult) -> Confirmation {
    Confirmation::Checked {
        result,
        checked: Box::new(found),
        basis: "Reproduced the 28-cent result against the reviewed source.".into(),
    }
}

fn retained(id: u64) -> Confirmation {
    Confirmation::Retained {
        finding: number(id),
        basis: "The earlier isolated check covers this unchanged version.".into(),
    }
}

fn with_basis(confirmation: Confirmation, text: &str) -> Confirmation {
    match confirmation {
        Confirmation::Checked {
            result, checked, ..
        } => Confirmation::Checked {
            result,
            checked,
            basis: text.into(),
        },
        Confirmation::Retained { finding, .. } => Confirmation::Retained {
            finding,
            basis: text.into(),
        },
    }
}

fn validated(findings: &[Finding], id: u64) -> bool {
    findings
        .iter()
        .find(|finding| finding.id() == number(id))
        .unwrap()
        .validated()
}

#[test]
fn an_exact_checked_confirmation_validates_an_open_blocking_finding() {
    let confirmation = checked(
        finding(1).with_validated(true),
        ConfirmationResult::Confirmed,
    );
    let admitted = admit_findings(vec![finding(1)], &[], &[confirmation]).unwrap();
    assert!(validated(&admitted, 1));
}

#[test]
fn a_blocking_finding_without_a_confirmed_verdict_stays_unvalidated() {
    let admitted = admit_findings(vec![finding(1)], &[], &[]).unwrap();
    assert!(!validated(&admitted, 1));
    for result in [
        ConfirmationResult::Rejected,
        ConfirmationResult::EvidenceGap,
    ] {
        let admitted =
            admit_findings(vec![finding(1)], &[], &[checked(finding(1), result)]).unwrap();
        assert!(!validated(&admitted, 1), "{result:?}");
    }
}

#[test]
fn a_confirmation_of_another_version_is_stale_and_admits_nothing() {
    let reworded = with(1, |data| {
        data.body = "Fees lose a cent at 57 basis points.".into()
    });
    let issues = admit_findings(
        vec![reworded],
        &[],
        &[checked(finding(1), ConfirmationResult::Confirmed)],
    )
    .unwrap_err();
    assert_eq!(issues, [ConfirmationIssue::StaleConfirmation(number(1))]);
    let severity = with(1, |data| data.severity = Severity::P3);
    assert!(
        admit_findings(
            vec![severity],
            &[],
            &[checked(finding(1), ConfirmationResult::Confirmed)]
        )
        .is_err()
    );
}

#[test]
fn a_validation_claim_without_a_matching_confirmation_is_rejected() {
    let issues = admit_findings(vec![finding(1).with_validated(true)], &[], &[]).unwrap_err();
    assert_eq!(
        issues,
        [ConfirmationIssue::UnconfirmedBlockingFinding(number(1))]
    );
    let uncertain = with(1, |data| {
        data.status = FindingStatus::Uncertain;
        data.validated = true;
    });
    assert_eq!(
        admit_findings(vec![uncertain], &[], &[]).unwrap_err(),
        [ConfirmationIssue::UnconfirmedBlockingFinding(number(1))]
    );
}

#[test]
fn a_retained_confirmation_needs_an_unchanged_reusable_captured_version() {
    let reusable = [finding(1).with_validated(true)];
    let admitted = admit_findings(vec![finding(1)], &reusable, &[retained(1)]).unwrap();
    assert!(validated(&admitted, 1));
    let reworded = with(1, |data| {
        data.evidence = "Observed 28 cents in the probe.".into()
    });
    assert_eq!(
        admit_findings(vec![reworded], &reusable, &[retained(1)]).unwrap_err(),
        [ConfirmationIssue::InvalidRetainedConfirmation(number(1))]
    );
    assert_eq!(
        admit_findings(vec![finding(1)], &[], &[retained(1)]).unwrap_err(),
        [ConfirmationIssue::InvalidRetainedConfirmation(number(1))]
    );
}

#[test]
fn confirmations_must_name_one_final_finding_once_with_a_basis() {
    let issues = admit_findings(
        vec![finding(1)],
        &[],
        &[
            checked(finding(2), ConfirmationResult::Confirmed),
            checked(finding(1), ConfirmationResult::Confirmed),
            with_basis(retained(1), " "),
        ],
    )
    .unwrap_err();
    assert!(issues.contains(&ConfirmationIssue::UnknownFinding(number(2))));
    assert!(issues.contains(&ConfirmationIssue::DuplicateConfirmation(number(1))));
    let blank = admit_findings(
        vec![finding(1)],
        &[],
        &[with_basis(
            checked(finding(1), ConfirmationResult::Confirmed),
            "\t",
        )],
    )
    .unwrap_err();
    assert_eq!(blank, [ConfirmationIssue::BlankBasis(number(1))]);
}

#[test]
fn advisory_and_closed_findings_keep_their_authored_validation() {
    let advisory = with(1, |data| {
        data.disposition = Disposition::Advisory;
        data.validated = true;
    });
    let fixed = with(2, |data| {
        data.status = FindingStatus::Fixed;
        data.validated = true;
    });
    let withdrawn = with(3, |data| data.status = FindingStatus::Withdrawn);
    let admitted = admit_findings(vec![advisory, fixed, withdrawn], &[], &[]).unwrap();
    assert!(validated(&admitted, 1));
    assert!(validated(&admitted, 2));
    assert!(!validated(&admitted, 3));
}

#[test]
fn rejections_and_evidence_gaps_are_records_that_never_need_a_matching_finding() {
    let withdrawn = with(1, |data| data.status = FindingStatus::Withdrawn);
    let admitted = admit_findings(
        vec![withdrawn],
        &[],
        &[
            checked(finding(1), ConfirmationResult::Rejected),
            checked(finding(2), ConfirmationResult::EvidenceGap),
        ],
    )
    .unwrap();
    assert!(!validated(&admitted, 1));
    assert_eq!(
        admit_findings(vec![finding(1)], &[], &[retained(2)]).unwrap_err(),
        [ConfirmationIssue::InvalidRetainedConfirmation(number(2))]
    );
}

#[test]
fn a_confirmation_of_a_since_withdrawn_finding_is_a_record_only() {
    let withdrawn = with(1, |data| {
        data.status = FindingStatus::Withdrawn;
        data.evidence = "The caller already rejects the input.".into();
    });
    let admitted = admit_findings(
        vec![withdrawn],
        &[],
        &[checked(finding(1), ConfirmationResult::Confirmed)],
    )
    .unwrap();
    assert!(!validated(&admitted, 1));
}
