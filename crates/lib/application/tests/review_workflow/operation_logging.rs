use super::*;
use operation_logs::{Capture, completed};
use review_kit_observation::{Observer, with_observer};

#[path = "operation_logging_safety.rs"]
mod safety;

#[test]
fn library_publication_records_internal_work_with_nested_redacted_json() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut request = request("first", Some(FindingStatus::Open), false);
    request.local_context = "private-operation-log-sentinel".into();
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let records = capture.records();
    assert!(!records.is_empty());
    for operation in [
        "application.publication.publish",
        "application.provider.acquire",
        "application.discovery",
        "application.validation.snapshot",
        "application.publication.build_plan",
        "application.validation.publication_shape",
        "application.validation.findings",
        "application.source.lines",
        "application.source.validate_position",
        "application.render.finding",
        "application.render.summary",
        "application.evidence.record",
        "application.publication.apply",
        "application.provider.authority",
        "application.provider.write",
        "application.provider.readback",
        "application.publication.verify_effect",
    ] {
        assert!(
            records
                .iter()
                .any(|record| record["operation"] == operation),
            "missing {operation}"
        );
    }
    let starts: Vec<_> = records
        .iter()
        .filter(|record| record["phase"] == "started")
        .collect();
    let root = starts
        .iter()
        .find(|record| record["operation"] == "application.publication.publish")
        .unwrap();
    assert!(root["parent_execution_id"].is_null());
    assert!(root["input"]["assessment"]["subject"].is_object());
    assert_eq!(root["input"]["assessment"]["subject"]["host"], "[REDACTED]");
    assert!(root["input"]["effects"]["allowed"].is_array());
    assert!(root["input"]["allow_colocated"].is_boolean());
    assert!(root["input"]["assessment"]["findings"].is_u64());
    for start in &starts {
        if !start["parent_execution_id"].is_null() {
            assert!(
                starts
                    .iter()
                    .any(|parent| parent["execution_id"] == start["parent_execution_id"])
            );
        }
        let finish = records
            .iter()
            .find(|record| {
                record["phase"] == "completed" && record["execution_id"] == start["execution_id"]
            })
            .unwrap();
        assert_eq!(finish["parent_execution_id"], start["parent_execution_id"]);
        assert!(finish["duration_us"].is_u64());
    }
    let outcome = completed(&records, "application.publication.publish");
    assert_eq!(outcome["output"]["status"], "completed");
    assert_eq!(outcome["output"]["attempted"], result.attempted.len());
    assert_eq!(
        outcome["output"]["readback_verified"],
        result.verified.len()
    );
    for private in [
        "private-operation-log-sentinel",
        "group/repo",
        "src/lib.rs",
        "Lost request",
        "The failure path drops the request.",
    ] {
        assert!(!capture.raw().contains(private), "disclosed {private}");
    }
}

#[test]
fn unknown_update_is_logged_as_uncertain_until_matching_readback() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownCommitted(Effect::UpdateSummary));
    let request = request("first", None, false);
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(host.progress.unresolved.is_none());
    let records = capture.records();
    let uncertain = records
        .iter()
        .find(|record| {
            record["phase"] == "completed"
                && record["operation"] == "application.provider.write"
                && record["outcome"] == "uncertain"
        })
        .unwrap();
    assert_eq!(uncertain["output"]["effect_state"], "uncertain");
    let apply_id = &uncertain["parent_execution_id"];
    assert!(!apply_id.is_null());
    let write_start = records
        .iter()
        .find(|record| {
            record["phase"] == "started" && record["execution_id"] == uncertain["execution_id"]
        })
        .unwrap();
    assert_eq!(write_start["input"]["effect"], "update_summary");
    assert!(records.iter().any(|record| {
        record["phase"] == "completed"
            && record["operation"] == "application.publication.verify_effect"
            && &record["parent_execution_id"] == apply_id
            && record["output"]["matched"] == true
    }));
    let applied = records
        .iter()
        .find(|record| {
            record["phase"] == "completed"
                && record["operation"] == "application.publication.apply"
                && &record["execution_id"] == apply_id
        })
        .unwrap();
    assert_eq!(applied["outcome"], "success");
    assert_eq!(
        completed(&records, "application.publication.publish")["outcome"],
        "success"
    );
}

