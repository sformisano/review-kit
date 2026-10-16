use super::*;
use review_kit_application::use_cases::checkpoints::ApplicabilityStatus;

fn blocked_request(status: FindingStatus) -> PublishRequest {
    let mut req = request("partial checks", Some(status), true);
    let proof = PolicyId::try_from("proof".to_owned()).unwrap();
    req.policies = AssessmentPolicies::new(
        "test-policies".into(),
        vec![
            selected().policies()[0].clone(),
            PolicyName {
                id: proof.clone(),
                name: "Proof".into(),
            },
        ],
    )
    .unwrap();
    req.policy_declarations = req
        .policies
        .policies()
        .iter()
        .map(|policy| PublicPolicyDeclaration {
            policy: policy.clone(),
            status: ApplicabilityStatus::Applicable,
            reason: "The changed failure path requires this policy.".into(),
        })
        .collect();
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::Blocked,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: Some("The integration runner is unavailable.".into()),
        selected_policies: vec![policy(), proof.clone()],
        evaluations: vec![
            PolicyEvaluation::new(
                policy(),
                PolicyJudgment::Violated {
                    evidence: "The failure path loses the request.".into(),
                },
            )
            .unwrap(),
            PolicyEvaluation::new(
                proof,
                PolicyJudgment::Blocked {
                    limitation: "The integration runner is unavailable.".into(),
                },
            )
            .unwrap(),
        ],
        findings: vec![finding(status)],
    })
    .unwrap();
    req
}

#[test]
fn validated_finding_is_planned_while_unrelated_policy_stays_blocked() {
    let (host, mut source, mut renderer) = fixtures();
    let req = blocked_request(FindingStatus::Open);
    let plan = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert_eq!(plan.round.assessment.decision(), ReviewDecision::Blocked);
    assert_eq!(plan.round.status, RoundStatus::Pending);
    assert!(
        matches!(plan.actions.as_slice(), [PlannedAction::Finding { finding, .. }] if finding.validated())
    );
    assert_eq!(plan.round.policy_declarations, req.policy_declarations);
}

#[test]
fn normal_nonblocked_finding_and_satisfied_policy_keep_their_existing_contract() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("normal review", Some(FindingStatus::Open), false);
    assert!(matches!(
        req.assessment.evaluations()[0].judgment(),
        PolicyJudgment::Satisfied
    ));
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::CreateSummary,
            Effect::Finding,
            Effect::UpdateSummary
        ]
    );
}

#[test]
fn blocked_round_can_recover_with_an_ineligible_candidate_retained_beside_a_validated_finding() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = blocked_request(FindingStatus::Open);
    let candidate = copy_finding(
        &finding(FindingStatus::Open),
        FindingId::try_from(2).unwrap(),
        req.policies.policies()[1].id.clone(),
        None,
    );
    replace_findings(&mut req, vec![finding(FindingStatus::Open), candidate]);
    req.allow_colocated = true;
    host.reject_finding = Some(FindingId::try_from(1).unwrap());
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Blocked
    );
    let owned: Vec<_> = host
        .snapshot
        .findings
        .iter()
        .filter(|root| root.author == reviewer("agent"))
        .collect();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].finding.id().get(), 1);
    assert_eq!(
        host.snapshot.summaries[0]
            .current
            .assessment
            .findings()
            .len(),
        2
    );
}

#[test]
fn source_drift_after_the_pending_summary_stops_the_eligible_blocked_finding() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.drift_after_pending = true;
    let result = publish(
        &blocked_request(FindingStatus::Open),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Superseded);
    assert_eq!(result.decision, ReviewDecision::Blocked);
    assert!(!effects(&host).contains(&Effect::Finding));
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Superseded
    );
}

