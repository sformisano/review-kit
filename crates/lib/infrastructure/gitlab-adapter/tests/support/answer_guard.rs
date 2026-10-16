use super::support::PublicationProgress;
use std::error::Error;

use review_kit_application::{
    ports::{PortError, PublicationHost, ReviewReader, WriteOutcome},
    publication::{ActionId, Answer, Effects, PlannedAction, ProviderId, ReplyKind, Round, Write},
    use_cases::{
        publication_run::{ExecutionEvent, ExecutionEvidence},
        publishing::{PublicationStatus, Stop, publish, publish_recorded},
    },
};
use review_kit_domain::{FindingId, FindingStatus};
use review_kit_gitlab_adapter::{GitLab, GitLabError};

use super::support::{Fault, Fixture, MR_PATH, Renderer, Response, finding, request};

fn rejected_answer(fixture: &Fixture) -> (GitLab, Round, Write, u64, PublicationProgress) {
    let mut progress = PublicationProgress::default();
    let client = fixture.connect();
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
        .question("discussion-1", "Does the regression cover request loss?");
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/discussions/discussion-1/notes"),
        Response::status(403),
    ));
    let mut followup = initial;
    followup.answer_targets = client.clone().discover(client.subject()).unwrap().replies;
    followup.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: ProviderId::new(question.to_string()).unwrap(),
        body: "The regression covers request loss.".into(),
    });
    let partial = publish(
        &followup,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    assert!(matches!(partial.stop, Some(Stop::Rejected(_))));
    let round = partial.final_snapshot.summaries[0].current.clone();
    let root = &partial.final_snapshot.findings[0];
    let write = round
        .actions
        .iter()
        .find_map(|action| match action {
            PlannedAction::Reply {
                finding,
                kind: kind @ ReplyKind::Answer(_),
                body,
            } => Some(Write::Reply {
                thread: root.id.clone(),
                action: ActionId {
                    round: round.number,
                    finding: *finding,
                    kind: kind.clone(),
                },
                body: body.clone(),
            }),
            _ => None,
        })
        .unwrap();
    assert_eq!(round.answer_targets[0].id.as_str(), question.to_string());
    (client, round, write, question, progress)
}

fn change_question(fixture: &Fixture, question: u64, remove: bool) {
    let mut state = fixture.state.lock().unwrap();
    let notes = state.discussions[0]["notes"].as_array_mut().unwrap();
    let index = notes
        .iter()
        .position(|note| note["id"] == question)
        .unwrap();
    if remove {
        notes.remove(index);
    } else {
        notes[index]["body"] = "Does the regression cover a different failure?".into();
    }
}

struct ChangeBeforeAnswer<'a> {
    fixture: &'a Fixture,
    question: u64,
    remove: bool,
    before: Option<usize>,
}

impl ExecutionEvidence for ChangeBeforeAnswer<'_> {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        if matches!(
            event,
            ExecutionEvent::BeforeWrite(Write::Reply {
                action: ActionId {
                    kind: ReplyKind::Answer(_),
                    ..
                },
                ..
            })
        ) {
            assert!(self.before.is_none());
            change_question(self.fixture, self.question, self.remove);
            self.before = Some(self.fixture.state.lock().unwrap().writes().len());
        }
        Ok(())
    }
}

#[test]
fn application_answers_stop_when_question_changes_after_its_last_prewrite_read() {
    for remove in [false, true] {
        let fixture = Fixture::new();
        let (client, _, _, question, mut progress) = rejected_answer(&fixture);
        let mut followup = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
        );
        followup.answer_targets = client.clone().discover(client.subject()).unwrap().replies;
        followup.answers.push(Answer {
            finding: FindingId::try_from(1).unwrap(),
            in_reply_to: ProviderId::new(question.to_string()).unwrap(),
            body: "The regression covers request loss.".into(),
        });
        let mut host = client.publication_host(Effects::review());
        let mut evidence = ChangeBeforeAnswer {
            fixture: &fixture,
            question,
            remove,
            before: None,
        };
        let result = publish_recorded(
            &followup,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
            &mut evidence,
        )
        .unwrap();
        let writes = evidence.before.unwrap();
        assert_eq!(result.status, PublicationStatus::Partial);
        assert!(matches!(result.stop, Some(Stop::ConcurrentChange)));
        assert!(progress.unresolved.is_none());
        assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
        assert!(result.final_snapshot.replies.iter().all(|reply| {
            reply.action.as_ref().is_none_or(|action| {
                !matches!(&action.kind, ReplyKind::Answer(target) if target.as_str() == question.to_string())
            })
        }));
    }
}

#[test]
fn changed_or_missing_answer_targets_are_rejected_before_http_write() {
    for remove in [false, true] {
        let fixture = Fixture::new();
        let (client, _, write, question, _) = rejected_answer(&fixture);
        change_question(&fixture, question, remove);
        let writes = fixture.state.lock().unwrap().writes().len();
        let mut host = client.publication_host(Effects::review());
        let mut session = host.acquire(client.subject(), client.reviewer()).unwrap();
        assert!(matches!(
            session.write(client.subject(), client.reviewer(), &write),
            WriteOutcome::Rejected(ref error)
                if error.source().and_then(|source| source.downcast_ref::<GitLabError>())
                    == Some(&GitLabError::Conflict)
        ));
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.writes().len(), writes);
        assert!(
            state.discussions[0]["notes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|note| note["author"]["id"] != 42 || note["type"] == "DiffNote")
        );
    }
}