#[test]
fn reused_pending_summary_is_verified_without_an_attempted_provider_write() {
    use review_kit_application::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence};

    struct RefuseNewWrites;
    impl ExecutionEvidence for RefuseNewWrites {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if matches!(event, ExecutionEvent::BeforeWrite(_)) {
                Err(failure("evidence"))
            } else {
                Ok(())
            }
        }
    }

    let (mut host, mut source, mut renderer) = fixtures();
    let request = request("initial", None, false);
    host.fault = Some(Fault::Rejected(Effect::UpdateSummary));
    let interrupted = publish(&request, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(interrupted.status, PublicationStatus::Partial);
    assert!(matches!(
        interrupted.finalization_stop,
        Some(Stop::Rejected(_))
    ));
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    host.authority = Effects::review();
    host.writes.clear();

    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish_recorded(
            &request,
            &mut host,
            &mut source,
            &mut renderer,
            &mut RefuseNewWrites,
        )
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert_eq!(result.round, interrupted.round);
    assert!(matches!(result.finalization_stop, Some(Stop::Evidence(_))));
    assert!(result.attempted.is_empty());
    assert!(host.writes.is_empty());
    assert_eq!(result.verified.len(), 1);
    assert!(matches!(result.verified[0].write, Write::Summary { .. }));
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );

    let records = capture.records();
    let reused = records
        .iter()
        .find(|record| {
            record["phase"] == "completed"
                && record["operation"] == "application.publication.apply"
                && record["outcome"] == "success"
        })
        .unwrap();
    assert_eq!(reused["output"]["effect_state"], "readback_verified");
    assert!(records.iter().any(|record| {
        record["phase"] == "completed"
            && record["operation"] == "application.publication.verify_effect"
            && record["parent_execution_id"] == reused["execution_id"]
            && record["output"]["matched"] == true
    }));
    assert!(
        !records
            .iter()
            .any(|record| record["operation"] == "application.provider.write")
    );
    let published = completed(&records, "application.publication.publish");
    assert_eq!(published["output"]["attempted"], 0);
    assert_eq!(published["output"]["readback_verified"], 1);
}

#[test]
fn unknown_create_can_be_verified_and_unchanged_review_does_not_log_attempt() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownCommitted(Effect::Finding));
    let request = request("first", Some(FindingStatus::Open), false);
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let records = capture.records();
    assert!(
        records
            .iter()
            .any(|record| record["operation"] == "application.provider.write"
                && record["outcome"] == "uncertain")
    );
    assert_eq!(
        completed(&records, "application.publication.publish")["outcome"],
        "success"
    );
    let unchanged = Capture::default();
    let result = with_observer(&unchanged.observer(), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(
        !unchanged
            .records()
            .iter()
            .any(|record| record["operation"] == "application.provider.write")
    );
}

#[test]
fn failing_log_writer_preserves_publication_effects() {
    struct FailingWriter;
    impl io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("private-writer-error"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("private-flush-error"))
        }
    }
    let request = request("first", Some(FindingStatus::Open), false);
    let (mut baseline_host, mut baseline_source, mut baseline_renderer) = fixtures();
    let baseline = publish(
        &request,
        &mut baseline_host,
        &mut baseline_source,
        &mut baseline_renderer,
    )
    .unwrap();
    let (mut host, mut source, mut renderer) = fixtures();
    let result = with_observer(&Observer::json(FailingWriter), || {
        publish(&request, &mut host, &mut source, &mut renderer)
    })
    .unwrap();
    assert_eq!(result.status, baseline.status);
    assert_eq!(host.writes, baseline_host.writes);
    assert_eq!(host.snapshot, baseline_host.snapshot);
    assert_eq!(host.progress, baseline_host.progress);
}

#[test]
fn custom_port_error_labels_and_causes_are_not_disclosed() {
    struct SecretSource;
    impl CommittedSource for SecretSource {
        fn lines(
            &mut self,
            _: &Subject,
            _: &Revision,
            _: &SourcePath,
        ) -> Result<Vec<String>, PortError> {
            Err(PortError::new(
                "private-port-label",
                io::Error::other("private-cause-sentinel"),
            ))
        }
        fn validate_position(
            &mut self,
            _: &Subject,
            _: &Revision,
            _: &Finding,
        ) -> Result<(), PortError> {
            Ok(())
        }
    }
    let (mut host, _, mut renderer) = fixtures();
    let request = request("first", Some(FindingStatus::Open), false);
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish_with_progress(
            &request,
            &mut host,
            &mut SecretSource,
            &mut renderer,
            &mut PublicationProgress::default(),
        )
    });
    assert!(result.is_err());
    assert!(host.writes.is_empty());
    let records = capture.records();
    let failure = completed(&records, "application.source.lines");
    assert_eq!(failure["outcome"], "failure");
    assert!(failure["error"].is_object());
    assert!(!capture.raw().contains("private-port-label"));
    assert!(!capture.raw().contains("private-cause-sentinel"));
}
