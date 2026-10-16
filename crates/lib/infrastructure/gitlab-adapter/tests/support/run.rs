use std::time::SystemTime;

use review_kit_application::{
    ports::PortError,
    publication::Effects,
    use_cases::{
        publication_run::{
            Clock, ExecutionEvent, ExecutionEvidence, RunMode, RunOutcome, RunPorts, run,
        },
        publishing::{PublicationProgress, PublicationStatus},
    },
};

use super::support::{Fixture, Renderer, request};

struct TestClock;
impl Clock for TestClock {
    fn now(&mut self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

#[derive(Default)]
struct Evidence {
    events: Vec<&'static str>,
}
impl ExecutionEvidence for Evidence {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        self.events.push(match event {
            ExecutionEvent::Started { .. } => "started",
            ExecutionEvent::Observed(_) => "observed",
            ExecutionEvent::Planned(_) => "planned",
            ExecutionEvent::BeforeWrite(_) => "before_write",
            ExecutionEvent::DiscardedUnsent(_) => "discarded_unsent",
            ExecutionEvent::Readback { .. } => "readback",
            ExecutionEvent::Finished(_) => "finished",
            ExecutionEvent::Progress { .. } => "progress",
        });
        Ok(())
    }
}

#[test]
fn application_run_exercises_plan_and_publish_with_real_http_readbacks() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(&client, vec![]);
    let mut reader = client.clone();
    let mut source = client.clone();
    let mut renderer = Renderer::default();
    let mut host = client.publication_host(Effects::review());
    let mut clock = TestClock;
    let mut evidence = Evidence::default();
    let mut progress = PublicationProgress::default();
    let planned = run(
        &req,
        RunMode::Plan,
        RunPorts {
            reader: &mut reader,
            host: &mut host,
            source: &mut source,
            renderer: &mut renderer,
            progress: &mut progress,
            clock: &mut clock,
            evidence: &mut evidence,
        },
    );
    assert!(matches!(planned.outcome, Ok(RunOutcome::Planned(_))));
    assert_eq!(
        evidence.events,
        ["started", "observed", "planned", "finished"]
    );
    assert!(fixture.state.lock().unwrap().writes().is_empty());
    evidence.events.clear();
    let published = run(
        &req,
        RunMode::Publish,
        RunPorts {
            reader: &mut reader,
            host: &mut host,
            source: &mut source,
            renderer: &mut renderer,
            progress: &mut progress,
            clock: &mut clock,
            evidence: &mut evidence,
        },
    );
    assert!(
        matches!(published.outcome,Ok(RunOutcome::Published(result)) if result.status==PublicationStatus::Completed)
    );
    assert_eq!(
        evidence
            .events
            .iter()
            .filter(|event| **event == "before_write")
            .count(),
        2
    );
    assert_eq!(
        evidence
            .events
            .iter()
            .filter(|event| **event == "readback")
            .count(),
        2
    );
    assert_eq!(evidence.events.last(), Some(&"finished"));
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 2);
}
