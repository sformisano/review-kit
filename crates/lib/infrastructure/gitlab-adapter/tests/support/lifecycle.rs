use review_kit_application::{
    ports::ReviewReader,
    publication::{Effect, Effects, PlannedAction, RoundStatus},
    use_cases::{
        discovery_bundle::discover_bundle,
        publishing::{AbandonRequest, PlanError, PublicationStatus, Stop, abandon, publish},
    },
};
use review_kit_domain::{
    Assessment, AssessmentDraft, FindingId, FindingStatus, PolicyEvaluation, PolicyJudgment,
    ReviewDecision,
};

use super::support::{
    Fault, Fixture, MR_PATH, PublicationProgress, Renderer, Response, finding, request, transition,
};

#[test]
fn blocked_review_repeats_without_writes_and_completes_the_same_round_after_reconnect() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let complete = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut blocked = complete.clone();
    let assessment = &complete.assessment;
    blocked.assessment = Assessment::new(AssessmentDraft {
        subject: assessment.subject().clone(),
        reviewer: assessment.reviewer().clone(),
        revision: assessment.revision().clone(),
        decision: ReviewDecision::Blocked,
        depth: assessment.depth().into(),
        scope: assessment.scope().into(),
        limitation: Some("Required regression is unavailable.".into()),
        selected_policies: assessment.selected_policies().to_vec(),
        evaluations: assessment
            .evaluations()
            .iter()
            .map(|evaluation| {
                PolicyEvaluation::new(
                    evaluation.policy_id().clone(),
                    PolicyJudgment::Blocked {
                        limitation: "Required regression is unavailable.".into(),
                    },
                )
                .unwrap()
            })
            .collect(),
        findings: assessment.findings().to_vec(),
    })
    .unwrap();
    let mut progress = PublicationProgress::default();
    let mut host = client.publication_host(Effects::review());
    let result = publish(
        &blocked,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(result.round, 1);
    assert!(progress.unresolved.is_none());
    assert!(result.final_snapshot.findings.is_empty());
    assert_eq!(
        result.final_snapshot.summaries[0].current.status,
        RoundStatus::Blocked
    );
    assert!(
        result.final_snapshot.summaries[0]
            .current
            .actions
            .is_empty()
    );
    {
        let state = fixture.state.lock().unwrap();
        let writes = state.writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].method, "POST");
        assert_eq!(writes[0].path, format!("{MR_PATH}/notes"));
        assert!(state.discussions.is_empty());
    }
    drop(host);
    drop(client);
    let restarted = fixture.connect();
    let mut host = restarted.publication_host(Effects::review());
    let repeated = publish(
        &blocked,
        &mut host,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(repeated.status, PublicationStatus::Unchanged);
    assert_eq!(repeated.round, result.round);
    assert!(repeated.attempted.is_empty());
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 1);
    let completed = publish(
        &complete,
        &mut host,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(completed.round, result.round);
    let snapshot = restarted.clone().discover(restarted.subject()).unwrap();
    assert_eq!(snapshot.summaries.len(), 1);
    assert_eq!(snapshot.summaries[0].current.status, RoundStatus::Completed);
    assert!(snapshot.summaries[0].history.is_empty());
    assert_eq!(snapshot.findings.len(), 1);
    assert_eq!(snapshot.findings[0].created_in, result.round);
    let state = fixture.state.lock().unwrap();
    let writes = state.writes();
    assert_eq!(writes.len(), 4);
    assert_eq!(writes[1].method, "PUT");
    assert_eq!(writes[2].method, "POST");
    assert_eq!(writes[2].path, format!("{MR_PATH}/discussions"));
    assert_eq!(writes[3].method, "PUT");
    assert_eq!(writes[1].path, writes[3].path);
    assert_eq!(state.notes.len(), 1);
    assert_eq!(state.discussions.len(), 1);
}

