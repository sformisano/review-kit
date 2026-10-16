use super::*;
use review_kit_application::use_cases::discovery_bundle::{
    ObservedChanges, RecoveryConflict, ReviewChanges, discover_bundle,
};
use review_kit_application::use_cases::publishing::check_review;

struct NoChanges;
impl ReviewChanges for NoChanges {
    fn read_changes(
        &mut self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<ObservedChanges, PortError> {
        Ok(ObservedChanges {
            subject: subject.clone(),
            revision: revision.clone(),
            files: vec![],
            unified_diff: String::new(),
        })
    }
}

fn transition_replies(host: &Host) -> usize {
    host.snapshot
        .replies
        .iter()
        .filter(|r| {
            matches!(
                r.action.as_ref().map(|a| &a.kind),
                Some(ReplyKind::Transition(_))
            )
        })
        .count()
}

fn with_scope(request: &mut PublishRequest, scope: &str) {
    let previous = &request.assessment;
    request.assessment = Assessment::new(AssessmentDraft {
        subject: previous.subject().clone(),
        reviewer: previous.reviewer().clone(),
        revision: previous.revision().clone(),
        decision: previous.decision(),
        depth: previous.depth().into(),
        scope: scope.into(),
        limitation: previous.limitation().map(str::to_owned),
        selected_policies: previous.selected_policies().to_vec(),
        evaluations: previous.evaluations().to_vec(),
        findings: previous.findings().to_vec(),
    })
    .unwrap();
}

#[test]
fn completed_assessment_cannot_override_verified_root() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut forged = request("fixed", Some(FindingStatus::Fixed), false);
    forged.transitions.clear();
    host.snapshot.summaries[0].current.assessment = forged.assessment.clone();
    host.writes.clear();

