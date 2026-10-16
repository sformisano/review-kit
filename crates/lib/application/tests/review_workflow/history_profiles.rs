use super::*;

fn profile(fingerprint: &str) -> RenderProfileIdentity {
    RenderProfileIdentity {
        id: "team-review".into(),
        input_contract: "review-kit-render-input/v1".into(),
        fingerprint: fingerprint.into(),
    }
}

#[test]
fn finding_history_retains_open_and_fixed_rows_after_recurrence_and_restart() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let original = host.snapshot.summaries[0]
        .current
        .communicated_findings
        .clone()
        .unwrap();
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let fixed = host.snapshot.summaries[0]
        .current
        .communicated_findings
        .clone()
        .unwrap();
    publish(
        &request("recurrence", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let (mut restarted, mut new_source, mut new_renderer) = fixtures();
    restarted.snapshot = host.snapshot.clone();
    let plan = plan_from_snapshot(
        &request("recurrence", Some(FindingStatus::Open), false),
        &restarted.snapshot,
        &mut new_source,
        &mut new_renderer,
    )
    .unwrap();
    assert!(plan.unchanged);
    assert_eq!(
        plan.history[0].communicated_findings.as_ref().unwrap(),
        &original
    );
    assert_eq!(original[0].finding.status(), FindingStatus::Open);
    assert_eq!(original[0].actions, vec![PublishedAction::Created]);
    assert_eq!(
        plan.history[1].communicated_findings.as_ref().unwrap(),
        &fixed
    );
    assert_eq!(fixed[0].finding.status(), FindingStatus::Fixed);
    assert!(fixed[0].actions.contains(&PublishedAction::Resolved));
    let current = plan.round.communicated_findings.unwrap();
    assert_eq!(current[0].finding.status(), FindingStatus::Open);
    assert_eq!(current[0].root_note, original[0].root_note);
    assert_eq!(current[0].thread, original[0].thread);
    assert!(current[0].actions.contains(&PublishedAction::Reopened));
}

#[test]
fn agent_coverage_retains_removed_policy_names_in_history() {
    let (mut host, mut source, mut renderer) = fixtures();
    let unused = PolicyId::try_from("ownership".to_owned()).unwrap();
    let set = AssessmentPolicies::new(
        "rust-codebase-policies".into(),
        vec![PolicyName {
            id: policy(),
            name: "Rust Error Contracts".into(),
        }],
    )
    .unwrap();
    let mut first = request("initial", Some(FindingStatus::Open), false);
    first.review_workflow = Some("codebase-policies-review".into());
    first.policies = set.clone();
    first.policy_declarations[0].policy = set.policies()[0].clone();
    publish(&first, &mut host, &mut source, &mut renderer).unwrap();
    let projection = renderer.summaries.last().unwrap();
    assert!(
        projection
            .policies
            .iter()
            .all(|row| row.policy_id != unused)
    );
    let mut fixed = request("fixed", Some(FindingStatus::Fixed), false);
    fixed.review_workflow = first.review_workflow.clone();
    fixed.policies = first.policies.clone();
    fixed.policy_declarations = first.policy_declarations.clone();
    publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    let remaining = AssessmentPolicies::new(
        "rust-codebase-policies".into(),
        vec![PolicyName {
            id: unused.clone(),
            name: "Rust Ownership Design".into(),
        }],
    )
    .unwrap();
    let old = fixed.assessment;
    fixed.assessment = Assessment::new(AssessmentDraft {
        subject: old.subject().clone(),
        reviewer: old.reviewer().clone(),
        revision: old.revision().clone(),
        decision: ReviewDecision::Ready,
        scope: "Selected ownership checks".into(),
        depth: old.depth().into(),
        limitation: None,
        selected_policies: vec![unused.clone()],
        evaluations: vec![PolicyEvaluation::new(unused, PolicyJudgment::Satisfied).unwrap()],
        findings: old.findings().to_vec(),
    })
    .unwrap();
    fixed.policies = remaining.clone();
    fixed.policy_declarations = vec![PublicPolicyDeclaration {
        policy: remaining.policies()[0].clone(),
        status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
        reason: "This review selects ownership checks.".into(),
    }];
    fixed.transitions.clear();
    publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    let projection = renderer.summaries.last().unwrap();
    let retained = projection
        .policies
        .iter()
        .find(|row| row.policy_id == policy())
        .unwrap();
    assert_eq!(retained.name.as_deref(), Some("Rust Error Contracts"));
    assert_eq!(retained.applicable_rounds, vec![1, 2]);
    assert_eq!(retained.comments, 1);
    assert_eq!(
        projection.round.review_workflow.as_deref(),
        Some("codebase-policies-review")
    );
    assert_eq!(
        projection.round.policy_set.as_deref(),
        Some("rust-codebase-policies")
    );
}

#[test]
fn completed_presentation_change_keeps_round_and_performs_zero_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.identity = Some(profile("original"));
    let request = request("initial", Some(FindingStatus::Open), false);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    renderer.identity = Some(profile("changed"));
    host.writes.clear();
    let original_body = host.snapshot.summaries[0].body.clone();
    let previous_renders = renderer.summaries.len();
    let preview = plan_from_snapshot(&request, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert!(preview.unchanged);
    assert_eq!(preview.round.number, 1);
    assert!(renderer.summaries.len() > previous_renders);
    assert!(original_body.contains("profile:original"));
    assert!(preview.summary_body.contains("profile:changed"));
    assert_eq!(preview.summary_body, preview.terminal_summary_preview);
    assert_eq!(host.snapshot.summaries[0].body, original_body);
    let result = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert_eq!(result.round, 1);
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.renderer,
        Some(profile("original"))
    );
}

