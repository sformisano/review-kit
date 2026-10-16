use super::*;
use review_kit_application::use_cases::{
    publication_run::{ExecutionEvent, ExecutionEvidence},
    publishing::AbandonRequest,
};

fn interrupted_review() -> (Host, Renderer, AbandonRequest) {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::Rejected(Effect::Finding));
    let result = publish(
        &request("initial", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    host.writes.clear();
    let request = AbandonRequest {
        subject: subject(),
        reviewer: reviewer("agent"),
        round: result.round,
        reason: "private-abandonment-reason".into(),
        effects: Effects::review(),
    };
    (host, renderer, request)
}

#[test]
fn observed_abandonment_updates_only_the_summary_and_reports_verified_success() {
    let (mut host, mut renderer, request) = interrupted_review();
    let retained_actions = host.snapshot.summaries[0].current.actions.clone();
    let retained_findings = host.snapshot.findings.clone();
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        abandon_observed(&request, &mut host, &mut renderer)
    })
    .unwrap();

    assert_eq!(result.status, PublicationStatus::Abandoned);
    assert_eq!(result.round, request.round);
    assert_eq!(result.skipped, retained_actions);
    assert!(result.stop.is_none());
    assert!(result.finalization_stop.is_none());
    assert_eq!(result.attempted.len(), 1);
    assert_eq!(result.verified.len(), 1);
    assert_eq!(effects(&host), vec![Effect::UpdateSummary]);
    assert_eq!(host.snapshot.findings, retained_findings);
    assert!(host.progress.unresolved.is_none());
    let round = &host.snapshot.summaries[0].current;
    assert_eq!(round.status, RoundStatus::Abandoned);
    assert_eq!(round.abandonment.as_ref().unwrap().reason, request.reason);

    let records = capture.records();
    let abandoned = completed(&records, "application.publication.abandon");
    assert!(abandoned["parent_execution_id"].is_null());
    assert_eq!(abandoned["outcome"], "success");
    assert_eq!(abandoned["output"]["status"], "abandoned");
    assert_eq!(abandoned["output"]["attempted"], 1);
    assert_eq!(abandoned["output"]["readback_verified"], 1);
    assert_eq!(
        records
            .iter()
            .filter(
                |record| record["operation"] == "application.publication.abandon"
                    && record["phase"] == "completed"
            )
            .count(),
        1
    );
    let acquired = completed(&records, "application.provider.acquire");
    assert_eq!(acquired["parent_execution_id"], abandoned["execution_id"]);
    let applied = completed(&records, "application.publication.apply");
    assert_eq!(applied["parent_execution_id"], abandoned["execution_id"]);
    assert_eq!(applied["outcome"], "success");
    assert_eq!(applied["output"]["effect_state"], "readback_verified");
    for operation in [
        "application.provider.write",
        "application.provider.readback",
    ] {
        let record = completed(&records, operation);
        assert_eq!(record["parent_execution_id"], applied["execution_id"]);
        assert_eq!(record["outcome"], "success");
    }
    assert!(!capture.raw().contains("private-abandonment-reason"));
}

#[test]
fn observed_abandonment_evidence_refusal_preserves_the_round_without_a_write() {
    struct RefuseWrite;
    impl ExecutionEvidence for RefuseWrite {
        fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
            if matches!(event, ExecutionEvent::BeforeWrite(_)) {
                Err(failure("evidence"))
            } else {
                Ok(())
            }
        }
    }

    let (mut host, mut renderer, request) = interrupted_review();
    let before = host.snapshot.clone();
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        abandon_recorded(&request, &mut host, &mut renderer, &mut RefuseWrite)
    })
    .unwrap();

    assert_eq!(result.status, PublicationStatus::NotStarted);
    assert!(matches!(result.stop, Some(Stop::Evidence(_))));
    assert!(result.attempted.is_empty());
    assert!(result.verified.is_empty());
    assert!(host.writes.is_empty());
    assert_eq!(host.snapshot, before);
    assert_eq!(
        host.progress.unresolved.as_ref().unwrap().phase,
        review_kit_application::use_cases::publishing::WritePhase::Prepared
    );

    let records = capture.records();
    let abandoned = completed(&records, "application.publication.abandon");
    assert_eq!(abandoned["outcome"], "failure");
    assert_eq!(abandoned["output"]["status"], "not_started");
    assert_eq!(abandoned["output"]["stop"]["variant"], "evidence");
    assert_eq!(abandoned["output"]["attempted"], 0);
    let applied = completed(&records, "application.publication.apply");
    assert_eq!(applied["parent_execution_id"], abandoned["execution_id"]);
    assert_eq!(applied["outcome"], "failure");
    let refusal = records
        .iter()
        .find(|record| {
            record["operation"] == "application.evidence.record"
                && record["phase"] == "completed"
                && record["outcome"] == "failure"
        })
        .unwrap();
    assert_eq!(refusal["parent_execution_id"], applied["execution_id"]);
    assert!(records.iter().any(|record| {
        record["execution_id"] == refusal["execution_id"]
            && record["phase"] == "started"
            && record["input"]["stage"] == "before_write"
    }));
    assert!(!records.iter().any(|record| {
        record["operation"] == "application.provider.write"
            || record["operation"] == "application.provider.readback"
    }));
}

#[test]
fn observed_authority_narrowing_keeps_the_finding_unpublished_and_reports_partial() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.narrow_on_third_authority = true;
    let retained_findings = host.snapshot.findings.clone();
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        publish(
            &request("narrowed", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer,
        )
    })
    .unwrap();

    assert_eq!(result.status, PublicationStatus::Partial);
    assert!(matches!(
        result.stop,
        Some(Stop::Authority(Effect::Finding))
    ));
    assert!(result.finalization_stop.is_none());
    assert_eq!(
        effects(&host),
        vec![Effect::CreateSummary, Effect::UpdateSummary]
    );
    assert_eq!(result.attempted.len(), 2);
    assert_eq!(result.verified.len(), 2);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(host.snapshot.findings, retained_findings);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Partial
    );
    assert!(host.progress.unresolved.is_none());

    let records = capture.records();
    let published = completed(&records, "application.publication.publish");
    assert_eq!(published["outcome"], "partial");
    assert_eq!(published["output"]["status"], "partial");
    assert_eq!(published["output"]["stop"]["variant"], "authority");
    assert_eq!(published["output"]["attempted"], 2);
    let authorities: Vec<_> = records
        .iter()
        .filter(|record| {
            record["operation"] == "application.provider.authority"
                && record["phase"] == "completed"
        })
        .collect();
    assert_eq!(authorities.len(), host.authority_reads);
    let narrowed = authorities
        .iter()
        .position(|record| {
            record["output"]["allowed"] == serde_json::json!(["create_summary", "update_summary"])
        })
        .unwrap();
    assert!(authorities[narrowed + 1..].iter().any(|record| {
        record["output"]["allowed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect == "create_finding")
    }));
    let denied = records
        .iter()
        .find(|record| {
            record["operation"] == "application.publication.apply"
                && record["phase"] == "completed"
                && record["error"]["variant"] == "authority"
        })
        .unwrap();
    assert_eq!(denied["parent_execution_id"], published["execution_id"]);
    assert_eq!(denied["outcome"], "rejected");
    assert!(records.iter().any(|record| {
        record["execution_id"] == denied["execution_id"]
            && record["phase"] == "started"
            && record["input"]["effect"] == "create_finding"
    }));
    assert!(!records.iter().any(|record| {
        record["operation"] == "application.provider.write"
            && record["parent_execution_id"] == denied["execution_id"]
    }));
}
