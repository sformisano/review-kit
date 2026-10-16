use super::support::PublicationProgress;
use std::{sync::mpsc, thread, time::Duration};

use review_kit_application::{
    ports::ReviewReader,
    publication::Effects,
    use_cases::publishing::{PublicationStatus, publish},
};
use review_kit_domain::FindingStatus;
use review_kit_gitlab_adapter::{Credential, GitLabConfig, GitLabError, Limits};

use super::support::{
    Fault, Fixture, MR_PATH, Renderer, Response, TOKEN, finding, request, transition,
};

#[test]
fn authentication_http_errors_and_debug_output_exclude_credentials_and_response_text() {
    for (code, expected) in [
        (401, GitLabError::Authentication),
        (403, GitLabError::Denied),
        (404, GitLabError::NotFound),
        (429, GitLabError::RateLimited),
        (422, GitLabError::Rejected(422)),
    ] {
        let fixture = Fixture::new();
        fixture.state.lock().unwrap().faults.push(Fault::response(
            "GET",
            "/user",
            Response::status(code),
        ));
        let error = fixture
            .connect_with_limits(Limits::default())
            .err()
            .unwrap();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
        assert!(!format!("{error:?} {error}").contains("private provider detail"));
    }
    let secret = Credential::private_token(TOKEN).unwrap();
    assert!(!format!("{secret:?}").contains(TOKEN));
    assert!(Credential::private_token("bad\nvalue").is_err());
    assert!(
        GitLabConfig::new(
            "http://public.example",
            Credential::private_token(TOKEN).unwrap()
        )
        .is_err()
    );
    assert!(
        GitLabConfig::new(
            "https://user:secret@example.com",
            Credential::private_token(TOKEN).unwrap()
        )
        .is_err()
    );
}

#[test]
fn authenticated_redirects_are_refused_without_contacting_the_destination() {
    let destination = Fixture::new();
    let fixture = Fixture::new();
    let mut response = Response::status(302);
    response.headers.push((
        "location".into(),
        format!("{}/api/v4/user", destination.server),
    ));
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response("GET", "/user", response));
    assert_eq!(
        fixture
            .connect_with_limits(Limits::default())
            .err()
            .unwrap(),
        GitLabError::Redirect
    );
    assert!(destination.state.lock().unwrap().requests.is_empty());
}

#[test]
fn redirected_mutations_stay_uncertain_and_are_not_reposted_after_restart() {
    let mut progress = PublicationProgress {
        recovery_policy:
            review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative,
        ..Default::default()
    };
    let destination = Fixture::new();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let mut response = Response::status(302);
    response.headers.push((
        "location".into(),
        format!("{}/api/v4/user", destination.server),
    ));
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/notes"),
        response,
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
        PublicationStatus::Uncertain
    );
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 1);
    assert!(destination.state.lock().unwrap().requests.is_empty());
}

#[test]
fn response_and_request_body_bounds_stop_work_before_unbounded_data_or_writes() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let response = Response {
        status: 200,
        headers: vec![],
        body: vec![b'x'; 256],
    };
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response("GET", "/user", response));
    assert_eq!(
        fixture
            .connect_with_limits(Limits {
                response_bytes: 64,
                ..Limits::default()
            })
            .err()
            .unwrap(),
        GitLabError::Limit
    );
    let fixture = Fixture::new();
    let client = fixture
        .connect_with_limits(Limits {
            body_bytes: 64,
            ..Limits::default()
        })
        .unwrap();
    let req = request(&client, vec![]);
    let mut host = client.publication_host(Effects::review());
    let result = publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert!(fixture.state.lock().unwrap().writes().is_empty());
}

#[test]
fn unknown_resolution_put_keeps_caller_progress_despite_matching_manual_change() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::with_lost_responses();
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
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost_method(
            "PUT",
            &format!("{MR_PATH}/discussions/discussion-1"),
            false,
        ));
    let mut fixed = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None)],
    );
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    assert_eq!(
        publish(
            &fixed,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Uncertain
    );
    let unresolved = progress.unresolved.clone();
    assert!(unresolved.is_some());
    fixture.state.lock().unwrap().discussions[0]["notes"][0]["resolved"] = true.into();
    let restarted = fixture.connect();
    assert!(
        restarted
            .clone()
            .discover(restarted.subject())
            .unwrap()
            .findings[0]
            .resolved
    );
    let mut resumed = restarted.publication_host(Effects::review());
    let writes = fixture.state.lock().unwrap().writes().len();
    let result = publish(
        &fixed,
        &mut resumed,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(result.stop.is_none());
    assert!(progress.unresolved.is_none());
    assert_eq!(
        progress.verified_resolutions.last().unwrap().provenance,
        review_kit_application::publication::ResolutionProvenance::Observed
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.writes().len(), writes + 2);
    assert_eq!(
        state
            .writes()
            .iter()
            .filter(
                |request| request.path.ends_with("/discussions/discussion-1")
                    && request.method == "PUT"
            )
            .count(),
        1
    );
}

#[test]
fn unknown_summary_update_remains_uncertain_even_when_new_body_is_observed() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::with_lost_responses();
    let client = fixture.connect();
    let initial = request(&client, vec![]);
    let mut host = client.publication_host(Effects::review());
    let created = publish(
        &initial,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    let summary_id = created.final_snapshot.summaries[0].id.as_str();
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost_method(
            "PUT",
            &format!("{MR_PATH}/notes/{summary_id}"),
            true,
        ));
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
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
    let writes = fixture.state.lock().unwrap().writes().len();
    let restarted = fixture.connect();
    assert_eq!(
        restarted
            .clone()
            .discover(restarted.subject())
            .unwrap()
            .summaries[0],
        result.final_snapshot.summaries[0]
    );
    let mut resumed = restarted.publication_host(Effects::review());
    let result = publish(
        &req,
        &mut resumed,
        &mut restarted.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(result.stop.is_none());
    assert!(result.attempted.is_empty());
    assert!(progress.unresolved.is_none());
    assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
    assert_eq!(fixture.state.lock().unwrap().discussions.len(), 1);
}

#[test]
fn nullable_unlocked_discussion_state_supports_normal_publication() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().discussion_locked = None;
    let client = fixture.connect();
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
        PublicationStatus::Completed
    );
}

#[test]
fn configured_network_timeout_ends_a_stalled_authenticated_request() {
    let fixture = Fixture::new();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::hold("/user", ready_tx, release_rx));
    let server = fixture.server.clone();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let config = GitLabConfig::new(&server, Credential::private_token(TOKEN).unwrap())
            .unwrap()
            .with_limits(Limits {
                timeout: Duration::from_millis(100),
                connect_timeout: Duration::from_millis(100),
                ..Limits::default()
            })
            .unwrap();
        let error = review_kit_gitlab_adapter::GitLab::connect(
            config,
            &format!("{server}/group/sub/repo/-/merge_requests/7"),
        )
        .err();
        result_tx.send(error).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = result_rx.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(result.unwrap(), Some(GitLabError::Transport));
}