#[test]
fn blocked_finding_requires_a_safe_diff_position() {
    struct UnsafePosition(Source);
    impl CommittedSource for UnsafePosition {
        fn lines(
            &mut self,
            subject: &Subject,
            revision: &Revision,
            path: &SourcePath,
        ) -> Result<Vec<String>, PortError> {
            self.0.lines(subject, revision, path)
        }
        fn validate_position(
            &mut self,
            _: &Subject,
            _: &Revision,
            _: &Finding,
        ) -> Result<(), PortError> {
            Err(failure("unsafe diff position"))
        }
    }
    let (host, source, mut renderer) = fixtures();
    let mut source = UnsafePosition(source);
    let error = plan_from_snapshot(
        &blocked_request(FindingStatus::Open),
        &host.snapshot,
        &mut source,
        &mut renderer,
    )
    .unwrap_err();
    assert!(has_plan_error(
        &error,
        &|error| matches!(error, PlanError::Checks(issues) if issues.iter().any(|issue| matches!(issue.problem, review_kit_application::use_cases::assessment::CheckProblem::Position(_))))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn blocked_finding_communication_finishes_only_after_all_effects_and_repeats_without_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = blocked_request(FindingStatus::Open);
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.decision, ReviewDecision::Blocked);
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(
        effects(&host),
        vec![
            Effect::CreateSummary,
            Effect::Finding,
            Effect::UpdateSummary
        ]
    );
    assert_eq!(result.verified.len(), 3);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Blocked
    );
    assert_eq!(
        host.snapshot.summaries[0].current.assessment,
        req.assessment
    );
    host.writes.clear();
    let repeated = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(repeated.status, PublicationStatus::Unchanged);
    assert_eq!(repeated.round, result.round);
    assert!(host.writes.is_empty());
}

#[test]
fn unvalidated_and_uncertain_candidates_do_not_get_finding_actions() {
    for (validated, status) in [
        (false, FindingStatus::Open),
        (true, FindingStatus::Uncertain),
    ] {
        let (host, mut source, mut renderer) = fixtures();
        let mut req = blocked_request(FindingStatus::Open);
        let value = finding(status);
        replace_findings(
            &mut req,
            vec![
                Finding::new(FindingDraft {
                    id: value.id(),
                    policy_id: value.policy_id().clone(),
                    title: value.title().into(),
                    body: value.body().into(),
                    severity: value.severity(),
                    disposition: value.disposition(),
                    status,
                    evidence: value.evidence().into(),
                    validated,
                    location: value.location().clone(),
                    suggestion: None,
                })
                .unwrap(),
            ],
        );
        let plan = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap();
        assert!(plan.actions.is_empty());
        assert_eq!(plan.round.status, RoundStatus::Blocked);
    }
}

#[test]
fn finding_with_its_own_policy_blocked_still_has_no_actions() {
    let (host, mut source, mut renderer) = fixtures();
    let req = request(
        "incomplete finding criterion",
        Some(FindingStatus::Open),
        true,
    );
    assert!(
        plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer)
            .unwrap()
            .actions
            .is_empty()
    );
}

#[test]
fn blocked_communication_keeps_empty_and_narrow_grants() {
    for allowed in [vec![], vec![Effect::CreateSummary, Effect::UpdateSummary]] {
        let (mut host, mut source, mut renderer) = fixtures();
        let mut req = blocked_request(FindingStatus::Open);
        req.effects = Effects::new(allowed.clone());
        let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert!(
            host.writes
                .iter()
                .all(|write| allowed.contains(&write.effect()))
        );
        assert!(!matches!(
            result.status,
            PublicationStatus::Blocked | PublicationStatus::Completed
        ));
        assert!(
            !host
                .snapshot
                .findings
                .iter()
                .any(|root| root.author == reviewer("agent"))
        );
        assert!(
            host.snapshot
                .summaries
                .iter()
                .all(|summary| summary.current.status != RoundStatus::Blocked)
        );
    }
}

#[test]
fn blocked_finding_still_requires_fresh_source_and_read_only_planning() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = blocked_request(FindingStatus::Open);
    source.fail = true;
    assert!(publish(&req, &mut host, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
    source.fail = false;
    host.snapshot.revision =
        Revision::new("other".into(), vec!["base".into(), "start".into()]).unwrap();
    assert!(plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
}

#[test]
fn finding_rejection_does_not_make_a_blocked_round_terminal() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = blocked_request(FindingStatus::Open);
    host.reject_finding = Some(FindingId::try_from(1).unwrap());
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Partial
    );
    assert_eq!(
        host.snapshot.summaries[0].current.assessment.decision(),
        ReviewDecision::Blocked
    );
    host.reject_finding = None;
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Blocked
    );
    assert_eq!(
        host.snapshot
            .findings
            .iter()
            .filter(|root| root.author == reviewer("agent"))
            .count(),
        1
    );
}