#[test]
fn rejection_then_abandonment_preserves_effects_and_reserves_ids_for_a_reconnected_round() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let mut progress = PublicationProgress::default();
    let mut host = client.publication_host(Effects::review());
    let initial = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    assert_eq!(
        publish(
            &initial,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let mut followup = request(
        &client,
        vec![
            finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None),
            finding(2, "src/new.rs", 1, FindingStatus::Open, None),
        ],
    );
    transition(&mut followup, 1, FindingStatus::Open, FindingStatus::Fixed);
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/discussions"),
        Response::status(403),
    ));
    let rejected = publish(
        &followup,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(rejected.status, PublicationStatus::Partial);
    assert!(matches!(rejected.stop, Some(Stop::Rejected(_))));
    assert!(progress.unresolved.is_none());
    assert_eq!(rejected.final_snapshot.findings.len(), 1);
    assert!(rejected.final_snapshot.findings[0].resolved);
    assert_eq!(rejected.final_snapshot.replies.len(), 1);
    let retained = rejected.final_snapshot.summaries[0].current.clone();
    assert_eq!(retained.status, RoundStatus::Partial);
    let verified = retained.actions[..2].to_vec();
    assert_eq!(verified[0].effect(), Effect::Reply);
    assert_eq!(verified[1].effect(), Effect::Resolve);
    let before_abandonment = fixture.state.lock().unwrap().writes().len();
    let abandoned = abandon(
        &AbandonRequest {
            subject: client.subject().clone(),
            reviewer: client.reviewer().clone(),
            round: rejected.round,
            reason: "The remaining finding requires a corrected assessment.".into(),
            effects: Effects::review(),
        },
        &mut host,
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    assert_eq!(
        abandoned.final_snapshot.findings,
        rejected.final_snapshot.findings
    );
    assert_eq!(
        abandoned.final_snapshot.replies,
        rejected.final_snapshot.replies
    );
    assert_eq!(abandoned.skipped, vec![retained.actions[2].clone()]);
    let abandoned_round = abandoned.final_snapshot.summaries[0].current.clone();
    assert_eq!(abandoned_round.status, RoundStatus::Abandoned);
    assert_eq!(abandoned_round.actions, retained.actions);
    assert_eq!(
        abandoned_round
            .abandonment
            .as_ref()
            .unwrap()
            .verified_actions,
        verified
    );
    {
        let state = fixture.state.lock().unwrap();
        let writes = state.writes();
        assert_eq!(writes.len(), before_abandonment + 1);
        assert_eq!(writes[before_abandonment].method, "PUT");
        assert_eq!(
            writes[before_abandonment].path,
            format!(
                "{MR_PATH}/notes/{}",
                abandoned.final_snapshot.summaries[0].id.as_str()
            )
        );
        assert_eq!(state.discussions.len(), 1);
    }
    drop(host);
    drop(client);
    let restarted = fixture.connect();
    let bundle = discover_bundle(
        &mut restarted.clone(),
        &mut restarted.clone(),
        restarted.subject(),
        restarted.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings()[0].status(), FindingStatus::Fixed);
    assert_eq!(bundle.next_finding_id().get(), 3);
    assert_eq!(bundle.snapshot().summaries[0].current, abandoned_round);
    let mut host = restarted.publication_host(Effects::review());
    let writes_before_reuse = fixture.state.lock().unwrap().writes().len();
    let reuse = publish(
        &followup,
        &mut host,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap_err();
    assert!(
        matches!(reuse, PlanError::ReusedFindingId(id) if id == FindingId::try_from(2).unwrap())
    );
    assert_eq!(
        fixture.state.lock().unwrap().writes().len(),
        writes_before_reuse
    );
    let corrected = request(
        &restarted,
        vec![
            finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None),
            finding(3, "src/new.rs", 1, FindingStatus::Open, None),
        ],
    );
    let completed = publish(
        &corrected,
        &mut host,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(completed.round, abandoned.round + 1);
    let snapshot = restarted.clone().discover(restarted.subject()).unwrap();
    assert_eq!(snapshot.summaries[0].current.status, RoundStatus::Completed);
    assert_eq!(snapshot.summaries[0].history.len(), 2);
    assert_eq!(snapshot.summaries[0].history[1], abandoned_round);
    assert_eq!(snapshot.findings[0], rejected.final_snapshot.findings[0]);
    assert_eq!(snapshot.replies, rejected.final_snapshot.replies);
    assert_eq!(snapshot.findings.len(), 2);
    assert_eq!(snapshot.findings[1].finding.id().get(), 3);
    assert!(
        !snapshot.summaries[0]
            .current
            .actions
            .iter()
            .any(|action| matches!(
                action,
                PlannedAction::Reply { .. } | PlannedAction::SetResolved { .. }
            ))
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.notes.len(), 1);
    assert_eq!(state.discussions.len(), 2);
    assert_eq!(
        state.writes()[writes_before_reuse..]
            .iter()
            .filter(|request| request.method == "POST"
                && request.path == format!("{MR_PATH}/discussions"))
            .count(),
        1
    );
}
