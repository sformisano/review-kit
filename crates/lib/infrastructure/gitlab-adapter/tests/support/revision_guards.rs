use super::support::PublicationProgress;
use std::error::Error;

use review_kit_application::{
    ports::{PublicationHost, WriteOutcome},
    publication::{ActionId, Effects, PlannedAction, Write},
    use_cases::publishing::{PublicationStatus, publish},
};
use review_kit_domain::FindingStatus;
use review_kit_gitlab_adapter::GitLabError;

use super::support::{Fault, Fixture, MR_PATH, Renderer, Response, finding, request, transition};

fn advance_revision(fixture: &Fixture) {
    let mut state = fixture.state.lock().unwrap();
    state.historical_head = Some(state.head.clone());
    state.head = "4".repeat(40);
}

#[test]
fn stale_replies_resolution_and_normal_summary_updates_are_rejected_before_http_writes() {
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
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/discussions/discussion-1/notes"),
        Response::status(403),
    ));
    let mut fixed = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None)],
    );
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    let partial = publish(
        &fixed,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    let summary = &partial.final_snapshot.summaries[0];
    let root = &partial.final_snapshot.findings[0];
    let (finding_id, kind, body) = summary
        .current
        .actions
        .iter()
        .find_map(|action| match action {
            PlannedAction::Reply {
                finding,
                kind,
                body,
            } => Some((*finding, kind.clone(), body.clone())),
            _ => None,
        })
        .unwrap();
    let writes = [
        Write::Reply {
            thread: root.id.clone(),
            action: ActionId {
                round: summary.current.number,
                finding: finding_id,
                kind,
            },
            body,
        },
        Write::SetResolved {
            thread: root.id.clone(),
            resolved: true,
        },
        Write::Summary {
            expected: Some(Box::new(summary.clone())),
            round: Box::new(summary.current.clone()),
            history: summary.history.clone(),
            body: summary.body.clone(),
        },
    ];
    let count = fixture.state.lock().unwrap().writes().len();
    advance_revision(&fixture);
    let mut session = host.acquire(client.subject(), client.reviewer()).unwrap();
    for write in writes {
        assert!(matches!(
            session.write(client.subject(), client.reviewer(), &write),
            WriteOutcome::Rejected(ref error) if error.source().and_then(|source| source.downcast_ref::<GitLabError>()) == Some(&GitLabError::RevisionChanged)
        ));
    }
    assert_eq!(fixture.state.lock().unwrap().writes().len(), count);
    let snapshot = session.discover(client.subject()).unwrap();
    assert!(snapshot.replies.is_empty());
    assert!(!snapshot.findings[0].resolved);
    assert_eq!(snapshot.summaries[0], *summary);
}
