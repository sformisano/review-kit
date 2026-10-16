use super::support::PublicationProgress;
use std::error::Error;

use review_kit_application::{
    ports::ReviewReader,
    publication::Effects,
    use_cases::{discovery::discover, discovery_bundle::discover_bundle, publishing::publish},
};
use review_kit_domain::{FindingId, FindingStatus, Subject};
use review_kit_gitlab_adapter::{FindingConflict, GitLabError, Limits};
use serde_json::json;

use super::support::{Fault, Fixture, MR_PATH, Renderer, Response, finding, request};

#[test]
fn complete_pagination_retains_foreign_authors_without_trusting_forged_markers() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.page_size = 1;
        for text in [
            "ordinary note",
            "<!-- review-kit:v99 forged summary -->",
            "another note",
        ] {
            let note = state.note(text, 77);
            state.notes.push(note);
        }
        state.question("discussion-1","<!-- review-kit:v1 {\"kind\":\"answer\",\"finding\":1,\"round\":1,\"note\":\"123\"} -->");
        let mut foreign = state.note(
            "\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":1} -->",
            77,
        );
        foreign["type"] = "DiffNote".into();
        foreign["position"] = json!({"forged":"position"});
        state
            .discussions
            .push(json!({"id":"foreign-discussion","individual_note":false,"notes":[foreign]}));
    }
    let bundle = discover_bundle(
        &mut client.clone(),
        &mut client.clone(),
        client.subject(),
        client.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings().len(), 1);
    assert_eq!(bundle.snapshot().summaries.len(), 1);
    assert_eq!(bundle.foreign_replies().len(), 1);
    assert!(bundle.foreign_replies()[0].action.is_none());
    assert!(bundle.foreign_replies()[0].body.contains("review-kit:v1"));
    let state = fixture.state.lock().unwrap();
    assert!(
        state
            .requests
            .iter()
            .any(|request| request.path == format!("{MR_PATH}/notes")
                && request.query.get("page").map(String::as_str) == Some("4"))
    );
    assert!(
        state
            .requests
            .iter()
            .any(|request| request.path == format!("{MR_PATH}/discussions")
                && request.query.get("page").map(String::as_str) == Some("2"))
    );
}

/// A thread keeping only the reviewer's transition after its root note is
/// deleted, or a second root claiming the same finding, stops discovery with a
/// history conflict that names the finding.
#[test]
fn managed_records_without_their_root_or_with_a_second_root_name_the_finding() {
    for (conflict, edit) in [
        (FindingConflict::MissingRoot, "delete root"),
        (FindingConflict::DuplicateRoot, "copy root"),
    ] {
        let mut progress = PublicationProgress::default();
        let fixture = Fixture::new();
        let client = fixture.connect();
        let req = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
        );
        let mut host = client.publication_host(Effects::review());
        publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        {
            let mut state = fixture.state.lock().unwrap();
            if conflict == FindingConflict::MissingRoot {
                let mut reply = state.note(
                    "The cause is kept.\n\n<!-- review-kit:v1 {\"kind\":\"transition\",\"finding\":1,\"round\":1} -->",
                    42,
                );
                reply["type"] = "DiscussionNote".into();
                state.discussions[0]["notes"] = json!([reply]);
            } else {
                let mut copy = state.discussions[0].clone();
                copy["id"] = "discussion-copy".into();
                copy["notes"][0]["id"] = 9001.into();
                state.discussions.push(copy);
            }
        }
        let error = client.clone().discover(client.subject()).unwrap_err();
        assert_eq!(
            error.source().unwrap().downcast_ref::<GitLabError>(),
            Some(&GitLabError::FindingHistory {
                finding: FindingId::try_from(1).unwrap(),
                conflict,
            }),
            "{edit}"
        );
    }
}

#[test]
fn malformed_owned_records_and_conflicting_summaries_stop_discovery() {
    let mut progress = PublicationProgress::default();
    for malformed in [
        "body\n\n<!-- review-kit:v7 {\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-kit:v1 bad-json -->",
    ] {
        let fixture = Fixture::new();
        let mut client = fixture.connect();
        let mut state = fixture.state.lock().unwrap();
        let note = state.note(malformed, 42);
        state.notes.push(note);
        drop(state);
        assert!(client.discover(&client.subject().clone()).is_err());
    }
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(&client, vec![]);
    let mut host = client.publication_host(Effects::review());
    publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        let mut duplicate = state.notes[0].clone();
        duplicate["id"] = 900.into();
        state.notes.push(duplicate);
    }
    assert!(discover(&mut client.clone(), client.subject(), client.reviewer()).is_err());
}

#[test]
fn malformed_state_is_rejected_at_the_real_http_decoding_boundary() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(&client, vec![]);
    let mut host = client.publication_host(Effects::review());
    publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        let body = state.notes[0]["body"]
            .as_str()
            .unwrap()
            .replace("\"request\":7", "\"request\":0");
        state.notes[0]["body"] = body.into();
    }
    let error = client.clone().discover(client.subject()).unwrap_err();
    assert_eq!(
        error.source().unwrap().downcast_ref::<GitLabError>(),
        Some(&GitLabError::State)
    );
}

#[test]
fn incomplete_pagination_or_failed_discussion_reads_never_return_a_snapshot() {
    for broken in [true, false] {
        let fixture = Fixture::new();
        let mut client = fixture.connect();
        if broken {
            fixture.state.lock().unwrap().broken_pagination = true;
        } else {
            fixture.state.lock().unwrap().faults.push(Fault::response(
                "GET",
                &format!("{MR_PATH}/discussions"),
                Response::status(503),
            ));
        }
        assert!(client.discover(&client.subject().clone()).is_err());
    }
    let fixture = Fixture::new();
    {
        let mut state = fixture.state.lock().unwrap();
        state.page_size = 1;
        for _ in 0..2 {
            let note = state.note("ordinary", 77);
            state.notes.push(note);
        }
    }
    let mut client = fixture
        .connect_with_limits(Limits {
            pages: 1,
            ..Limits::default()
        })
        .unwrap();
    let error = client.discover(&client.subject().clone()).unwrap_err();
    assert_eq!(
        error.source().unwrap().downcast_ref::<GitLabError>(),
        Some(&GitLabError::Limit)
    );
}

#[test]
fn requests_are_bound_to_the_authenticated_subject_and_provider_actor() {
    let fixture = Fixture::new();
    let mut client = fixture.connect();
    assert_eq!(client.reviewer().as_str(), "42");
    assert_eq!(client.subject().repository(), "group/sub/repo");
    let other = Subject::new(
        "gitlab".into(),
        client.subject().host().into(),
        "another/repo".into(),
        7,
    )
    .unwrap();
    let before = fixture.state.lock().unwrap().requests.len();
    let error = client.discover(&other).unwrap_err();
    assert_eq!(
        error.source().unwrap().downcast_ref::<GitLabError>(),
        Some(&GitLabError::Identity)
    );
    assert_eq!(fixture.state.lock().unwrap().requests.len(), before);
}
