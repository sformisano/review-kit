use super::support::PublicationProgress;
use review_kit_application::{
    ports::{PortError, ReviewReader, ReviewRenderer},
    publication::{Answer, Effects, ProviderId},
    summary::SummaryView,
    use_cases::publishing::{PublicationStatus, PublishRequest, publish},
};
use review_kit_domain::{Finding, FindingId, FindingStatus};
use review_kit_gitlab_adapter::GitLab;
use serde_json::{Value, json};

use super::support::{Fixture, MR_PATH, Renderer, finding, request};

#[derive(Clone, Copy, Debug)]
enum BodySurface {
    Summary,
    Finding,
    Reply,
}

struct BodyRenderer {
    inner: Renderer,
    surface: BodySurface,
    body: String,
}

impl BodyRenderer {
    fn new(surface: BodySurface, body: &str) -> Self {
        Self {
            inner: Renderer::default(),
            surface,
            body: body.to_owned(),
        }
    }
}

impl ReviewRenderer for BodyRenderer {
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError> {
        if matches!(self.surface, BodySurface::Finding) {
            Ok(self.body.clone())
        } else {
            self.inner.finding(finding)
        }
    }

    fn transition(
        &mut self,
        finding: &Finding,
        previous: FindingStatus,
        explanation: &str,
    ) -> Result<String, PortError> {
        self.inner.transition(finding, previous, explanation)
    }

    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        if matches!(self.surface, BodySurface::Reply) {
            Ok(self.body.clone())
        } else {
            self.inner.answer(body)
        }
    }

    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError> {
        if matches!(self.surface, BodySurface::Summary) {
            Ok(self.body.clone())
        } else {
            self.inner.summary(view)
        }
    }
}

fn review_with_root(client: &GitLab) -> PublishRequest {
    let mut progress = PublicationProgress::default();
    let req = request(
        client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
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
    req
}

fn request_for_surface(fixture: &Fixture, client: &GitLab, surface: BodySurface) -> PublishRequest {
    match surface {
        BodySurface::Summary => request(client, vec![]),
        BodySurface::Finding => request(
            client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
        ),
        BodySurface::Reply => {
            let mut req = review_with_root(client);
            let question = fixture
                .state
                .lock()
                .unwrap()
                .question("discussion-1", "Why does the request disappear?");
            req.answer_targets = client.clone().discover(client.subject()).unwrap().replies;
            req.answers.push(Answer {
                finding: FindingId::try_from(1).unwrap(),
                in_reply_to: ProviderId::new(question.to_string()).unwrap(),
                body: "The failure branch discards its value.".into(),
            });
            req
        }
    }
}

fn effect_path(surface: BodySurface) -> String {
    match surface {
        BodySurface::Summary => format!("{MR_PATH}/notes"),
        BodySurface::Finding => format!("{MR_PATH}/discussions"),
        BodySurface::Reply => format!("{MR_PATH}/discussions/discussion-1/notes"),
    }
}

fn assert_no_effect_request(fixture: &Fixture, surface: BodySurface) {
    let path = effect_path(surface);
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .writes()
            .iter()
            .all(|request| request.path != path),
        "unexpected HTTP mutation for {surface:?}"
    );
}

#[test]
fn carriage_returns_in_rendered_bodies_stop_before_any_new_http_write() {
    for surface in [
        BodySurface::Summary,
        BodySurface::Finding,
        BodySurface::Reply,
    ] {
        for body in ["Review explanation\r\nMore detail", "Review\rexplanation"] {
            let fixture = Fixture::new();
            let client = fixture.connect();
            let req = request_for_surface(&fixture, &client, surface);
            let writes_before = fixture.state.lock().unwrap().writes().len();
            let mut renderer = BodyRenderer::new(surface, body);
            let mut host = client.publication_host(Effects::review());
            let mut progress = PublicationProgress::default();
            let outcome = publish(
                &req,
                &mut host,
                &mut client.clone(),
                &mut renderer,
                &mut progress,
            );
            if let Ok(result) = outcome {
                assert_ne!(result.status, PublicationStatus::Completed);
            }
            assert_eq!(
                fixture.state.lock().unwrap().writes().len(),
                writes_before,
                "rendered carriage return caused a write for {surface:?}: {body:?}"
            );
            assert!(progress.unresolved.is_none());
        }
    }
}

