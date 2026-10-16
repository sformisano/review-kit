#![allow(clippy::unwrap_used, clippy::expect_used)]
use support::PublicationProgress;

mod support;

#[path = "support/answer_guard.rs"]
mod answer_guard;

#[path = "support/conversation.rs"]
mod conversation;

#[path = "support/evolution.rs"]
mod evolution;

#[path = "support/body_safety.rs"]
mod body_safety;

#[path = "support/lifecycle.rs"]
mod lifecycle;

#[path = "support/observation.rs"]
mod observation;

#[path = "support/evidence_integrity.rs"]
mod evidence_integrity;

use review_kit_application::{
    ports::ReviewReader,
    publication::{Answer, Effect, Effects, ProviderId, RoundStatus},
    use_cases::{
        discovery_bundle::discover_bundle,
        publishing::{PublicationStatus, Stop, publish},
    },
};
use review_kit_domain::{Assessment, AssessmentDraft, FindingId, FindingStatus, Revision};
use serde_json::json;

use support::{Fixture, Renderer, finding, request, transition};

#[test]
fn clean_review_and_multiple_findings_survive_restart_and_unchanged_reruns() {
    let mut progress = PublicationProgress::default();
    for count in [0, 2] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let findings = if count == 0 {
            vec![]
        } else {
            vec![
                finding(1, "src/lib.rs", 2, FindingStatus::Open, None),
                finding(2, "src/new.rs", 1, FindingStatus::Open, None),
            ]
        };
        let req = request(&client, findings);
        let mut host = client.publication_host(Effects::review());
        let mut source = client.clone();
        let mut renderer = Renderer::default();
        let result = publish(&req, &mut host, &mut source, &mut renderer, &mut progress).unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
        assert_eq!(result.verified.len(), count + 2);
        let writes = fixture.state.lock().unwrap().writes().len();
        let restarted = fixture.connect();
        let bundle = discover_bundle(
            &mut restarted.clone(),
            &mut restarted.clone(),
            restarted.subject(),
            restarted.reviewer(),
        )
        .unwrap();
        assert_eq!(bundle.findings().len(), count);
        assert_eq!(bundle.next_finding_id().get(), count as u64 + 1);
        assert_eq!(bundle.snapshot().summaries.len(), 1);
        assert_eq!(
            bundle.snapshot().summaries[0].current.status,
            RoundStatus::Completed
        );
        assert!(bundle.snapshot().summaries[0].history.is_empty());
        assert_eq!(bundle.changes().files.len(), 2);
        assert!(
            bundle
                .changes()
                .unified_diff
                .contains("rename from src/old.rs\nrename to src/lib.rs")
        );
        let mut restarted_host = restarted.publication_host(Effects::review());
        let unchanged = publish(
            &req,
            &mut restarted_host,
            &mut restarted.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(unchanged.status, PublicationStatus::Unchanged);
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.writes().len(), writes);
        assert_eq!(state.notes.len(), 1);
        assert_eq!(state.discussions.len(), count);
        assert!(
            state
                .requests
                .iter()
                .any(|request| request.target.contains("projects/group%2Fsub%2Frepo"))
        );
        let payloads = state
            .writes()
            .iter()
            .map(|request| &request.payload)
            .collect::<Vec<_>>();
        let public_text = serde_json::to_string(&payloads).unwrap();
        assert!(!public_text.contains("PRIVATE local agent context"));
        assert!(!public_text.contains(support::TOKEN));
    }
}