#[test]
fn lost_summary_or_finding_response_is_reconciled_without_duplicates_or_decision_changes() {
    for effect in [Effect::CreateSummary, Effect::Finding] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = blocked_request(FindingStatus::Open);
        host.fault = Some(Fault::UnknownCommitted(effect));
        let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(recovered.status, PublicationStatus::Blocked);
        assert_eq!(recovered.decision, ReviewDecision::Blocked);
        assert_eq!(
            effects(&host),
            vec![
                Effect::CreateSummary,
                Effect::Finding,
                Effect::UpdateSummary
            ]
        );
        assert_eq!(
            host.snapshot.summaries[0].current.policy_declarations,
            req.policy_declarations
        );
    }
}

#[test]
fn missing_finding_policy_from_declared_coverage_cannot_publish_a_validated_finding() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = blocked_request(FindingStatus::Open);
    req.policies = AssessmentPolicies::new(
        "test-policies".into(),
        vec![req.policies.policies()[1].clone()],
    )
    .unwrap();
    let errors = review_kit_application::use_cases::publishing::check_review(
        &req,
        &host.snapshot,
        &mut source,
        &mut renderer,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| has_plan_error(error, &|error| matches!(error, PlanError::Coverage(_))))
    );
    assert!(publish(&req, &mut host, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
}

#[test]
fn eligible_blocked_transition_uses_normal_reply_resolution_and_manual_change_rules() {
    let (mut host, mut source, mut renderer) = fixtures();
    let open = blocked_request(FindingStatus::Open);
    publish(&open, &mut host, &mut source, &mut renderer).unwrap();
    host.writes.clear();
    let fixed = blocked_request(FindingStatus::Fixed);
    let result = publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(result.round, 2);
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
    assert!(
        host.snapshot
            .findings
            .iter()
            .find(|root| root.author == reviewer("agent"))
            .unwrap()
            .resolved
    );
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.author == reviewer("agent"))
        .unwrap()
        .resolved = false;
    host.writes.clear();
    assert_eq!(
        publish(&fixed, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Unchanged
    );
    assert!(host.writes.is_empty());
}

#[test]
fn blocked_assessment_answers_an_owned_validated_finding_under_a_completed_policy() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = blocked_request(FindingStatus::Open);
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    foreign_question(&mut host);
    req.answer_targets.push(question_record());
    req.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: provider_id("question"),
        body: "The error branch discards the request before retry.".into(),
    });
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(result.round, 2);
    assert_eq!(
        effects(&host),
        vec![Effect::UpdateSummary, Effect::Reply, Effect::UpdateSummary]
    );
}

#[test]
fn unknown_absent_finding_keeps_the_original_grant_and_one_retry_budget() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = blocked_request(FindingStatus::Open);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    let authorization = host.progress.authorization.clone();
    let unresolved = host.progress.unresolved.clone();
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert_eq!(host.progress.retried_creations.len(), 1);
    assert_eq!(host.progress.unresolved, unresolved);
    assert_eq!(host.progress.authorization, authorization);
    host.writes.clear();
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert!(host.writes.is_empty());
    observe_pending_create(&mut host);
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Blocked
    );
    assert_eq!(host.progress.retried_creations.len(), 1);
    assert_eq!(
        host.snapshot.summaries[0].current.policy_declarations,
        req.policy_declarations
    );
}

#[test]
fn terminal_blocked_round_refuses_missing_or_edited_published_roots() {
    for remove in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        let req = blocked_request(FindingStatus::Open);
        publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        host.writes.clear();
        if remove {
            host.snapshot
                .findings
                .retain(|root| root.author != reviewer("agent"));
        } else {
            host.snapshot
                .findings
                .iter_mut()
                .find(|root| root.author == reviewer("agent"))
                .unwrap()
                .body = "edited root".into();
        }
        assert!(publish(&req, &mut host, &mut source, &mut renderer).is_err());
        assert!(host.writes.is_empty());
    }
}