    assert!(
        discover_bundle(
            &mut Session(&mut host),
            &mut NoChanges,
            &subject(),
            &reviewer("agent")
        )
        .is_err()
    );
    let summary = &host.snapshot.summaries[0];
    assert!(
        review_kit_application::summary::summary_view(
            &summary.current,
            &summary.history,
            &host.snapshot
        )
        .is_err()
    );
    assert!(publish(&forged, &mut host, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
}

#[test]
fn completed_carried_assessment_requires_verified_transition() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut carried = request("initial", Some(FindingStatus::Open), false);
    with_scope(&mut carried, "Request handling and retry paths");
    assert_eq!(
        publish(&carried, &mut host, &mut source, &mut renderer)
            .unwrap()
            .round,
        2
    );
    assert!(host.snapshot.summaries[0].current.actions.is_empty());
    let mut forged = request("fixed", Some(FindingStatus::Fixed), false);
    with_scope(&mut forged, "Request handling and retry paths");
    forged.transitions.clear();
    host.snapshot.summaries[0].current.assessment = forged.assessment.clone();
    host.writes.clear();

    assert!(
        discover_bundle(
            &mut Session(&mut host),
            &mut NoChanges,
            &subject(),
            &reviewer("agent")
        )
        .is_err()
    );
    assert!(publish(&forged, &mut host, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
}

#[test]
fn completed_transition_is_not_reposted_for_changed_scope() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let fixed = request("fixed", Some(FindingStatus::Fixed), false);
    let r2 = publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(r2.status, PublicationStatus::Completed);
    assert_eq!(transition_replies(&host), 1);
    let mut again = fixed.clone();
    with_scope(&mut again, "Request handling and retry paths");
    host.writes.clear();
    let r3 = publish(&again, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(r3.status, PublicationStatus::Completed);
    assert_eq!(r3.round, r2.round + 1);
    assert_eq!(
        transition_replies(&host),
        1,
        "transition reply was duplicated"
    );
}

#[test]
fn completed_reclassification_is_not_reposted_on_new_revision() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut reclass = request("initial", Some(FindingStatus::Open), false);
    let updated = Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: policy(),
        title: "Lost request".into(),
        body: "The failure path drops the request.".into(),
        severity: Severity::P1,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Regression case".into(),
        validated: true,
        location: finding(FindingStatus::Open).location().clone(),
        suggestion: None,
    })
    .unwrap();
    replace_findings(&mut reclass, vec![updated]);
    reclass.transitions.push(TransitionReply {
        finding: FindingId::try_from(1).unwrap(),
        from: FindingStatus::Open,
        to: FindingStatus::Open,
        body: "Higher severity follows from the impact evidence.".into(),
    });
    let r2 = publish(&reclass, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(r2.status, PublicationStatus::Completed);
    assert_eq!(transition_replies(&host), 1);
    let new_rev = Revision::new("head2".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = new_rev.clone();
    let mut fresh = reclass.clone();
    replace_revision(&mut fresh, new_rev);
    host.writes.clear();
    let r3 = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(r3.status, PublicationStatus::Completed);
    assert_eq!(r3.round, r2.round + 1);
    assert_eq!(
        transition_replies(&host),
        1,
        "reclassification reply was duplicated"
    );
}

#[test]
fn refreshed_suggestion_revision_rereads_source_without_transition() {
    let (mut host, mut source, mut renderer) = fixtures();
    let base = finding(FindingStatus::Open);
    let make = |rev: Revision| {
        Suggestion::new(
            base.location().clone(),
            rev,
            LineNumber::try_from(1).unwrap(),
            vec!["committed line".into()],
            vec!["replacement".into()],
            "evidence".into(),
        )
        .unwrap()
    };
    let mut req = request("initial", Some(FindingStatus::Open), false);
    replace_findings(
        &mut req,
        vec![copy_finding(
            &base,
            base.id(),
            policy(),
            Some(make(revision())),
        )],
    );
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    let new_rev = Revision::new("head2".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = new_rev.clone();
    let mut fresh = request("fresh", Some(FindingStatus::Open), false);
    replace_revision(&mut fresh, new_rev.clone());
    replace_findings(
        &mut fresh,
        vec![copy_finding(
            &base,
            base.id(),
            policy(),
            Some(make(new_rev.clone())),
        )],
    );
    host.writes.clear();
    source.expected_revision = new_rev;
    source.log.borrow_mut().clear();
    let outcome = publish(&fresh, &mut host, &mut source, &mut renderer);
    assert_eq!(outcome.unwrap().status, PublicationStatus::Completed);
    assert!(source.log.borrow().iter().any(|event| event == "source"));
    assert!(source.log.borrow().iter().any(|event| event == "position"));
    assert_eq!(transition_replies(&host), 0);
}

#[test]
fn edited_pending_question_remains_discoverable_on_new_revision() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
    let failed = publish(
        &answer_request("answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(failed.status, PublicationStatus::Uncertain);
    host.snapshot
        .replies
        .iter_mut()
        .find(|r| r.id == provider_id("question"))
        .unwrap()
        .body = "Edited question".into();
    let new_rev = Revision::new("head2".into(), vec!["base".into(), "start".into()]).unwrap();
    host.snapshot.revision = new_rev.clone();
    let discovered = discover_bundle(
        &mut Session(&mut host),
        &mut NoChanges,
        &subject(),
        &reviewer("agent"),
    );
    let mut fresh = request("fresh", Some(FindingStatus::Open), false);
    replace_revision(&mut fresh, new_rev);
    let planned = plan(&fresh, &mut Session(&mut host), &mut source, &mut renderer);
    let checked = check_review(&fresh, &host.snapshot, &mut source, &mut renderer);
    assert!(discovered.unwrap().recovery_conflicts().is_empty());
    assert!(planned.is_ok());
    assert!(checked.is_ok());
}

#[test]
fn deleted_pending_question_remains_discoverable() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
    publish(
        &answer_request("answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    host.snapshot
        .replies
        .retain(|r| r.id != provider_id("question"));
    let discovered = discover_bundle(
        &mut Session(&mut host),
        &mut NoChanges,
        &subject(),
        &reviewer("agent"),
    );
    assert_eq!(
        discovered.unwrap().recovery_conflicts(),
        &[RecoveryConflict::ChangedAnswerTarget {
            finding: FindingId::try_from(1).unwrap(),
            target: provider_id("question")
        }]
    );
}

#[test]
fn manually_resolved_created_root_does_not_block_recovery() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::Rejected(Effect::UpdateSummary));
    let req = request("create-only", Some(FindingStatus::Open), false);
    let partial = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    host.snapshot
        .findings
        .iter_mut()
        .find(|r| r.author == reviewer("agent"))
        .unwrap()
        .resolved = true;
    host.authority = Effects::review();
    host.writes.clear();
    let rerun = publish(&req, &mut host, &mut source, &mut renderer);
    let planned = plan(&req, &mut Session(&mut host), &mut source, &mut renderer);
    let discovered = discover_bundle(
        &mut Session(&mut host),
        &mut NoChanges,
        &subject(),
        &reviewer("agent"),
    );
    assert_eq!(rerun.unwrap().status, PublicationStatus::Completed);
    assert!(planned.is_ok());
    assert!(discovered.is_ok());
    assert!(!effects(&host).contains(&Effect::Finding));
    assert!(
        host.snapshot
            .findings
            .iter()
            .find(|r| r.id == provider_id("owned-root"))
            .unwrap()
            .resolved
    );
}

fn two_findings() -> PublishRequest {
    let mut value = request("two", Some(FindingStatus::Open), false);
    replace_findings(&mut value, vec![numbered_finding(1), numbered_finding(2)]);
    value.allow_colocated = true;
    value
}

#[test]
fn two_creates_have_distinct_provider_identities() {
    let (mut host, mut source, mut renderer) = fixtures();
    let result = publish(&two_findings(), &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let owned: Vec<_> = host
        .snapshot
        .findings
        .iter()
        .filter(|root| root.author == reviewer("agent"))
        .collect();
    assert_eq!(owned.len(), 2);
    assert_ne!(owned[0].id, owned[1].id);
    assert_ne!(owned[0].root_note, owned[1].root_note);
    assert_eq!(owned[0].finding.id(), FindingId::try_from(1).unwrap());
    assert_eq!(owned[1].finding.id(), FindingId::try_from(2).unwrap());
}

#[test]
fn second_create_failure_resumes_without_recreating_first() {
    let (mut host, mut source, mut renderer) = fixtures();
    let value = two_findings();
    host.reject_finding = Some(FindingId::try_from(2).unwrap());
    let failed = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(failed.status, PublicationStatus::Partial);
    let attempted_creates: Vec<_> = failed
        .attempted
        .iter()
        .filter_map(|write| match write {
            Write::Finding { finding, .. } => Some(finding.id()),
            _ => None,
        })
        .collect();
    assert_eq!(
        attempted_creates,
        vec![
            FindingId::try_from(1).unwrap(),
            FindingId::try_from(2).unwrap()
        ]
    );
    let verified_creates: Vec<_> = failed
        .verified
        .iter()
        .filter_map(|effect| match &effect.write {
            Write::Finding { finding, .. } => Some(finding.id()),
            _ => None,
        })
        .collect();
    assert_eq!(verified_creates, vec![FindingId::try_from(1).unwrap()]);
    let skipped_creates: Vec<_> = failed
        .skipped
        .iter()
        .filter_map(|action| match action {
            PlannedAction::Finding { finding, .. } => Some(finding.id()),
            _ => None,
        })
        .collect();
    assert_eq!(skipped_creates, vec![FindingId::try_from(2).unwrap()]);
    let first = host
        .snapshot
        .findings
        .iter()
        .find(|root| root.author == reviewer("agent"))
        .unwrap()
        .clone();
    host.writes.clear();
    let recovered = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(recovered.status, PublicationStatus::Completed);
    assert_eq!(recovered.round, failed.round);
    let creates: Vec<_> = host
        .writes
        .iter()
        .filter_map(|write| match write {
            Write::Finding { finding, .. } => Some(finding.id()),
            _ => None,
        })
        .collect();
    assert_eq!(creates, vec![FindingId::try_from(2).unwrap()]);
    assert_eq!(
        host.snapshot
            .findings
            .iter()
            .find(|root| root.id == first.id),
        Some(&first)
    );
    assert_eq!(
        host.snapshot
            .findings
            .iter()
            .filter(|root| root.author == reviewer("agent"))
            .count(),
        2
    );
}

#[test]
fn repeated_fixed_open_fixed_cycles_publish_each_real_transition() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    for (label, status) in [
        ("fixed", FindingStatus::Fixed),
        ("recurrence", FindingStatus::Open),
        ("fixed", FindingStatus::Fixed),
    ] {
        let result = publish(
            &request(label, Some(status), false),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
    }
    assert_eq!(transition_replies(&host), 3);
    assert!(
        host.snapshot
            .findings
            .iter()
            .find(|root| root.id == provider_id("owned-root"))
            .unwrap()
            .resolved
    );
}

#[test]
fn blocked_source_failure_is_rejected_by_check_and_publish() {
    let (mut host, mut source, mut renderer) = fixtures();
    let value = request("blocked", Some(FindingStatus::Open), true);
    source.fail = true;
    assert!(check_review(&value, &host.snapshot, &mut source, &mut renderer).is_err());
    let error = publish(&value, &mut host, &mut source, &mut renderer).unwrap_err();
    assert!(has_plan_error(&error, &|error| matches!(
        error,
        PlanError::Checks(_)
    )));
    assert!(host.writes.is_empty());
}

#[test]
fn same_state_severity_cycles_publish_each_reclassification() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    for severity in [Severity::P1, Severity::P2, Severity::P1] {
        let mut value = request("reclassify", Some(FindingStatus::Open), false);
        let base = finding(FindingStatus::Open);
        let updated = Finding::new(FindingDraft {
            id: base.id(),
            policy_id: policy(),
            title: base.title().into(),
            body: base.body().into(),
            severity,
            disposition: base.disposition(),
            status: base.status(),
            evidence: base.evidence().into(),
            validated: true,
            location: base.location().clone(),
            suggestion: None,
        })
        .unwrap();
        replace_findings(&mut value, vec![updated]);
        value.transitions.push(TransitionReply {
            finding: base.id(),
            from: FindingStatus::Open,
            to: FindingStatus::Open,
            body: "Impact evidence changes severity.".into(),
        });
        assert_eq!(
            publish(&value, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Completed
        );
    }
    assert_eq!(transition_replies(&host), 3);
}

#[test]
fn blocked_renderer_failure_is_rejected_by_check_and_publish() {
    let (mut host, mut source, mut renderer) = fixtures();
    let value = request("blocked", Some(FindingStatus::Open), true);
    renderer.fail = true;
    assert!(check_review(&value, &host.snapshot, &mut source, &mut renderer).is_err());
    let error = publish(&value, &mut host, &mut source, &mut renderer).unwrap_err();
    assert!(has_plan_error(&error, &|error| matches!(
        error,
        PlanError::Checks(_) | PlanError::Render(_)
    )));
    assert!(host.writes.is_empty());
}

fn abandon_request(host: &Host) -> review_kit_application::use_cases::publishing::AbandonRequest {
    review_kit_application::use_cases::publishing::AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: host.snapshot.summaries[0].current.number,
        reason: "The finding requires a corrected assessment.".into(),
        effects: Effects::review(),
    }
}

fn reject_first_create(host: &mut Host, source: &mut Source, renderer: &mut Renderer) {
    host.fault = Some(Fault::Rejected(Effect::Finding));
    let result = publish(
        &request("initial", Some(FindingStatus::Open), false),
        host,
        source,
        renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    host.writes.clear();
}

#[test]
fn abandonment_stops_for_an_unresolved_create() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let req = request("uncertain", Some(FindingStatus::Open), false);
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.writes.clear();
    let value = abandon_request(&host);
    let result = abandon_observed(&value, &mut host, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(result.stop, Some(Stop::UnresolvedWrite)));
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

#[test]
fn abandonment_closes_observed_intent_after_known_rejection() {
    let (mut host, mut source, mut renderer) = fixtures();
    reject_first_create(&mut host, &mut source, &mut renderer);
    let value = abandon_request(&host);
    let result = abandon_observed(&value, &mut host, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Abandoned);
    assert_eq!(result.skipped.len(), 1);
    assert!(host.progress.unresolved.is_none());
    assert_eq!(effects(&host), vec![Effect::UpdateSummary]);
}

#[test]
fn abandoned_unpublished_ids_are_reserved_and_corrected_assessment_can_start() {
    let (mut host, mut source, mut renderer) = fixtures();
    reject_first_create(&mut host, &mut source, &mut renderer);
    let unperformed = host.snapshot.summaries[0].current.actions.clone();
    let value = abandon_request(&host);
    let abandoned = abandon_observed(&value, &mut host, &mut renderer).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    assert_eq!(abandoned.skipped, unperformed);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Abandoned
    );
    let discovered = discover_bundle(
        &mut Session(&mut host),
        &mut NoChanges,
        &subject(),
        &reviewer("agent"),
    )
    .unwrap();
    assert_eq!(
        discovered.next_finding_id(),
        FindingId::try_from(2).unwrap()
    );
    let mut corrected = request("corrected", Some(FindingStatus::Open), false);
    replace_findings(&mut corrected, vec![numbered_finding(2)]);
    host.writes.clear();
    let result = publish(&corrected, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, abandoned.round + 1);
    assert!(
        host.snapshot
            .findings
            .iter()
            .any(|root| root.author == reviewer("agent")
                && root.finding.id() == FindingId::try_from(2).unwrap())
    );
}

#[test]
fn abandonment_preserves_verified_root_reply_and_manual_resolution() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let result = publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.id == provider_id("owned-root"))
        .unwrap()
        .resolved = true;
    let root = host
        .snapshot
        .findings
        .iter()
        .find(|root| root.id == provider_id("owned-root"))
        .unwrap()
        .clone();
    let replies = host.snapshot.replies.clone();
    host.writes.clear();
    let value = abandon_request(&host);
    let abandoned = abandon_observed(&value, &mut host, &mut renderer).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    assert!(abandoned.skipped.is_empty());
    assert_eq!(
        host.snapshot
            .findings
            .iter()
            .find(|current| current.id == root.id),
        Some(&root)
    );
    assert_eq!(host.snapshot.replies, replies);
    assert!(
        host.writes
            .iter()
            .all(|write| matches!(write, Write::Summary { .. }))
    );
    assert!(
        !host.snapshot.summaries[0]
            .current
            .abandonment
            .as_ref()
            .unwrap()
            .verified_actions
            .is_empty()
    );
}

#[test]
fn stale_completed_transition_does_not_resolve_manual_reopen() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut value = request("fixed", Some(FindingStatus::Fixed), false);
    assert_eq!(
        publish(&value, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    host.snapshot
        .findings
        .iter_mut()
        .find(|root| root.id == provider_id("owned-root"))
        .unwrap()
        .resolved = false;
    with_scope(&mut value, "Request handling and retry paths");
    host.writes.clear();
    assert_eq!(
        publish(&value, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    assert!(
        host.writes
            .iter()
            .all(|write| matches!(write, Write::Summary { .. }))
    );
    assert!(
        !host
            .snapshot
            .findings
            .iter()
            .find(|root| root.id == provider_id("owned-root"))
            .unwrap()
            .resolved
    );
}

#[test]
fn local_context_is_available_in_debug() {
    let mut value = request("initial", Some(FindingStatus::Open), false);
    value.local_context = "private-marker-8173".into();
    for debug in [format!("{value:?}"), format!("{value:#?}")] {
        assert!(debug.contains("private-marker-8173"));
        assert!(debug.contains("assessment"));
    }
    let (host, _, _) = fixtures();
    let mut template = review_kit_application::use_cases::assessment::prepare(
        host.snapshot,
        reviewer("agent"),
        selected(),
    )
    .unwrap();
    template.local_context = "private-marker-8173".into();
    for debug in [format!("{template:?}"), format!("{template:#?}")] {
        assert!(debug.contains("private-marker-8173"));
        assert!(debug.contains("snapshot"));
    }
}

#[test]
fn edited_pending_question_is_reported_as_recovery_conflict() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
    assert_eq!(
        publish(
            &answer_request("answer"),
            &mut host,
            &mut source,
            &mut renderer
        )
        .unwrap()
        .status,
        PublicationStatus::Uncertain
    );
    host.snapshot
        .replies
        .iter_mut()
        .find(|reply| reply.id == provider_id("question"))
        .unwrap()
        .body = "Edited question".into();
    let discovered = discover_bundle(
        &mut Session(&mut host),
        &mut NoChanges,
        &subject(),
        &reviewer("agent"),
    )
    .unwrap();
    assert_eq!(
        discovered.recovery_conflicts(),
        &[RecoveryConflict::ChangedAnswerTarget {
            finding: FindingId::try_from(1).unwrap(),
            target: provider_id("question")
        }]
    );
}

#[test]
fn abandoned_verified_effects_cannot_disappear_silently() {
    for delete_root in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        host.fault = Some(Fault::Rejected(Effect::Resolve));
        let fixed = request("fixed", Some(FindingStatus::Fixed), false);
        assert_eq!(
            publish(&fixed, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Partial
        );
        let value = abandon_request(&host);
        assert_eq!(
            abandon_observed(&value, &mut host, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Abandoned
        );
        if delete_root {
            host.snapshot
                .findings
                .retain(|root| root.id != provider_id("owned-root"));
        } else {
            host.snapshot.replies.clear();
        }
        let discovered = discover_bundle(
            &mut Session(&mut host),
            &mut NoChanges,
            &subject(),
            &reviewer("agent"),
        );
        assert!(discovered.is_err());
        host.writes.clear();
        let error = plan(&fixed, &mut Session(&mut host), &mut source, &mut renderer).unwrap_err();
        assert!(has_plan_error(&error, &|error| matches!(
            error,
            PlanError::MissingPublishedFinding(_) | PlanError::ConflictingReply(_)
        )));
        assert!(host.writes.is_empty());
    }
}

#[test]
fn abandonment_evidence_failure_prevents_summary_write() {
    use review_kit_application::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence};
    struct FailBeforeWrite;
    impl ExecutionEvidence for FailBeforeWrite {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if matches!(event, ExecutionEvent::BeforeWrite(_)) {
                Err(failure("evidence"))
            } else {
                Ok(())
            }
        }
    }
    let (mut host, mut source, mut renderer) = fixtures();
    reject_first_create(&mut host, &mut source, &mut renderer);
    let value = abandon_request(&host);
    let result = abandon_recorded(&value, &mut host, &mut renderer, &mut FailBeforeWrite).unwrap();
    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert!(matches!(result.stop, Some(Stop::Evidence(_))));
    assert!(result.attempted.is_empty());
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Partial
    );
}

#[test]
fn disappearing_observed_create_stops_abandonment() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.authority = Effects::new([Effect::CreateSummary, Effect::Finding]);
    let value = request("initial", Some(FindingStatus::Open), false);
    assert_eq!(
        publish(&value, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    assert!(
        host.snapshot
            .findings
            .iter()
            .any(|root| root.id == provider_id("owned-root"))
    );
    host.authority = Effects::review();
    host.delete_root_on_discovery = Some(host.discoveries + 2);
    host.writes.clear();
    let abandon = abandon_request(&host);
    let result = abandon_observed(&abandon, &mut host, &mut renderer);
    match result {
        Err(error) => assert!(has_plan_error(&error, &|error| matches!(
            error,
            PlanError::MissingPublishedFinding(_) | PlanError::ConflictingFinding(_)
        ))),
        Ok(report) => assert!(matches!(
            report.stop,
            Some(Stop::Conflict | Stop::ConcurrentChange)
        )),
    }
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

#[test]
fn recovered_pending_summary_does_not_turn_prewrite_failure_uncertain() {
    use review_kit_application::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence};
    struct FailBeforeWrite;
    impl ExecutionEvidence for FailBeforeWrite {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if matches!(event, ExecutionEvent::BeforeWrite(_)) {
                Err(failure("evidence"))
            } else {
                Ok(())
            }
        }
    }
    let (mut host, mut source, mut renderer) = fixtures();
    let value = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let failed = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(failed.status, PublicationStatus::Uncertain);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    observe_pending_create(&mut host);
    host.writes.clear();
    let recovered = publish_recorded(
        &value,
        &mut host,
        &mut source,
        &mut renderer,
        &mut FailBeforeWrite,
    )
    .unwrap();
    assert_eq!(recovered.status, PublicationStatus::NotStarted);
    assert!(matches!(recovered.stop, Some(Stop::Evidence(_))));
    assert_eq!(recovered.round, failed.round);
    assert!(recovered.attempted.is_empty());
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

fn check_rejected_initial_summary_with_failed_readback(read_failure: bool) {
    use review_kit_application::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence};

    struct FailReadback;
    impl ExecutionEvidence for FailReadback {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if matches!(event, ExecutionEvent::Readback { .. }) {
                Err(failure("evidence"))
            } else {
                Ok(())
            }
        }
    }

