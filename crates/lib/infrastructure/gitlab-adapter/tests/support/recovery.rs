use super::support::PublicationProgress;
use review_kit_application::{
    publication::Effects,
    use_cases::publishing::{PublicationStatus, Stop, publish},
};
use review_kit_domain::FindingStatus;

use super::support::{Fault, Fixture, MR_PATH, Renderer, Response, finding, request};

#[test]
fn committed_writes_with_lost_http_responses_are_reused_without_duplicates() {
    for path in [format!("{MR_PATH}/notes"), format!("{MR_PATH}/discussions")] {
        let mut progress = PublicationProgress::default();
        let fixture = Fixture::with_lost_responses();
        let client = fixture.connect();
        fixture
            .state
            .lock()
            .unwrap()
            .faults
            .push(Fault::lost(&path, true));
        let req = request(
            &client,
            vec![
                finding(1, "src/lib.rs", 2, FindingStatus::Open, None),
                finding(2, "src/new.rs", 1, FindingStatus::Open, None),
            ],
        );
        let mut host = client.publication_host(Effects::review());
        let result = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
        assert!(progress.unresolved.is_none());
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.notes.len(), 1);
        assert_eq!(state.discussions.len(), 2);
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|request| request.method == "POST"
                    && request.path == format!("{MR_PATH}/notes"))
                .count(),
            1
        );
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|request| request.method == "POST"
                    && request.path == format!("{MR_PATH}/discussions"))
                .count(),
            2
        );
    }
}

#[test]
fn unresolved_http_write_stays_uncertain_with_caller_progress_after_restart() {
    for path in [format!("{MR_PATH}/notes"), format!("{MR_PATH}/discussions")] {
        let mut progress = PublicationProgress {
            recovery_policy:
                review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative,
            ..Default::default()
        };
        let fixture = Fixture::with_lost_responses();
        let client = fixture.connect();
        fixture
            .state
            .lock()
            .unwrap()
            .faults
            .push(Fault::lost(&path, false));
        let req = request(
            &client,
            vec![
                finding(1, "src/lib.rs", 2, FindingStatus::Open, None),
                finding(2, "src/new.rs", 1, FindingStatus::Open, None),
            ],
        );
        let mut host = client.publication_host(Effects::review());
        let result = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Uncertain);
        assert!(progress.unresolved.is_some());
        let retained = progress.clone();
        let writes = fixture.state.lock().unwrap().writes().len();
        drop(host);
        drop(client);
        let restarted = fixture.connect();
        let mut restarted_host = restarted.publication_host(Effects::review());
        let mut restarted_progress = retained.clone();
        let retry = publish(
            &req,
            &mut restarted_host,
            &mut restarted.clone(),
            &mut Renderer::default(),
            &mut restarted_progress,
        )
        .unwrap();
        assert_eq!(retry.status, PublicationStatus::Uncertain);
        assert!(matches!(retry.stop, Some(Stop::UnresolvedWrite)));
        assert_eq!(restarted_progress, retained);
        assert!(retry.attempted.is_empty());
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.writes().len(), writes);
        assert!(state.discussions.is_empty());
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|request| request.path == path && request.method == "POST")
                .count(),
            1
        );
    }
}

#[test]
fn a_late_committed_finding_is_discovered_before_recovery_resumes() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::with_lost_responses();
    let client = fixture.connect();
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    assert_eq!(
        publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Uncertain
    );
    {
        let mut state = fixture.state.lock().unwrap();
        let request = state
            .writes()
            .into_iter()
            .find(|request| request.path == format!("{MR_PATH}/discussions"))
            .unwrap()
            .clone();
        state.commit(&request);
    }
    let restarted = fixture.connect();
    let mut resumed = restarted.publication_host(Effects::review());
    assert_eq!(
        publish(
            &req,
            &mut resumed,
            &mut restarted.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(
        state
            .writes()
            .iter()
            .filter(|request| request.path == format!("{MR_PATH}/discussions"))
            .count(),
        1
    );
}

#[test]
fn confirmed_http_rejection_can_resume_the_retained_round() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/discussions"),
        Response::status(403),
    ));
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    let rejected = publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(rejected.status, PublicationStatus::Partial);
    assert!(matches!(rejected.stop, Some(Stop::Rejected(_))));
    assert!(progress.unresolved.is_none());
    assert!(fixture.state.lock().unwrap().discussions.is_empty());
    let resumed = publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert_eq!(resumed.round, 1);
    assert_eq!(fixture.state.lock().unwrap().discussions.len(), 1);
}

#[test]
fn server_error_responses_do_not_claim_confirmed_rejection() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/notes"),
        Response::status(503),
    ));
    let req = request(&client, vec![]);
    let mut host = client.publication_host(Effects::review());
    assert_eq!(
        publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Uncertain
    );
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 1);
}
