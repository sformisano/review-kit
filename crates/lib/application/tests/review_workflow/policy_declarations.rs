use super::*;
use review_kit_application::use_cases::checkpoints::ApplicabilityStatus;

fn excluded() -> PublicPolicyDeclaration {
    PublicPolicyDeclaration {
        policy: PolicyName {
            id: PolicyId::try_from("async-work".to_owned()).unwrap(),
            name: "Async Work".into(),
        },
        status: ApplicabilityStatus::Excluded,
        reason: "The change contains no asynchronous work.".into(),
    }
}

fn declared(blocked: bool, finding: Option<FindingStatus>) -> PublishRequest {
    let mut request = request("initial", finding, blocked);
    request.policy_declarations.push(excluded());
    request
}

#[test]
fn every_public_declaration_difference_persists_a_successor_without_duplicate_effects() {
    for change in 0..6 {
        let (mut host, mut source, mut renderer) = fixtures();
        let first = declared(false, None);
        publish(&first, &mut host, &mut source, &mut renderer).unwrap();
        let original = host.snapshot.summaries[0].current.clone();
        host.writes.clear();
        assert_eq!(
            publish(&first, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Unchanged
        );
        assert!(host.writes.is_empty());
        let mut next = first.clone();
        match change {
            0 => {
                next.policy_declarations[1].reason = "Only synchronous control flow changes.".into()
            }
            1 => next.policy_declarations[1].policy.name = "Async Work Placement".into(),
            2 => {
                next.policy_declarations[1].policy.id =
                    PolicyId::try_from("other-excluded".to_owned()).unwrap()
            }
            3 => next.policy_declarations.reverse(),
            4 => {
                next.policy_declarations.pop();
            }
            _ => {
                let mut added = excluded();
                added.policy.id = PolicyId::try_from("ownership".to_owned()).unwrap();
                next.policy_declarations.push(added);
            }
        }
        let result = publish(&next, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(
            result.status,
            PublicationStatus::Completed,
            "change {change}"
        );
        assert_eq!(result.round, 2);
        assert_eq!(
            effects(&host),
            vec![Effect::UpdateSummary, Effect::UpdateSummary]
        );
        assert_eq!(host.snapshot.summaries[0].history, vec![original]);
        assert_eq!(
            host.snapshot.summaries[0].current.policy_declarations,
            next.policy_declarations
        );
        host.writes.clear();
        assert_eq!(
            publish(&next, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Unchanged
        );
        assert!(host.writes.is_empty());
    }
}

#[test]
fn a_terminal_blocked_round_advances_when_an_exclusion_reason_changes() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut request = declared(true, None);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    let original = host.snapshot.summaries[0].current.clone();
    assert_eq!(original.status, RoundStatus::Blocked);
    request.policy_declarations[1].reason = "Only synchronous parsing changes.".into();
    host.writes.clear();
    let result = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(result.round, 2);
    assert_eq!(host.snapshot.summaries[0].history, vec![original]);
    assert!(
        effects(&host)
            .iter()
            .all(|effect| *effect == Effect::UpdateSummary)
    );
}

#[test]
fn changed_applicability_advances_even_with_the_same_blocked_judgment() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut request = declared(true, None);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    let original = host.snapshot.summaries[0].current.clone();
    request.policy_declarations[0].status = ApplicabilityStatus::Unresolved;
    let result = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.round, 2);
    assert_eq!(host.snapshot.summaries[0].history, vec![original]);
    assert_eq!(
        host.snapshot.summaries[0].current.assessment,
        request.assessment
    );
    assert_eq!(
        host.snapshot.summaries[0].current.policy_declarations,
        request.policy_declarations
    );
}

#[test]
fn a_declaration_change_does_not_authorize_a_different_public_review_identity() {
    for workflow in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let mut first = declared(false, None);
        first.review_workflow = Some("original-workflow".into());
        publish(&first, &mut host, &mut source, &mut renderer).unwrap();
        let retained = host.snapshot.clone();
        let mut next = first;
        next.policy_declarations[1].reason = "A revised public exclusion reason.".into();
        if workflow {
            next.review_workflow = Some("different-workflow".into());
        } else {
            next.policies = AssessmentPolicies::new(
                "different-policy-set".into(),
                next.policies.policies().to_vec(),
            )
            .unwrap();
        }
        host.writes.clear();
        assert!(matches!(
            publish(&next, &mut host, &mut source, &mut renderer),
            Err(PlanError::ChangedReviewIdentity)
        ));
        assert!(host.writes.is_empty());
        assert_eq!(host.snapshot, retained);
    }
}

#[test]
fn a_partial_round_refuses_a_changed_declaration_and_recovers_the_original_after_restart() {
    let (mut host, mut source, mut renderer) = fixtures();
    let original = declared(false, Some(FindingStatus::Open));
    host.fault = Some(Fault::Rejected(Effect::Finding));
    assert_eq!(
        publish(&original, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    let retained = host.snapshot.clone();
    let progress = host.progress.clone();
    let mut altered = original.clone();
    altered.policy_declarations[1].reason = "An edited recovery declaration.".into();
    host.writes.clear();
    assert!(matches!(
        publish(&altered, &mut host, &mut source, &mut renderer),
        Err(PlanError::ChangedPendingAssessment)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.snapshot, retained);
    assert_eq!(host.progress, progress);
    let (mut restarted, mut source, mut renderer) = fixtures();
    restarted.snapshot = retained;
    restarted.progress = progress;
    let result = publish(&original, &mut restarted, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, 1);
    assert_eq!(
        restarted
            .snapshot
            .findings
            .iter()
            .filter(|root| root.author == *original.assessment.reviewer())
            .count(),
        1
    );
    assert_eq!(
        restarted.snapshot.summaries[0].current.policy_declarations,
        original.policy_declarations
    );
}

#[test]
fn declarations_reject_duplicate_blank_missing_excluded_and_unresolved_slot_shapes() {
    let request = declared(false, None);
    for variant in 0..6 {
        let mut rows = request.policy_declarations.clone();
        match variant {
            0 => rows.push(rows[0].clone()),
            1 => rows[0].reason = " \n".into(),
            2 => rows[0].policy.name = " ".into(),
            3 => {
                rows.remove(0);
            }
            4 => rows[0].status = ApplicabilityStatus::Excluded,
            _ => rows[0].status = ApplicabilityStatus::Unresolved,
        }
        assert!(
            validate_policy_declarations(&rows, &request.assessment, request.policies.policies())
                .is_err(),
            "variant {variant}"
        );
    }
    let mut blocked = declared(true, None);
    assert!(
        validate_policy_declarations(
            &blocked.policy_declarations,
            &blocked.assessment,
            blocked.policies.policies()
        )
        .is_ok()
    );
    blocked.policy_declarations[0].status = ApplicabilityStatus::Unresolved;
    assert!(
        validate_policy_declarations(
            &blocked.policy_declarations,
            &blocked.assessment,
            blocked.policies.policies()
        )
        .is_ok()
    );
}
