use super::*;
use operation_logs::{Capture, completed};
use review_kit_observation::with_observer;
use serde_json::Value;

fn started<'a>(records: &'a [Value], operation: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["operation"] == operation && record["phase"] == "started")
        .collect()
}

#[test]
fn managed_cache_calls_keep_keys_callback_counts_and_individual_operation_records() {
    use review_kit_application::use_cases::publishing::check_review;

    let mut request = request("cache", Some(FindingStatus::Open), false);
    let first = request.assessment.findings()[0].clone();
    let second = copy_finding(&first, FindingId::try_from(2).unwrap(), policy(), None);
    replace_findings(&mut request, vec![first, second]);
    request.allow_colocated = true;
    let (baseline_host, mut baseline_source, mut baseline_renderer) = fixtures();
    let expected = check_review(
        &request,
        &baseline_host.snapshot,
        &mut baseline_source,
        &mut baseline_renderer,
    )
    .unwrap();
    let baseline_calls = baseline_host.log.borrow().clone();
    let (host, mut source, mut renderer) = fixtures();

    // Each invocation owns its successful-value cache, while each logical port
    // request remains observable even when the raw adapter is not called again.
    for _ in 0..2 {
        host.log.borrow_mut().clear();
        let capture = Capture::default();
        let actual = with_observer(&capture.observer(), || {
            check_review(&request, &host.snapshot, &mut source, &mut renderer)
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(*host.log.borrow(), baseline_calls);
        for (call, count) in [("source", 1), ("position", 2), ("render:finding", 2)] {
            assert_eq!(
                host.log
                    .borrow()
                    .iter()
                    .filter(|entry| *entry == call)
                    .count(),
                count
            );
        }
        let records = capture.records();
        for operation in [
            "application.source.lines",
            "application.source.validate_position",
            "application.render.finding",
        ] {
            let calls = started(&records, operation);
            assert_eq!(calls.len(), 4, "duplicate or missing {operation}");
            for call in calls {
                let parent = records
                    .iter()
                    .find(|record| {
                        record["phase"] == "started"
                            && record["execution_id"] == call["parent_execution_id"]
                    })
                    .unwrap();
                assert_eq!(parent["operation"], "application.validation.findings");
            }
        }
        assert!(!capture.raw().contains("src/lib.rs"));
        assert!(
            !capture
                .raw()
                .contains("The failure path drops the request.")
        );
    }

    source.fail = true;
    host.log.borrow_mut().clear();
    let capture = Capture::default();
    let failed = with_observer(&capture.observer(), || {
        check_review(&request, &host.snapshot, &mut source, &mut renderer)
    });
    assert!(failed.is_err());
    assert_eq!(
        host.log
            .borrow()
            .iter()
            .filter(|entry| *entry == "source")
            .count(),
        2
    );
    let records = capture.records();
    assert_eq!(started(&records, "application.source.lines").len(), 2);
    assert!(
        records
            .iter()
            .filter(|record| record["operation"] == "application.source.lines"
                && record["phase"] == "completed")
            .all(|record| record["outcome"] == "failure")
    );
}

#[test]
fn public_discovery_binds_custom_reader_without_duplicate_operations() {
    struct Reader {
        snapshot: Snapshot,
        calls: usize,
    }
    impl ReviewReader for Reader {
        fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
            assert_eq!(&self.snapshot.subject, subject);
            self.calls += 1;
            Ok(self.snapshot.clone())
        }
    }

    let (host, _, _) = fixtures();
    let mut reader = Reader {
        snapshot: host.snapshot,
        calls: 0,
    };
    let capture = Capture::default();
    let snapshot = with_observer(&capture.observer(), || {
        review_kit_application::use_cases::discovery::discover(
            &mut reader,
            &subject(),
            &reviewer("agent"),
        )
    })
    .unwrap();
    assert_eq!(snapshot, reader.snapshot);
    assert_eq!(reader.calls, 1);
    let records = capture.records();
    let discovery = started(&records, "application.discovery");
    let port = started(&records, "application.provider.discover");
    let validation = started(&records, "application.validation.snapshot");
    assert_eq!(discovery.len(), 1);
    assert_eq!(port.len(), 1);
    assert_eq!(validation.len(), 1);
    assert!(discovery[0]["parent_execution_id"].is_null());
    assert_eq!(port[0]["parent_execution_id"], discovery[0]["execution_id"]);
    assert_eq!(
        validation[0]["parent_execution_id"],
        discovery[0]["execution_id"]
    );
    for operation in [
        "application.discovery",
        "application.provider.discover",
        "application.validation.snapshot",
    ] {
        assert_eq!(completed(&records, operation)["outcome"], "success");
        assert_eq!(
            records
                .iter()
                .filter(|record| record["operation"] == operation && record["phase"] == "completed")
                .count(),
            1
        );
    }
}

#[test]
fn borrowed_custom_session_calls_are_observed_once_with_readback_identity() {
    let (mut host, mut source, mut renderer) = fixtures();
    let request = request("initial", None, false);
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![Effect::CreateSummary, Effect::UpdateSummary]
    );
    let records = capture.records();
    let publish = started(&records, "application.publication.publish");
    let acquire = started(&records, "application.provider.acquire");
    assert_eq!(publish.len(), 1);
    assert_eq!(acquire.len(), 1);
    assert_eq!(
        acquire[0]["parent_execution_id"],
        publish[0]["execution_id"]
    );
    let authority = started(&records, "application.provider.authority");
    assert_eq!(authority.len(), host.authority_reads);
    let discoveries = started(&records, "application.provider.discover");
    let readbacks = started(&records, "application.provider.readback");
    let writes = started(&records, "application.provider.write");
    assert_eq!(discoveries.len() + readbacks.len(), host.discoveries);
    assert_eq!(writes.len(), host.writes.len());
    assert_eq!(readbacks.len(), writes.len());
    for write in writes {
        let apply_id = &write["parent_execution_id"];
        let apply = records
            .iter()
            .find(|record| {
                record["operation"] == "application.publication.apply"
                    && record["phase"] == "started"
                    && &record["execution_id"] == apply_id
            })
            .unwrap();
        assert_eq!(apply["parent_execution_id"], publish[0]["execution_id"]);
        assert_eq!(
            readbacks
                .iter()
                .filter(|record| &record["parent_execution_id"] == apply_id)
                .count(),
            1
        );
        assert!(records.iter().any(|record| record["operation"]
            == "application.publication.verify_effect"
            && record["phase"] == "completed"
            && &record["parent_execution_id"] == apply_id
            && record["output"]["matched"] == true));
    }
}

#[test]
fn generated_discovery_preserves_custom_reader_failure_and_safe_error_chain() {
    struct FailedReader;
    impl ReviewReader for FailedReader {
        fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
            Err(PortError::new(
                "private-reader-operation",
                io::Error::other("private-reader-cause"),
            ))
        }
    }
    let capture = Capture::default();
    let error = with_observer(&capture.observer(), || {
        review_kit_application::use_cases::discovery::discover(
            &mut FailedReader,
            &subject(),
            &reviewer("agent"),
        )
    })
    .unwrap_err();
    assert!(matches!(
        error,
        review_kit_application::use_cases::discovery::DiscoveryError::Read(_)
    ));
    let records = capture.records();
    assert_eq!(started(&records, "application.discovery").len(), 1);
    assert_eq!(started(&records, "application.provider.discover").len(), 1);
    assert!(started(&records, "application.validation.snapshot").is_empty());
    assert_eq!(
        completed(&records, "application.discovery")["outcome"],
        "failure"
    );
    assert_eq!(
        completed(&records, "application.provider.discover")["outcome"],
        "failure"
    );
    assert!(!capture.raw().contains("private-reader-operation"));
    assert!(!capture.raw().contains("private-reader-cause"));
}