#[test]
fn invalid_rendered_finding_suggestions_stop_before_any_http_write() {
    for (with_suggestion, body) in [
        (true, "Review explanation without its suggestion"),
        (true, "```suggestion:-0+0\nwrong replacement\n```"),
        (true, "```suggestion:-1+0\ncorrected\n```"),
        (false, "```suggestion:-0+0\nunvalidated replacement\n```"),
    ] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let range = with_suggestion.then(|| (2, vec!["added"], vec!["corrected"]));
        let req = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, range)],
        );
        let mut renderer = BodyRenderer::new(BodySurface::Finding, body);
        let mut host = client.publication_host(Effects::review());
        let mut progress = PublicationProgress::default();
        let outcome = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut renderer,
            &mut progress,
        );
        if let Ok(result) = outcome {
            assert_ne!(result.status, PublicationStatus::Completed);
        }
        assert!(
            fixture.state.lock().unwrap().writes().is_empty(),
            "invalid rendered suggestion caused a write: {body:?}"
        );
        assert!(progress.unresolved.is_none());
    }
}

#[test]
fn quick_actions_in_rendered_summary_finding_and_reply_stop_before_http() {
    let mut progress = PublicationProgress::default();
    for surface in [
        BodySurface::Summary,
        BodySurface::Finding,
        BodySurface::Reply,
    ] {
        for command in [
            "/close",
            "/CLOSE  \t",
            "\r/close",
            "/cl\rose",
            "/unknown_action",
            "``/close `\n/approve\ntext` ``",
            "`start\n/close\nend`",
        ] {
            let fixture = Fixture::new();
            let client = fixture.connect();
            let req = request_for_surface(&fixture, &client, surface);
            let body = format!("Review explanation\n\n{command}");
            let mut renderer = BodyRenderer::new(surface, &body);
            let mut host = client.publication_host(Effects::review());
            if let Ok(result) = publish(
                &req,
                &mut host,
                &mut client.clone(),
                &mut renderer,
                &mut progress,
            ) {
                assert_ne!(
                    result.status,
                    PublicationStatus::Completed,
                    "{surface:?}: {command:?}"
                );
            }
            assert_no_effect_request(&fixture, surface);
        }
    }
}

#[test]
fn extra_suggestion_blocks_in_summaries_and_replies_stop_before_http() {
    let mut progress = PublicationProgress::default();
    for surface in [BodySurface::Summary, BodySurface::Reply] {
        for body in [
            "Review explanation\n\n```suggestion\nunvalidated replacement\n```",
            "Review explanation\n\n```sugges\rtion\nunvalidated replacement\n```",
            "Review explanation\n\n```\rsuggestion\nunvalidated replacement\n```",
            "Review explanation\n\n~~~suggestion:\r-0+0\nunvalidated replacement\n~~~",
            "Review explanation\n\n> ```suggestion\n> unvalidated replacement\n> ```",
            "Review explanation\n\n> ```sugges\rtion\n> unvalidated replacement\n> ```",
            "Review explanation\n\n<pre lang=\"suggestion\">unvalidated replacement</pre>",
            "Review explanation\n\n<p\rre lang=\"suggestion\">unvalidated replacement</pre>",
            "Review explanation\n\n<c\rode>unvalidated replacement</code>",
        ] {
            let fixture = Fixture::new();
            let client = fixture.connect();
            let req = request_for_surface(&fixture, &client, surface);
            let writes_before = fixture.state.lock().unwrap().writes().len();
            let mut renderer = BodyRenderer::new(surface, body);
            let mut host = client.publication_host(Effects::review());
            if let Ok(result) = publish(
                &req,
                &mut host,
                &mut client.clone(),
                &mut renderer,
                &mut progress,
            ) {
                assert_ne!(
                    result.status,
                    PublicationStatus::Completed,
                    "{surface:?}: {body:?}"
                );
            }
            assert_no_effect_request(&fixture, surface);
            assert_eq!(
                fixture.state.lock().unwrap().writes().len(),
                writes_before,
                "invalid rendered suggestion caused a write for {surface:?}: {body:?}"
            );
        }
    }
}

