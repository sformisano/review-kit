use review_kit_application::use_cases::discovery_bundle::ReviewChanges;
use review_kit_gitlab_adapter::{GitLabError, RetainedSource};
use serde_json::{Value, json};

use super::support::{Fault, Fixture, MR_PATH, Response, revision};

#[test]
fn empty_versions_capture_complete_empty_source_and_changes() {
    for state in ["empty", "collected"] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        {
            let mut data = fixture.state.lock().unwrap();
            data.version_state = state.into();
            data.diffs.clear();
        }
        let changes = client
            .clone()
            .read_changes(client.subject(), &revision())
            .unwrap();
        assert!(changes.files.is_empty());
        assert_eq!(changes.unified_diff, "");
        let source = client
            .capture_source(client.subject(), &revision())
            .unwrap();
        let retained = RetainedSource::decode(&source.encode().unwrap()).unwrap();
        assert_eq!(retained.subject(), client.subject());
        assert_eq!(retained.revision(), &revision());
        let requests = fixture.state.lock().unwrap();
        assert!(requests.writes().is_empty());
        assert!(
            requests
                .requests
                .iter()
                .all(|request| !request.path.contains("/repository/"))
        );
    }
}

#[test]
fn empty_versions_reject_nonzero_list_counts_before_capture() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture.state.lock().unwrap().version_state = "empty".into();
    assert_eq!(
        client
            .capture_source(client.subject(), &revision())
            .unwrap_err(),
        GitLabError::Limit
    );
    assert!(fixture.state.lock().unwrap().writes().is_empty());
}

#[test]
fn empty_versions_reject_contradictory_detail_and_refs_during_capture() {
    let detail = json!({
        "id": 12, "merge_request_id": 55,
        "head_commit_sha": revision().head(),
        "base_commit_sha": revision().comparison()[0],
        "start_commit_sha": revision().comparison()[1],
        "state": "empty", "real_size": "0", "diffs": []
    });
    let cases = [
        ("real_size", json!("1"), GitLabError::Protocol),
        ("id", json!(13), GitLabError::Protocol),
        ("merge_request_id", json!(56), GitLabError::Protocol),
        (
            "head_commit_sha",
            json!("4".repeat(40)),
            GitLabError::Protocol,
        ),
        (
            "base_commit_sha",
            json!("4".repeat(40)),
            GitLabError::Protocol,
        ),
        (
            "start_commit_sha",
            json!("4".repeat(40)),
            GitLabError::Protocol,
        ),
        ("state", json!("pending"), GitLabError::Limit),
        ("state", json!("overflow"), GitLabError::Limit),
        ("state", json!("unknown"), GitLabError::Limit),
        ("diffs", Value::Null, GitLabError::Protocol),
        (
            "diffs",
            json!([{
                "old_path": "src/new.rs", "new_path": "src/new.rs",
                "a_mode": "0", "b_mode": "100644", "new_file": true,
                "renamed_file": false, "deleted_file": false,
                "collapsed": false, "too_large": false,
                "diff": "@@ -0,0 +1 @@\n+added\n"
            }]),
            GitLabError::Limit,
        ),
    ];
    for (field, value, expected) in cases {
        let fixture = Fixture::new();
        let client = fixture.connect();
        {
            let mut data = fixture.state.lock().unwrap();
            data.version_state = "empty".into();
            data.diffs.clear();
            let mut invalid = detail.clone();
            invalid[field] = value;
            data.faults.push(Fault::response(
                "GET",
                &format!("{MR_PATH}/versions/12"),
                Response::json(invalid),
            ));
        }
        assert_eq!(
            client
                .capture_source(client.subject(), &revision())
                .unwrap_err(),
            expected,
            "{field}"
        );
        assert!(fixture.state.lock().unwrap().writes().is_empty());
    }
}
