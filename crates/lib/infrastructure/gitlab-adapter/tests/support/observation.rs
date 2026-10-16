use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use review_kit_application::{
    ports::{CommittedSource, PublicationHost, WriteOutcome},
    publication::{Effects, Write as ProviderWrite},
    use_cases::publishing::{PublicationStatus, plan, publish},
};
use review_kit_domain::FindingStatus;
use review_kit_gitlab_adapter::{RetainedSource, validate_plan_bodies};
use review_kit_observation::{Observer, with_observer};
use serde_json::Value;

use super::support::{
    Fault, Fixture, MR_PATH, PublicationProgress, Renderer, TOKEN, finding, request,
};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn records(&self) -> Vec<Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
            .collect()
    }
}

#[test]
fn direct_library_operations_emit_correlated_safe_json() {
    let fixture = Fixture::new();
    let log = Log::default();
    let observer = Observer::json(log.clone());
    with_observer(&observer, || {
        let client = fixture.connect();
        let finding = finding(1, "src/lib.rs", 2, FindingStatus::Open, None);
        let req = request(&client, vec![finding.clone()]);
        let mut plan = plan(
            &req,
            &mut client.clone(),
            &mut client.clone(),
            &mut Renderer::default(),
        )
        .unwrap();
        validate_plan_bodies(&plan).unwrap();
        plan.summary_body = "/close\nprivate-plan-body".into();
        assert!(validate_plan_bodies(&plan).is_err());
        let retained = client
            .capture_source(client.subject(), req.assessment.revision())
            .unwrap();
        let mut retained = RetainedSource::decode(&retained.encode().unwrap()).unwrap();
        assert!(
            !retained
                .lines(
                    client.subject(),
                    req.assessment.revision(),
                    finding.location().path()
                )
                .unwrap()
                .is_empty()
        );
        retained
            .validate_position(client.subject(), req.assessment.revision(), &finding)
            .unwrap();
        let mut host = client.publication_host(Effects::review());
        let result = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut PublicationProgress::default(),
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
    });
    let text = log.text();
    for secret in [
        TOKEN,
        "PRIVATE local agent context",
        "group/sub/repo",
        "src/lib.rs",
        "The failure path discards the request.",
        "Controlled regression",
        "discussion-1",
    ] {
        assert!(!text.contains(secret), "log contains {secret}");
    }
    let records = log.records();
    for operation in [
        "gitlab.connect",
        "gitlab.capture_source",
        "gitlab.retained_source.encode",
        "gitlab.retained_source.decode",
        "gitlab.retained_source.lines",
        "gitlab.retained_source.validate_position",
        "gitlab.acquire",
        "gitlab.authority",
        "gitlab.snapshot",
        "gitlab.read_diff",
        "gitlab.read_source",
        "gitlab.validate_position",
        "gitlab.verify_position",
        "gitlab.validate_plan_bodies",
        "gitlab.prepare_write",
        "gitlab.write",
        "gitlab.http.write",
        "gitlab.http.read",
    ] {
        assert!(
            records
                .iter()
                .any(|record| record["operation"] == operation && record["phase"] == "completed"),
            "missing {operation}"
        );
    }
    let starts: Vec<_> = records
        .iter()
        .filter(|record| record["phase"] == "started")
        .collect();
    for completed in records
        .iter()
        .filter(|record| record["phase"] == "completed")
    {
        assert!(completed["duration_us"].is_u64());
        assert!(
            starts
                .iter()
                .any(|start| start["execution_id"] == completed["execution_id"])
        );
        if !completed["parent_execution_id"].is_null() {
            assert!(
                starts
                    .iter()
                    .any(|start| start["execution_id"] == completed["parent_execution_id"])
            );
        }
    }
    let write = records
        .iter()
        .find(|record| record["operation"] == "gitlab.http.write" && record["phase"] == "completed")
        .unwrap();
    assert_eq!(write["output"]["effect"], "acknowledged");
    let attempted = starts
        .iter()
        .find(|start| start["execution_id"] == write["execution_id"])
        .unwrap();
    assert!(attempted["input"]["bytes"].is_u64());
    assert_eq!(attempted["input"]["effect"], "attempted");
    assert!(!write["parent_execution_id"].is_null());
    let authority = records
        .iter()
        .find(|record| record["operation"] == "gitlab.authority" && record["phase"] == "completed")
        .unwrap();
    assert!(authority["output"]["allowed"].is_array());
    let invalid_body = records
        .iter()
        .find(|record| {
            record["operation"] == "gitlab.validate_plan_bodies" && record["outcome"] == "failure"
        })
        .unwrap();
    assert_eq!(invalid_body["error"]["variant"], "unsafe_body");
    assert!(!text.contains("private-plan-body"));
}

#[test]
fn denied_writes_have_no_transport_attempt_and_lost_responses_remain_uncertain() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let denied_log = Log::default();
    with_observer(&Observer::json(denied_log.clone()), || {
        let mut host = client.publication_host(Effects::none());
        let mut session = host.acquire(client.subject(), client.reviewer()).unwrap();
        let outcome = session.write(
            client.subject(),
            client.reviewer(),
            &ProviderWrite::SetResolved {
                thread: review_kit_application::publication::ProviderId::new(
                    "private-discussion".into(),
                )
                .unwrap(),
                resolved: true,
            },
        );
        assert!(matches!(outcome, WriteOutcome::Rejected(_)));
    });
    let records = denied_log.records();
    assert!(
        !records
            .iter()
            .any(|record| record["operation"] == "gitlab.http.write")
    );
    let rejected = records
        .iter()
        .find(|record| record["operation"] == "gitlab.write" && record["phase"] == "completed")
        .unwrap();
    assert_eq!(rejected["outcome"], "rejected");
    assert_eq!(rejected["error"]["variant"], "denied");
    assert!(fixture.state.lock().unwrap().writes().is_empty());

    let fixture = Fixture::with_lost_responses();
    let client = fixture.connect();
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
    let log = Log::default();
    with_observer(&Observer::json(log.clone()), || {
        let req = request(&client, vec![]);
        let result = publish(
            &req,
            &mut client.publication_host(Effects::review()),
            &mut client.clone(),
            &mut Renderer::default(),
            &mut PublicationProgress::default(),
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Uncertain);
    });
    let records = log.records();
    let write = records
        .iter()
        .find(|record| record["operation"] == "gitlab.http.write" && record["phase"] == "completed")
        .unwrap();
    assert_eq!(write["outcome"], "uncertain");
    assert_eq!(write["output"]["effect"], "uncertain");
    assert_eq!(write["error"]["variant"], "uncertain");
    assert!(!log.text().contains(TOKEN));
}

#[test]
fn failed_log_writer_does_not_change_verified_publication() {
    struct FailedWriter;
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("private-writer-detail"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("private-flush-detail"))
        }
    }
    let fixture = Fixture::new();
    with_observer(&Observer::json(FailedWriter), || {
        let client = fixture.connect();
        let req = request(&client, vec![]);
        let result = publish(
            &req,
            &mut client.publication_host(Effects::review()),
            &mut client.clone(),
            &mut Renderer::default(),
            &mut PublicationProgress::default(),
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
        assert_eq!(result.verified.len(), 2);
    });
    assert_eq!(fixture.state.lock().unwrap().writes().len(), 2);
}
