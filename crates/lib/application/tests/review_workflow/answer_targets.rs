use super::*;

#[test]
fn new_answers_require_the_complete_retained_question_before_first_live_planning() {
    for changed in [
        "missing-observation",
        "body",
        "author",
        "thread",
        "id",
        "deleted",
    ] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        foreign_question(&mut host);
        let mut req = answer_request("answer");
        let target = host.snapshot.replies.last_mut().unwrap();
        match changed {
            "missing-observation" => req.answer_targets.clear(),
            "body" => target.body.push_str(" Please include cancellation."),
            "author" => target.author = reviewer("another-author"),
            "thread" => target.thread = provider_id("another-thread"),
            "id" => target.id = provider_id("another-question"),
            "deleted" => host.snapshot.replies.clear(),
            _ => unreachable!(),
        }
        let planned = plan(&req, &mut Session(&mut host), &mut source, &mut renderer).unwrap_err();
        assert!(
            has_plan_error(&planned, &|error| matches!(
                error,
                PlanError::InvalidAnswer(_)
            )),
            "{changed}: {planned:?}"
        );
        let published = publish(&req, &mut host, &mut source, &mut renderer).unwrap_err();
        assert!(
            has_plan_error(&published, &|error| matches!(
                error,
                PlanError::InvalidAnswer(_)
            )),
            "{changed}: {published:?}"
        );
        assert!(host.writes.is_empty(), "{changed}");
        assert_eq!(host.snapshot.summaries[0].current.number, 1);
    }
}

#[test]
fn unrelated_replies_do_not_invalidate_a_retained_answer_target() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let req = answer_request("answer");
    let mut other = question_record();
    other.id = provider_id("another-question");
    other.body = "An unrelated follow-up.".into();
    host.snapshot.replies.push(other);
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host)
            .iter()
            .filter(|effect| **effect == Effect::Reply)
            .count(),
        1
    );
}

#[test]
fn a_new_round_does_not_repeat_an_already_published_answer_after_its_question_changes() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let answered = answer_request("answer");
    publish(&answered, &mut host, &mut source, &mut renderer).unwrap();
    host.snapshot
        .replies
        .iter_mut()
        .find(|reply| reply.id == provider_id("question"))
        .unwrap()
        .body = "Updated question".into();
    host.writes.clear();
    let mut fixed = request("fixed", Some(FindingStatus::Fixed), false);
    fixed.answers = answered.answers;
    fixed.answer_targets = answered.answer_targets;
    let result = publish(&fixed, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, 3);
    assert!(host.writes.iter().all(|write| !matches!(
        write,
        Write::Reply {
            action: ActionId {
                kind: ReplyKind::Answer(_),
                ..
            },
            ..
        }
    )));
}