    let (mut host, mut source, mut renderer) = fixtures();
    let value = request("initial", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::Rejected(Effect::CreateSummary));
    if read_failure {
        host.fail_on_discovery = Some(3);
    }
    let result = publish_recorded(
        &value,
        &mut host,
        &mut source,
        &mut renderer,
        &mut FailReadback,
    )
    .unwrap();

    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert!(matches!(result.stop, Some(Stop::Rejected(_))));
    assert_eq!(result.attempted.len(), 1);
    assert!(result.verified.is_empty());
    assert!(host.snapshot.summaries.is_empty());
    assert!(host.progress.unresolved.is_none());

    host.fail_on_discovery = None;
    let retried = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(retried.status, PublicationStatus::Completed);
    assert_eq!(retried.round, 1);
}

#[test]
fn rejected_initial_summary_stays_not_started_when_readback_read_fails() {
    check_rejected_initial_summary_with_failed_readback(true);
}

#[test]
fn rejected_initial_summary_stays_not_started_when_readback_recording_fails() {
    check_rejected_initial_summary_with_failed_readback(false);
}

#[test]
fn acknowledged_verified_updates_can_resume_after_readback_recording_failure() {
    use review_kit_application::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence};
    struct FailReadback {
        summary: bool,
    }
    impl ExecutionEvidence for FailReadback {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if let ExecutionEvent::Readback { write, .. } = event {
                let selected = if self.summary {
                    matches!(
                        write,
                        Write::Summary {
                            expected: Some(_),
                            ..
                        }
                    )
                } else {
                    matches!(write, Write::SetResolved { .. })
                };
                if selected {
                    return Err(failure("evidence"));
                }
            }
            Ok(())
        }
    }
    for summary in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        host.writes.clear();
        let value = request("fixed", Some(FindingStatus::Fixed), false);
        let failed = publish_recorded(
            &value,
            &mut host,
            &mut source,
            &mut renderer,
            &mut FailReadback { summary },
        )
        .unwrap();
        assert_eq!(failed.status, PublicationStatus::Uncertain);
        assert!(matches!(failed.stop, Some(Stop::Evidence(_))));
        assert!(host.progress.unresolved.is_none());
        host.writes.clear();
        let recovered = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(recovered.status, PublicationStatus::Completed);
        assert_eq!(recovered.round, failed.round);
        assert!(host.progress.unresolved.is_none());
        if !summary {
            assert_eq!(effects(&host), vec![Effect::UpdateSummary]);
        }
    }
}

