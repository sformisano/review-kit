use super::*;
use operation_logs::{Capture, completed};
use review_kit_observation::with_observer;

#[test]
fn run_composes_evidence_without_duplicate_callbacks_or_records() {
    for (mode, fail_write) in [
        (RunMode::Plan, false),
        (RunMode::Publish, false),
        (RunMode::Publish, true),
    ] {
        let request = request(Effects::review());
        let mut fixture = Fixture::new(&request);
        fixture.recorder.fail_write = fail_write;
        let capture = Capture::default();
        let report = with_observer(&capture.observer(), || run(&request, mode, fixture.ports()));
        assert!(report.outcome.is_ok());
        assert_eq!(fixture.clock.calls, 2);
        let records = capture.records();
        let stages: Vec<_> = records
            .iter()
            .filter(|record| {
                record["phase"] == "started" && record["operation"] == "application.evidence.record"
            })
            .map(|record| record["input"]["stage"].as_str().unwrap())
            .collect();
        let raw_stages: Vec<_> = fixture
            .recorder
            .log
            .borrow()
            .iter()
            .filter_map(|event| match event.as_str() {
                "started" | "observed" | "planned" | "readback" | "progress" | "finished" => {
                    Some(event.clone())
                }
                "before-write" => Some("before_write".to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(stages, raw_stages);
        assert_eq!(stages.len(), fixture.recorder.events.len());
        assert_eq!(stages.first(), Some(&"started"));
        assert_eq!(stages.last(), Some(&"finished"));
        let root = completed(&records, "application.publication.run");
        for stage in ["started", "finished"] {
            let record = records
                .iter()
                .find(|record| {
                    record["phase"] == "started"
                        && record["operation"] == "application.evidence.record"
                        && record["input"]["stage"] == stage
                })
                .unwrap();
            assert_eq!(record["parent_execution_id"], root["execution_id"]);
        }
    }
}

#[test]
fn final_evidence_failure_is_separate_from_completed_publication_in_json()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_finished = true;
    let capture = Capture::default();
    let report = with_observer(&capture.observer(), || {
        run(&request, RunMode::Publish, fixture.ports())
    });
    assert!(report.evidence_failure.is_some());
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        return Err("publication result missing".into());
    };
    assert_eq!(result.status, PublicationStatus::Completed);
    let records = capture.records();
    let outcome = completed(&records, "application.publication.run");
    assert_eq!(outcome["outcome"], "success");
    assert_eq!(outcome["output"]["status"], "completed");
    assert_eq!(outcome["output"]["final_evidence_failed"], true);
    assert!(outcome["output"]["final_evidence_error"].is_object());
    assert!(!capture.raw().contains("injected failure"));
    assert!(records.iter().any(
        |record| record["operation"] == "application.evidence.record"
            && record["outcome"] == "failure"
    ));
    Ok(())
}

#[test]
fn evidence_refusal_prevents_provider_attempt_and_preserves_structured_error()
-> Result<(), Box<dyn std::error::Error>> {
    let request = request(Effects::review());
    let mut fixture = Fixture::new(&request);
    fixture.recorder.fail_write = true;
    let capture = Capture::default();
    let report = with_observer(&capture.observer(), || {
        run(&request, RunMode::Publish, fixture.ports())
    });
    let Ok(RunOutcome::Published(result)) = report.outcome else {
        return Err("publication result missing".into());
    };
    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert!(result.attempted.is_empty());
    let records = capture.records();
    assert!(
        !records
            .iter()
            .any(|record| record["operation"] == "application.provider.write")
    );
    let outcome = completed(&records, "application.publication.run");
    assert_eq!(outcome["outcome"], "failure");
    assert!(outcome["output"]["stop"].is_object());
    assert!(outcome["output"]["finalization_error"].is_null());
    assert!(!capture.raw().contains("injected failure"));
    Ok(())
}