#[test]
fn followups_answer_resolve_and_reopen_the_same_owned_discussion_with_history() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let mut host = client.publication_host(Effects::review());
    let mut source = client.clone();
    let mut renderer = Renderer::default();
    let initial = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    assert_eq!(
        publish(
            &initial,
            &mut host,
            &mut source,
            &mut renderer,
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let question = fixture
        .state
        .lock()
        .unwrap()
        .question("discussion-1", "Has this regression been covered?");
    let mut fixed = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None)],
    );
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    fixed.answer_targets = client.clone().discover(client.subject()).unwrap().replies;
    fixed.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: ProviderId::new(question.to_string()).unwrap(),
        body: "The added regression covers this path.".into(),
    });
    assert_eq!(
        publish(&fixed, &mut host, &mut source, &mut renderer, &mut progress)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    let mut restarted = fixture.connect();
    let bundle = discover_bundle(
        &mut restarted.clone(),
        &mut restarted.clone(),
        restarted.subject(),
        restarted.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings()[0].status(), FindingStatus::Fixed);
    assert_eq!(bundle.foreign_replies().len(), 1);
    assert_eq!(bundle.snapshot().summaries[0].history.len(), 1);
    assert!(bundle.snapshot().findings[0].resolved);
    let mut recurrent = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    transition(&mut recurrent, 1, FindingStatus::Fixed, FindingStatus::Open);
    assert_eq!(
        publish(
            &recurrent,
            &mut host,
            &mut source,
            &mut renderer,
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let snapshot = restarted.discover(client.subject()).unwrap();
    assert_eq!(snapshot.summaries[0].current.number, 3);
    assert_eq!(snapshot.summaries[0].history.len(), 2);
    assert!(!snapshot.findings[0].resolved);
    assert_eq!(snapshot.findings.len(), 1);
    let state = fixture.state.lock().unwrap();
    let replies = state
        .writes()
        .into_iter()
        .filter(|request| {
            request.method == "POST" && request.path.ends_with("/discussions/discussion-1/notes")
        })
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 3);
    assert!(replies.iter().any(|request|request.payload["body"].as_str().unwrap().ends_with(&format!("<!-- review-kit:v1 {{\"kind\":\"answer\",\"finding\":1,\"round\":2,\"note\":\"{question}\"}} -->"))));
    let resolutions = state
        .writes()
        .into_iter()
        .filter(|request| {
            request.method == "PUT" && request.path.ends_with("/discussions/discussion-1")
        })
        .map(|request| request.payload.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        resolutions,
        [json!({"resolved":true}), json!({"resolved":false})]
    );
    assert!(
        state
            .writes()
            .iter()
            .any(|request| request.method == "PUT" && request.path.contains("/notes/"))
    );
}

#[test]
fn authority_limits_actual_http_writes() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut limited = client.publication_host(Effects::new([Effect::CreateSummary]));
    let denied = publish(
        &req,
        &mut limited,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(denied.status, PublicationStatus::Partial);
    assert!(matches!(
        denied.stop,
        Some(Stop::Authority(Effect::Finding))
    ));
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 1);
}

#[test]
fn a_followup_revision_retains_and_rediscovers_the_original_finding_and_round() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let initial = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    publish(
        &initial,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.historical_head = Some(state.head.clone());
        state.head = "4".repeat(40);
    }
    let mut fixed = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None)],
    );
    let old = &fixed.assessment;
    fixed.assessment = Assessment::new(AssessmentDraft {
        subject: old.subject().clone(),
        reviewer: old.reviewer().clone(),
        revision: Revision::new("4".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap(),
        decision: old.decision(),
        depth: old.depth().into(),
        scope: old.scope().into(),
        limitation: old.limitation().map(str::to_owned),
        selected_policies: old.selected_policies().to_vec(),
        evaluations: old.evaluations().to_vec(),
        findings: old.findings().to_vec(),
    })
    .unwrap();
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    let result = publish(
        &fixed,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let restarted = fixture.connect();
    let bundle = discover_bundle(
        &mut restarted.clone(),
        &mut restarted.clone(),
        restarted.subject(),
        restarted.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings()[0].status(), FindingStatus::Fixed);
    assert_eq!(
        bundle.snapshot().summaries[0].history[0]
            .assessment
            .revision()
            .head(),
        "1".repeat(40)
    );
    assert_eq!(
        bundle.snapshot().findings[0].revision.head(),
        "1".repeat(40)
    );
    assert_eq!(bundle.snapshot().revision.head(), "4".repeat(40));
    assert_eq!(fixture.state.lock().unwrap().discussions.len(), 1);
}

#[path = "support/discovery.rs"]
mod discovery;
#[path = "support/recovery.rs"]
mod recovery;
#[path = "support/revision_guards.rs"]
mod revision_guards;
#[path = "support/run.rs"]
mod run;
#[path = "support/source.rs"]
mod source;

#[path = "support/empty_diff.rs"]
mod empty_diff;
#[path = "support/transport.rs"]
mod transport;
