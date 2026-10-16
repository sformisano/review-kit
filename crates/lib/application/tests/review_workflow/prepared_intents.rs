//! A prepared write never reached the provider; once its retained intent can
//! no longer resume, it cannot block publication or abandonment.

use super::*;
use review_kit_application::use_cases::{
    publication_run::{ExecutionEvent, ExecutionEvidence},
    publishing::{AbandonRequest, WritePhase},
};

/// Interrupts publication after the answer's intent is prepared, before dispatch.
#[derive(Default)]
struct StopBeforeAnswer {
    discarded: Vec<Write>,
}

impl ExecutionEvidence for StopBeforeAnswer {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        match event {
            ExecutionEvent::BeforeWrite(Write::Reply { .. }) => Err(failure("interrupted")),
            ExecutionEvent::DiscardedUnsent(write) => {
                self.discarded.push(write.clone());
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Round 2 answers a question; its summary is verified and the answer only prepared.
fn prepared_answer() -> (Host, Source, Renderer, PublishRequest, Write) {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let req = answer_request("answer");
    let stopped = publish_recorded(
        &req,
        &mut host,
        &mut source,
        &mut renderer,
        &mut StopBeforeAnswer::default(),
    )
    .unwrap();
    assert!(matches!(stopped.stop, Some(Stop::Evidence(_))));
    let pending = host.progress.unresolved.clone().unwrap();
    assert_eq!(pending.phase, WritePhase::Prepared);
    assert!(matches!(pending.write, Write::Reply { .. }));
    assert_eq!(host.snapshot.summaries[0].current.number, 2);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    assert!(!effects(&host).contains(&Effect::Reply));
    host.writes.clear();
    (host, source, renderer, req, pending.write)
}

fn edit_question(host: &mut Host) {
    host.snapshot
        .replies
        .iter_mut()
        .find(|reply| reply.id == provider_id("question"))
        .unwrap()
        .body = "Why does this drop, and what retries it?".into();
}

fn abandonment() -> AbandonRequest {
    AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: 2,
        reason: "The question changed before the answer was sent.".into(),
        effects: Effects::new([Effect::UpdateSummary]),
    }
}

/// Journal fields other than the discarded intent.
fn retained(progress: &PublicationProgress) -> PublicationProgress {
    PublicationProgress {
        unresolved: None,
        ..progress.clone()
    }
}

#[test]
fn recovery_discards_a_prepared_write_that_can_no_longer_resume() {
    let (mut host, mut source, mut renderer, req, write) = prepared_answer();
    edit_question(&mut host);
    let before = host.progress.clone();
    let mut evidence = StopBeforeAnswer::default();
    let refused =
        publish_recorded(&req, &mut host, &mut source, &mut renderer, &mut evidence).unwrap_err();
    assert!(
        has_plan_error(&refused, &|error| matches!(
            error,
            PlanError::InvalidAnswer(_)
        )),
        "the current plan stops the stale answer: {refused:?}"
    );
    assert!(host.writes.is_empty(), "the stale answer is never sent");
    assert_eq!(evidence.discarded, [write]);
    assert_eq!(host.progress.unresolved, None);
    assert_eq!(
        host.progress,
        retained(&before),
        "every other field is kept"
    );

    let abandoned = abandon_observed(&abandonment(), &mut host, &mut renderer).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    assert_eq!(effects(&host), [Effect::UpdateSummary]);
}

#[test]
fn abandonment_discards_a_prepared_write_other_than_its_own_close() {
    let (mut host, _source, mut renderer, _req, write) = prepared_answer();
    edit_question(&mut host);
    let before = host.progress.clone();
    let mut evidence = StopBeforeAnswer::default();
    let abandoned =
        abandon_recorded(&abandonment(), &mut host, &mut renderer, &mut evidence).unwrap();
    assert_eq!(abandoned.status, PublicationStatus::Abandoned);
    assert_eq!(evidence.discarded, [write]);
    assert_eq!(effects(&host), [Effect::UpdateSummary]);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Abandoned
    );
    assert_eq!(retained(&host.progress), retained(&before));
}

#[test]
fn a_write_that_may_have_been_dispatched_is_never_discarded() {
    for phase in [WritePhase::DispatchPossible, WritePhase::LegacyAmbiguous] {
        let (mut host, mut source, mut renderer, req, _write) = prepared_answer();
        host.progress.unresolved.as_mut().unwrap().phase = phase;
        edit_question(&mut host);
        let before = host.progress.clone();
        let mut evidence = StopBeforeAnswer::default();
        let uncertain =
            publish_recorded(&req, &mut host, &mut source, &mut renderer, &mut evidence).unwrap();
        assert_eq!(uncertain.status, PublicationStatus::Uncertain, "{phase:?}");
        assert!(matches!(uncertain.stop, Some(Stop::UnresolvedWrite)));
        let refused =
            abandon_recorded(&abandonment(), &mut host, &mut renderer, &mut evidence).unwrap();
        assert_eq!(refused.status, PublicationStatus::Uncertain, "{phase:?}");
        assert!(evidence.discarded.is_empty(), "{phase:?}");
        assert_eq!(host.progress, before, "{phase:?}");
        assert!(host.writes.is_empty(), "{phase:?}");
    }
}

#[test]
fn a_prepared_write_that_can_still_resume_is_sent_exactly_once() {
    let (mut host, mut source, mut renderer, req, write) = prepared_answer();
    let resumed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(resumed.status, PublicationStatus::Completed);
    assert_eq!(host.writes.iter().filter(|sent| **sent == write).count(), 1);
}
