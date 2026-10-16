use super::*;
use operation_logs::{Capture, completed};
use review_kit_observation::with_observer;

#[test]
fn custom_changes_and_draft_ports_are_observed_once_and_keep_typed_outcomes() {
    let (mut reader, mut changes) = ports(snapshot());
    let capture = Capture::default();
    let discovered = with_observer(&capture.observer(), || {
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer())
    })
    .unwrap();
    assert_eq!(reader.reads, 2);
    assert_eq!(changes.calls, 1);
    assert_eq!(discovered.changes(), &observed_changes());
    let records = capture.records();
    let root = completed(&records, "application.discovery_bundle");
    let changed = completed(&records, "application.source.changes");
    assert_eq!(changed["parent_execution_id"], root["execution_id"]);
    assert_eq!(changed["output"]["files"], 3);
    for (operation, count) in [
        ("application.source.changes", 1),
        ("application.provider.discover", 2),
    ] {
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "started" && record["operation"] == operation)
                .count(),
            count
        );
    }
    for private in ["group/repo", "old.rs", "diff --git"] {
        assert!(!capture.raw().contains(private));
    }

    let mut writer = Writer::default();
    for (outcome, label) in [
        (DraftCreation::Created, "created"),
        (DraftCreation::Existing, "existing"),
    ] {
        let capture = Capture::default();
        let result = with_observer(&capture.observer(), || {
            initialize(&discovered, policy_set(), &mut writer)
        })
        .unwrap();
        assert_eq!(result.outcome, outcome);
        let records = capture.records();
        let created = completed(&records, "application.draft.create");
        assert_eq!(created["output"]["destination"], label);
        assert_eq!(
            created["parent_execution_id"],
            completed(&records, "application.initialization")["execution_id"]
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "started"
                    && record["operation"] == "application.draft.create")
                .count(),
            1
        );
    }
    assert_eq!(writer.calls, 2);
    assert!(writer.saved.is_some());
}

#[test]
fn changes_failure_stops_before_later_ports() {
    let (mut reader, mut changes) = ports(snapshot());
    changes.fail = true;
    let capture = Capture::default();
    let result = with_observer(&capture.observer(), || {
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer())
    });
    assert!(matches!(result, Err(BundleError::Changes(_))));
    assert_eq!(reader.reads, 1);
    assert_eq!(changes.calls, 1);
    let records = capture.records();
    assert_eq!(
        completed(&records, "application.source.changes")["outcome"],
        "failure"
    );
    assert!(!capture.raw().contains("injected failure"));
}