#[test]
fn literal_slash_commands_in_code_are_published_without_changing_body_bytes() {
    let mut progress = PublicationProgress::default();
    for surface in [
        BodySurface::Summary,
        BodySurface::Finding,
        BodySurface::Reply,
    ] {
        for body in [
            "Review explanation\n\n```text\n/close\n```",
            "Review explanation\n\n~~~~text\n/close\n~~~~",
            "Review explanation\n\n    /close",
            "Review explanation\n\n`/close`",
        ] {
            let fixture = Fixture::new();
            let client = fixture.connect();
            let req = request_for_surface(&fixture, &client, surface);
            let mut renderer = BodyRenderer::new(surface, body);
            let mut host = client.publication_host(Effects::review());
            let result = publish(
                &req,
                &mut host,
                &mut client.clone(),
                &mut renderer,
                &mut progress,
            )
            .unwrap();
            assert_eq!(
                result.status,
                PublicationStatus::Completed,
                "{surface:?}: {body:?}"
            );
            let state = fixture.state.lock().unwrap();
            let path = effect_path(surface);
            let sent = state
                .writes()
                .into_iter()
                .find(|request| request.path == path)
                .unwrap();
            assert!(sent.payload["body"].as_str().unwrap().starts_with(body));
        }
    }
}

#[test]
fn literal_slash_command_in_validated_suggestion_keeps_its_exact_replacement() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(
            1,
            "src/lib.rs",
            2,
            FindingStatus::Open,
            Some((2, vec!["added"], vec!["/close"])),
        )],
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
    let state = fixture.state.lock().unwrap();
    let root = &state.discussions[0]["notes"][0];
    assert!(
        root["body"]
            .as_str()
            .unwrap()
            .contains("```suggestion:-0+0\n/close\n```")
    );
    assert_eq!(root["suggestions"][0]["from_content"], "added\n");
    assert_eq!(root["suggestions"][0]["to_content"], "/close\n");
}

fn unexpected_suggestion() -> Value {
    json!([{
        "id": 700,
        "from_line": 2,
        "to_line": 2,
        "from_content": "added\n",
        "to_content": "unvalidated replacement\n",
        "appliable": true,
        "applied": false
    }])
}

#[test]
fn owned_marked_reply_with_extra_provider_suggestion_is_rejected_on_http_readback() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request_for_surface(&fixture, &client, BodySurface::Reply);
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
    {
        let mut state = fixture.state.lock().unwrap();
        let notes = state.discussions[0]["notes"].as_array_mut().unwrap();
        let position = notes[0]["position"].clone();
        let reply = notes
            .iter_mut()
            .skip(1)
            .find(|note| note["author"]["id"] == 42)
            .unwrap();
        assert!(
            reply["body"]
                .as_str()
                .unwrap()
                .contains("\"kind\":\"answer\"")
        );
        reply["type"] = "DiffNote".into();
        reply["position"] = position;
        reply["resolvable"] = true.into();
        reply["suggestions"] = unexpected_suggestion();
    }
    assert!(client.clone().discover(client.subject()).is_err());
}

#[test]
fn foreign_reply_with_provider_suggestion_remains_foreign_content() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    review_with_root(&client);
    let body = "Could this work?\n\n```suggestion\nunvalidated replacement\n```";
    let question = {
        let mut state = fixture.state.lock().unwrap();
        let id = state.question("discussion-1", body);
        let notes = state.discussions[0]["notes"].as_array_mut().unwrap();
        let position = notes[0]["position"].clone();
        let reply = notes.iter_mut().find(|note| note["id"] == id).unwrap();
        reply["type"] = "DiffNote".into();
        reply["position"] = position;
        reply["resolvable"] = true.into();
        reply["suggestions"] = unexpected_suggestion();
        id
    };
    let snapshot = client.clone().discover(client.subject()).unwrap();
    let reply = snapshot
        .replies
        .iter()
        .find(|reply| reply.id.as_str() == question.to_string())
        .unwrap();
    assert_eq!(reply.author.as_str(), "77");
    assert_eq!(reply.body, body);
    assert!(reply.action.is_none());
}