#[test]
fn changed_partial_profile_preserves_pending_assessment_and_exact_bodies() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.identity = Some(profile("original"));
    host.fault = Some(Fault::Rejected(Effect::Finding));
    let request = request("initial", Some(FindingStatus::Open), false);
    let result = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    let retained = host.snapshot.clone();
    renderer.identity = Some(profile("changed"));
    host.writes.clear();
    assert!(matches!(
        publish(&request, &mut host, &mut source, &mut renderer),
        Err(PlanError::ChangedRenderProfile)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.snapshot, retained);
    renderer.identity = Some(profile("original"));
    assert_eq!(
        publish(&request, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
}

#[test]
fn missing_legacy_pending_profile_identity_stops_recovery_before_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::Rejected(Effect::Finding));
    let request = request("initial", Some(FindingStatus::Open), false);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    let retained = host.snapshot.clone();
    renderer.identity = Some(profile("selected"));
    host.writes.clear();
    assert!(matches!(
        publish(&request, &mut host, &mut source, &mut renderer),
        Err(PlanError::UnknownPendingRenderProfile)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.snapshot, retained);
}

#[test]
fn changed_profile_preserves_unknown_initial_summary_intent_without_provider_state() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.identity = Some(profile("original"));
    host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
    let request = request("initial", Some(FindingStatus::Open), false);
    assert_eq!(
        publish(&request, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert!(host.snapshot.summaries.is_empty());
    assert_eq!(
        host.progress.unresolved.as_ref().unwrap().renderer,
        Some(profile("original"))
    );
    let progress = host.progress.clone();
    renderer.identity = Some(profile("changed"));
    host.writes.clear();
    assert!(matches!(
        publish(&request, &mut host, &mut source, &mut renderer),
        Err(PlanError::ChangedRenderProfile)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.progress, progress);
}

#[test]
fn missing_legacy_history_stays_unavailable_after_a_new_verified_round() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot.summaries[0].current.communicated_findings = None;
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert!(
        host.snapshot.summaries[0].history[0]
            .communicated_findings
            .is_none()
    );
    assert_eq!(
        host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()[0]
            .finding
            .status(),
        FindingStatus::Fixed
    );
}

#[test]
fn fabricated_historical_finding_state_stops_publication() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot.summaries[0]
        .current
        .communicated_findings
        .as_mut()
        .unwrap()[0]
        .finding = finding(FindingStatus::Fixed);
    host.writes.clear();
    assert!(matches!(
        publish(
            &request("initial", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer
        ),
        Err(PlanError::Discovery(_))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn partial_fix_retains_verified_transition_without_claiming_resolution() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let request = request("fixed", Some(FindingStatus::Fixed), false);
    assert_eq!(
        publish(&request, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    let partial = host.snapshot.summaries[0]
        .current
        .communicated_findings
        .as_ref()
        .unwrap();
    assert_eq!(partial[0].finding.status(), FindingStatus::Fixed);
    assert_eq!(
        partial[0].actions,
        vec![PublishedAction::Transition {
            from: FindingStatus::Open,
            to: FindingStatus::Fixed
        }]
    );
    assert!(
        !host
            .snapshot
            .findings
            .iter()
            .find(|root| root.author == reviewer("agent"))
            .unwrap()
            .resolved
    );
    let (mut restarted, mut new_source, mut new_renderer) = fixtures();
    restarted.snapshot = host.snapshot.clone();
    restarted.progress = host.progress.clone();
    assert_eq!(
        publish(&request, &mut restarted, &mut new_source, &mut new_renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    let summary = &restarted.snapshot.summaries[0];
    assert_eq!(summary.current.number, 2);
    assert!(
        summary.current.communicated_findings.as_ref().unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
    assert_eq!(
        summary.history[0].communicated_findings.as_ref().unwrap()[0]
            .finding
            .status(),
        FindingStatus::Open
    );
}

#[test]
fn abandonment_preserves_partial_communicated_state_and_cancels_missing_effects() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let expected = host.snapshot.summaries[0]
        .current
        .communicated_findings
        .clone();
    let abandon = review_kit_application::use_cases::publishing::AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: 2,
        reason: "Close this interrupted attempt".into(),
        effects: Effects::review(),
    };
    assert_eq!(
        abandon_observed(&abandon, &mut host, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Abandoned
    );
    assert_eq!(
        host.snapshot.summaries[0].current.communicated_findings,
        expected
    );
    host.writes.clear();
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert!(!effects(&host).contains(&Effect::Resolve));
    let history = &host.snapshot.summaries[0].history;
    assert_eq!(history[1].status, RoundStatus::Abandoned);
    assert_eq!(history[1].communicated_findings, expected);
}

#[test]
fn advancing_an_active_round_captures_verified_creates_after_the_last_summary_write() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.authority = Effects::new([Effect::CreateSummary, Effect::Finding]);
    let original = request("initial", Some(FindingStatus::Open), false);
    assert_eq!(
        publish(&original, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    assert!(
        host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()
            .is_empty()
    );
    host.authority = Effects::review();
    let next_revision =
        Revision::new("next-head".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = next_revision.clone();
    source.expected_revision = next_revision.clone();
    let previous = original.assessment;
    let followup = PublishRequest {
        confirmed_validation: false,
        assessment: Assessment::new(AssessmentDraft {
            subject: previous.subject().clone(),
            reviewer: previous.reviewer().clone(),
            revision: next_revision,
            decision: previous.decision(),
            depth: previous.depth().into(),
            scope: previous.scope().into(),
            limitation: None,
            selected_policies: previous.selected_policies().to_vec(),
            evaluations: previous.evaluations().to_vec(),
            findings: previous.findings().to_vec(),
        })
        .unwrap(),
        ..original
    };
    publish(&followup, &mut host, &mut source, &mut renderer).unwrap();
    let historical = host.snapshot.summaries[0].history[0]
        .communicated_findings
        .as_ref()
        .unwrap();
    assert_eq!(historical.len(), 1);
    assert_eq!(historical[0].actions, vec![PublishedAction::Created]);
    assert_eq!(historical[0].finding.status(), FindingStatus::Open);
}

#[test]
fn a_policy_rename_starts_and_retains_a_successor_round() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.identity = Some(profile("unchanged-profile"));
    let mut request = request("initial", Some(FindingStatus::Open), false);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    let retained = host.snapshot.clone();
    let set = AssessmentPolicies::new(
        "test-policies".into(),
        vec![PolicyName {
            id: policy(),
            name: "Updated Policy Name".into(),
        }],
    )
    .unwrap();
    request.policies = set.clone();
    request.policy_declarations[0].policy = set.policies()[0].clone();
    let preview = plan_from_snapshot(&request, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert!(!preview.unchanged);
    assert_eq!(preview.round.number, 2);
    let row = renderer
        .summaries
        .last()
        .unwrap()
        .policies
        .iter()
        .find(|row| row.policy_id == policy())
        .unwrap();
    assert_eq!(row.name.as_deref(), Some("Updated Policy Name"));
    assert_eq!(row.applicable_rounds, vec![1, 2]);
    assert_eq!(row.comments, 1);
    host.writes.clear();
    assert_eq!(
        publish(&request, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    assert!(
        effects(&host)
            .iter()
            .all(|effect| *effect == Effect::UpdateSummary)
    );
    assert_eq!(
        host.snapshot.summaries[0].history[0],
        retained.summaries[0].current
    );
    assert_eq!(
        host.snapshot.summaries[0].current.policy_declarations,
        request.policy_declarations
    );
}

#[test]
fn confirmed_resolution_survives_terminal_render_failure_and_fresh_recovery() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    renderer.fail_after_resolution = true;
    let request = request("fixed", Some(FindingStatus::Fixed), false);
    let interrupted = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert!(matches!(
        interrupted.finalization_stop,
        Some(Stop::Render(_))
    ));
    assert_eq!(host.progress.verified_resolutions.len(), 1);
    let receipt = host.progress.verified_resolutions[0].clone();
    assert_eq!(receipt.round, 2);
    assert!(receipt.resolved);
    assert!(host.progress.unresolved.is_none());
    assert!(
        !host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
    let (mut restarted, mut new_source, mut new_renderer) = fixtures();
    restarted.snapshot = host.snapshot.clone();
    restarted.progress = host.progress.clone();
    assert_eq!(
        publish(&request, &mut restarted, &mut new_source, &mut new_renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    assert_eq!(effects(&restarted), vec![Effect::UpdateSummary]);
    let current = &restarted.snapshot.summaries[0].current;
    assert_eq!(current.verified_resolutions, vec![receipt]);
    assert!(
        current.communicated_findings.as_ref().unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
}

#[test]
fn matching_manual_resolution_does_not_supply_a_tool_resolution_receipt() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let request = request("fixed", Some(FindingStatus::Fixed), false);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.author == reviewer("agent"))
        .unwrap()
        .resolved = true;
    host.writes.clear();
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert!(!effects(&host).contains(&Effect::Resolve));
    assert!(host.progress.verified_resolutions.is_empty());
    assert!(
        host.snapshot.summaries[0]
            .current
            .verified_resolutions
            .is_empty()
    );
    assert!(
        !host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
}

#[test]
fn partial_snapshot_cannot_promote_uncommunicated_assessment_intent() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Reply));
    let request = request("fixed", Some(FindingStatus::Fixed), false);
    publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    let row = &mut host.snapshot.summaries[0]
        .current
        .communicated_findings
        .as_mut()
        .unwrap()[0];
    assert_eq!(row.finding.status(), FindingStatus::Open);
    assert!(row.actions.is_empty());
    row.finding = finding(FindingStatus::Fixed);
    host.writes.clear();
    assert!(matches!(
        publish(&request, &mut host, &mut source, &mut renderer),
        Err(PlanError::Discovery(_))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn confirmed_resolution_is_retained_when_the_next_invocation_reviews_new_commits() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    renderer.fail_after_resolution = true;
    let fixed = request("fixed", Some(FindingStatus::Fixed), false);
    publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    let receipt = host.progress.verified_resolutions[0].clone();
    let (mut restarted, mut new_source, mut new_renderer) = fixtures();
    restarted.snapshot = host.snapshot.clone();
    restarted.progress = host.progress.clone();
    let next_revision =
        Revision::new("new-commits".into(), vec!["base".into(), "start".into()]).unwrap();
    restarted.snapshot.revision = next_revision.clone();
    new_source.expected_revision = next_revision.clone();
    let previous = fixed.assessment;
    let followup = PublishRequest {
        confirmed_validation: false,
        assessment: Assessment::new(AssessmentDraft {
            subject: previous.subject().clone(),
            reviewer: previous.reviewer().clone(),
            revision: next_revision,
            decision: previous.decision(),
            depth: previous.depth().into(),
            scope: previous.scope().into(),
            limitation: None,
            selected_policies: previous.selected_policies().to_vec(),
            evaluations: previous.evaluations().to_vec(),
            findings: previous.findings().to_vec(),
        })
        .unwrap(),
        transitions: vec![],
        ..fixed
    };
    let preview = review_kit_application::use_cases::publishing::plan_from_snapshot_with_progress(
        &followup,
        &restarted.snapshot,
        &mut new_source,
        &mut new_renderer,
        &restarted.progress,
    )
    .unwrap();
    assert_eq!(preview.round.number, 3);
    assert_eq!(
        preview.history[1].verified_resolutions,
        vec![receipt.clone()]
    );
    assert!(
        preview.history[1].communicated_findings.as_ref().unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
    assert_eq!(
        publish(
            &followup,
            &mut restarted,
            &mut new_source,
            &mut new_renderer
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let retained = &restarted.snapshot.summaries[0].history[1];
    assert_eq!(retained.verified_resolutions, vec![receipt]);
    assert!(
        retained.communicated_findings.as_ref().unwrap()[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
    assert!(!effects(&restarted).contains(&Effect::Resolve));
    assert!(!effects(&restarted).contains(&Effect::Reply));
}

#[test]
fn completed_public_identity_changes_require_a_changed_assessment() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut original = request("initial", Some(FindingStatus::Open), false);
    original.review_workflow = Some("original-public-workflow".into());
    publish(&original, &mut host, &mut source, &mut renderer).unwrap();
    let retained = host.snapshot.clone();
    host.writes.clear();
    let mut workflow = original.clone();
    workflow.review_workflow = Some("different-public-workflow".into());
    assert!(matches!(
        publish(&workflow, &mut host, &mut source, &mut renderer),
        Err(PlanError::ChangedReviewIdentity)
    ));
    let previous_set = &original.policies;
    let different_set = AssessmentPolicies::new(
        "different-public-policy-set".into(),
        previous_set.policies().to_vec(),
    )
    .unwrap();
    let mut selection = original.clone();
    selection.policies = different_set.clone();
    assert_eq!(selection.policies.ids(), original.policies.ids());
    assert!(matches!(
        publish(&selection, &mut host, &mut source, &mut renderer),
        Err(PlanError::ChangedReviewIdentity)
    ));
    assert!(host.writes.is_empty());
    assert_eq!(host.snapshot, retained);
}

#[test]
fn legacy_completed_public_identities_remain_unknown() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot.summaries[0].current.review_workflow = None;
    host.snapshot.summaries[0].current.policy_set = None;
    let mut request = request("initial", Some(FindingStatus::Open), false);
    request.review_workflow = Some("current-caller-public-workflow".into());
    let preview = plan_from_snapshot(&request, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert!(preview.unchanged);
    assert!(preview.round.review_workflow.is_none());
    assert!(preview.round.policy_set.is_none());
    host.writes.clear();
    assert_eq!(
        publish(&request, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Unchanged
    );
    assert!(host.writes.is_empty());
    assert!(host.snapshot.summaries[0].current.review_workflow.is_none());
    assert!(host.snapshot.summaries[0].current.policy_set.is_none());
}

fn on_revision(
    mut request: PublishRequest,
    revision: Revision,
    findings: Vec<Finding>,
) -> PublishRequest {
    let previous = &request.assessment;
    request.assessment = Assessment::new(AssessmentDraft {
        subject: previous.subject().clone(),
        reviewer: previous.reviewer().clone(),
        revision,
        decision: previous.decision(),
        depth: previous.depth().into(),
        scope: previous.scope().into(),
        limitation: previous.limitation().map(str::to_owned),
        selected_policies: previous.selected_policies().to_vec(),
        evaluations: previous.evaluations().to_vec(),
        findings,
    })
    .unwrap();
    request
}

#[test]
fn authored_answers_are_not_duplicated_by_round_sensitive_templates() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.answer_context_uses_round = true;
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let answer = answer_request("first-answer");
    publish(&answer, &mut host, &mut source, &mut renderer).unwrap();
    let original_reply = host
        .snapshot
        .replies
        .iter()
        .find(|reply| {
            reply
                .action
                .as_ref()
                .is_some_and(|action| matches!(action.kind, ReplyKind::Answer(_)))
        })
        .unwrap()
        .clone();
    assert!(original_reply.body.contains("Answer R2 @head:"));
    let next = Revision::new(
        "next-answer-head".into(),
        vec!["base".into(), "start".into()],
    )
    .unwrap();
    host.snapshot.revision = next.clone();
    source.expected_revision = next.clone();
    let repeated = on_revision(answer.clone(), next, answer.assessment.findings().to_vec());
    host.writes.clear();
    let result = publish(&repeated, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, 3);
    assert!(!effects(&host).contains(&Effect::Reply));
    let answered: Vec<_> = host
        .snapshot
        .replies
        .iter()
        .filter(|reply| {
            reply
                .action
                .as_ref()
                .is_some_and(|action| matches!(action.kind, ReplyKind::Answer(_)))
        })
        .collect();
    assert_eq!(answered, vec![&original_reply]);
    let mut useful = repeated;
    useful.answers[0].body = "A different authored response.".into();
    host.writes.clear();
    publish(&useful, &mut host, &mut source, &mut renderer).unwrap();
    assert!(effects(&host).contains(&Effect::Reply));
}

#[test]
fn completed_rounds_do_not_promote_uncommunicated_finding_prose() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let original = finding(FindingStatus::Open);
    let intended = Finding::new(FindingDraft {
        id: original.id(),
        policy_id: original.policy_id().clone(),
        title: "Uncommunicated title".into(),
        body: "Uncommunicated explanation".into(),
        evidence: "Uncommunicated evidence".into(),
        severity: original.severity(),
        disposition: original.disposition(),
        status: original.status(),
        validated: original.validated(),
        location: original.location().clone(),
        suggestion: original.suggestion().cloned(),
    })
    .unwrap();
    let next = Revision::new("prose-head-2".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = next.clone();
    source.expected_revision = next.clone();
    let second = on_revision(
        request("followup", Some(FindingStatus::Open), false),
        next,
        vec![intended.clone()],
    );
    publish(&second, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(
        host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()[0]
            .finding,
        original
    );
    let prepared = review_kit_application::use_cases::assessment::prepare(
        host.snapshot.clone(),
        reviewer("agent"),
        second.policies.clone(),
    )
    .unwrap();
    assert_eq!(prepared.findings[0].previous, original);
    let later = Revision::new("prose-head-3".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = later.clone();
    source.expected_revision = later.clone();
    let third = on_revision(second, later, vec![intended.clone()]);
    publish(&third, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(
        host.snapshot.summaries[0]
            .current
            .communicated_findings
            .as_ref()
            .unwrap()[0]
            .finding,
        original
    );
    assert_eq!(
        renderer.summaries.last().unwrap().findings[0].finding,
        original
    );
    host.snapshot.summaries[0].history[1]
        .communicated_findings
        .as_mut()
        .unwrap()[0]
        .finding = intended;
    host.writes.clear();
    assert!(matches!(
        publish(&third, &mut host, &mut source, &mut renderer),
        Err(PlanError::Discovery(_))
    ));
    assert!(host.writes.is_empty());
}