#[test]
fn acknowledged_updates_can_resume_after_provider_readback_failure() {
    for summary in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        host.writes.clear();
        host.fail_on_discovery = Some(host.discoveries + if summary { 3 } else { 7 });
        let value = request("fixed", Some(FindingStatus::Fixed), false);
        let failed = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(failed.status, PublicationStatus::Uncertain);
        assert!(matches!(failed.stop, Some(Stop::Read(_))));
        assert!(host.progress.unresolved.is_some());
        assert!(matches!(
            host.writes.last().unwrap(),
            Write::Summary {
                expected: Some(_),
                ..
            } | Write::SetResolved { .. }
        ));
        host.writes.clear();
        let recovered = publish(&value, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(recovered.status, PublicationStatus::Completed);
        assert_eq!(recovered.round, failed.round);
        assert!(host.progress.unresolved.is_none());
        if !summary {
            assert_eq!(effects(&host), vec![Effect::UpdateSummary]);
        }
    }
}

#[test]
fn abandonment_keeps_and_authorizes_the_round_confirmation_mark() {
    use review_kit_application::use_cases::publishing::{UnresolvedWrite, WritePhase};
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::Rejected(Effect::Finding));
    let mut req = request("initial", Some(FindingStatus::Open), false);
    req.confirmed_validation = true;
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    host.writes.clear();
    let value = abandon_request(&host);
    let abandoned = abandon_observed(&value, &mut host, &mut renderer).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    let close = host
        .writes
        .iter()
        .find(|write| matches!(write, Write::Summary { round, .. } if round.status == RoundStatus::Abandoned))
        .unwrap()
        .clone();
    let Write::Summary { round, .. } = &close else {
        unreachable!()
    };
    assert!(round.confirmed_validation);
    let pending = |write: Write| UnresolvedWrite {
        subject: subject(),
        reviewer: reviewer("agent"),
        write,
        renderer: None,
        phase: WritePhase::Prepared,
        baseline: None,
    };
    assert_eq!(
        pending(close.clone()).abandonment_round(),
        Some(round.number)
    );
    let mut flipped = close;
    if let Write::Summary { round, .. } = &mut flipped {
        round.confirmed_validation = false;
    }
    assert_eq!(pending(flipped).abandonment_round(), None);
}
