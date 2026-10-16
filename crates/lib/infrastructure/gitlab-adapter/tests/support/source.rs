use super::support::PublicationProgress;
use std::error::Error;

use review_kit_application::{
    ports::{CommittedSource, PortError},
    publication::Effects,
    use_cases::{
        assessment::CheckProblem,
        discovery_bundle::ReviewChanges,
        publishing::{PlanError, PublicationStatus, publish},
    },
};
use review_kit_domain::{Finding, FindingStatus, Revision, SourcePath, Subject};
use review_kit_gitlab_adapter::{GitLab, GitLabError, RetainedSource};
use serde_json::json;

use super::support::{Fault, Fixture, MR_PATH, Renderer, finding, request, revision};

#[test]
fn retained_committed_source_roundtrips_and_checks_positions_without_a_server() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    {
        let mut state = fixture.state.lock().unwrap();
        state.files.insert(
            "src/lib.rs".into(),
            "same\naddéd\r\nlast".as_bytes().to_vec(),
        );
        state.diffs[0]["diff"] = json!("@@ -1,2 +1,3 @@\n same\n+addéd\r\n last\n");
        state.files.insert("src/new.rs".into(), Vec::new());
        state.diffs[1]["diff"] = json!("");
    }
    let source = client
        .capture_source(client.subject(), &revision())
        .unwrap();
    let bytes = source.encode().unwrap();
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        envelope["subject"],
        json!({
            "provider": "gitlab", "host": client.subject().host(),
            "repository": client.subject().repository(), "request": client.subject().request()
        })
    );
    assert_eq!(
        envelope["revision"],
        json!({
            "head": revision().head(), "comparison": revision().comparison()
        })
    );
    let mut offline = RetainedSource::decode(&bytes).unwrap();
    let subject = client.subject().clone();
    assert_eq!(offline.subject(), &subject);
    assert_eq!(offline.revision(), &revision());
    assert!(fixture.state.lock().unwrap().writes().is_empty());
    drop(client);
    drop(fixture);

    let path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
    assert_eq!(
        offline.lines(&subject, &revision(), &path).unwrap(),
        ["same", "addéd\r", "last"]
    );
    let empty = SourcePath::try_from("src/new.rs".to_owned()).unwrap();
    assert!(
        offline
            .lines(&subject, &revision(), &empty)
            .unwrap()
            .is_empty()
    );
    let suggested = finding(
        1,
        "src/lib.rs",
        2,
        FindingStatus::Open,
        Some((1, vec!["same", "addéd\r", "last"], vec!["replacement"])),
    );
    offline
        .validate_position(&subject, &revision(), &suggested)
        .unwrap();
    let outside = finding(1, "src/lib.rs", 4, FindingStatus::Open, None);
    assert!(
        offline
            .validate_position(&subject, &revision(), &outside)
            .is_err()
    );
}

#[test]
fn retained_source_rejects_wrong_subject_and_changed_comparison() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let mut source = client
        .capture_source(client.subject(), &revision())
        .unwrap();
    let path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
    let other = review_kit_domain::Subject::new(
        "gitlab".into(),
        client.subject().host().into(),
        "group/other".into(),
        client.subject().request(),
    )
    .unwrap();
    let changed = review_kit_domain::Revision::new(
        revision().head().into(),
        vec!["4".repeat(40), "3".repeat(40)],
    )
    .unwrap();
    for error in [
        source.lines(&other, &revision(), &path).unwrap_err(),
        source.lines(client.subject(), &changed, &path).unwrap_err(),
    ] {
        assert!(matches!(
            error.source().unwrap().downcast_ref::<GitLabError>(),
            Some(GitLabError::Identity | GitLabError::RevisionChanged)
        ));
    }
    assert_eq!(
        client
            .capture_source(client.subject(), &changed)
            .unwrap_err(),
        GitLabError::RevisionChanged
    );
}

#[test]
fn unsupported_and_deleted_sources_remain_explicit_without_stopping_capture() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    {
        let mut state = fixture.state.lock().unwrap();
        state.diffs[0]["b_mode"] = json!("120000");
        state
            .files
            .insert("src/new.rs".into(), b"binary\0bytes".to_vec());
        state.diffs[1]["diff"] = json!("Binary files /dev/null and b/src/new.rs differ\n");
        state.diffs.push(json!({
            "old_path": "gone.rs", "new_path": "gone.rs", "a_mode": "100644", "b_mode": "0",
            "new_file": false, "renamed_file": false, "deleted_file": true,
            "collapsed": false, "too_large": false, "diff": "@@ -1 +0,0 @@\n-removed\n"
        }));
    }
    let source = client
        .capture_source(client.subject(), &revision())
        .unwrap();
    let bytes = source.encode().unwrap();
    let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["blobs"][0]["source"]["kind"], "unsupported");
    assert_eq!(data["blobs"][1]["source"]["kind"], "unsupported");
    assert_eq!(data["blobs"][2]["source"]["kind"], "deleted");
    let mut source = RetainedSource::decode(&bytes).unwrap();
    for path in ["src/lib.rs", "src/new.rs", "gone.rs", "missing.rs"] {
        let path = SourcePath::try_from(path.to_owned()).unwrap();
        let error = source
            .lines(client.subject(), &revision(), &path)
            .unwrap_err();
        assert_eq!(
            error.source().unwrap().downcast_ref::<GitLabError>(),
            Some(&GitLabError::UnsupportedSource)
        );
    }
    assert!(
        !fixture
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| {
                request.path.ends_with("/files/src/lib.rs/raw")
                    || request.path.ends_with("/files/gone.rs/raw")
            })
    );
}

#[test]
fn retained_source_decode_validates_complete_diff_blob_bindings() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let bytes = client
        .capture_source(client.subject(), &revision())
        .unwrap()
        .encode()
        .unwrap();
    let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let changes = [
        ("/version", json!(2)),
        ("/subject/provider", json!("github")),
        ("/subject/request", json!(0)),
        ("/subject/repository", json!("../project")),
        ("/revision/head", json!("invalid-sha")),
        ("/revision/comparison/0", json!("4".repeat(40))),
        ("/diff/real_size", json!("3")),
        ("/diff/diffs/0/collapsed", json!(true)),
        ("/blobs/0/path", json!("../outside.rs")),
        ("/blobs/0/source/text", json!("binary\0bytes")),
        ("/blobs/0/source", json!({"kind": "deleted"})),
        ("/blobs", json!([])),
    ];
    for (path, value) in changes {
        let mut invalid = data.clone();
        *invalid.pointer_mut(path).unwrap() = value;
        assert!(
            RetainedSource::decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{path}"
        );
    }
}

#[test]
fn capture_source_preserves_authentication_failure_as_a_failed_read() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "GET",
        "/projects/22/repository/files/src/lib.rs/raw",
        super::support::Response::status(401),
    ));
    assert_eq!(
        client
            .capture_source(client.subject(), &revision())
            .unwrap_err(),
        GitLabError::Authentication
    );
    assert!(fixture.state.lock().unwrap().writes().is_empty());
}

#[test]
fn live_source_preflight_retains_typed_failures_without_provider_writes() -> Result<(), PlanError> {
    struct Source<'a> {
        client: GitLab,
        fixture: &'a Fixture,
        advance: bool,
    }
    impl CommittedSource for Source<'_> {
        fn lines(
            &mut self,
            subject: &Subject,
            revision: &Revision,
            path: &SourcePath,
        ) -> Result<Vec<String>, PortError> {
            self.client.lines(subject, revision, path)
        }

        fn validate_position(
            &mut self,
            subject: &Subject,
            revision: &Revision,
            finding: &Finding,
        ) -> Result<(), PortError> {
            if self.advance {
                let mut state = self.fixture.state.lock().unwrap();
                state.historical_head = Some(state.head.clone());
                state.head = "4".repeat(40);
            }
            self.client.validate_position(subject, revision, finding)
        }
    }

    for advance in [false, true] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let expected = if advance {
            GitLabError::RevisionChanged
        } else {
            fixture.state.lock().unwrap().faults.push(Fault::response(
                "GET",
                "/projects/22/repository/files/src/lib.rs/raw",
                super::support::Response::status(503),
            ));
            GitLabError::Rejected(503)
        };
        let req = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
        );
        let mut source = Source {
            client: client.clone(),
            fixture: &fixture,
            advance,
        };
        let mut host = client.publication_host(Effects::review());
        let mut progress = PublicationProgress::default();
        let error = publish(
            &req,
            &mut host,
            &mut source,
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap_err();
        let PlanError::Checks(issues) = error else {
            return Err(error);
        };
        assert!(
            issues.iter().any(|issue| match &issue.problem {
                CheckProblem::Source(error) | CheckProblem::Position(error) => {
                    error
                        .source()
                        .and_then(|cause| cause.downcast_ref::<GitLabError>())
                        == Some(&expected)
                }
                _ => false,
            }),
            "missing typed source cause {expected:?}"
        );
        assert!(fixture.state.lock().unwrap().writes().is_empty());
        assert!(progress.unresolved.is_none());
    }
    Ok(())
}

#[test]
fn exact_renamed_context_added_and_suggestion_positions_are_sent_to_gitlab() {
    let mut progress = PublicationProgress::default();
    for line in [2, 3] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let req = request(
            &client,
            vec![finding(1, "src/lib.rs", line, FindingStatus::Open, None)],
        );
        let mut host = client.publication_host(Effects::review());
        let result = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(
            result.status,
            PublicationStatus::Completed,
            "line {line}; stop {:?}; finalization {:?}",
            result.stop,
            result.finalization_stop
        );
        let state = fixture.state.lock().unwrap();
        let sent = state
            .writes()
            .into_iter()
            .find(|request| request.path == format!("{MR_PATH}/discussions"))
            .unwrap();
        let mut expected = json!({"base_sha":"2".repeat(40),"start_sha":"3".repeat(40),"head_sha":"1".repeat(40),"position_type":"text","old_path":"src/old.rs","new_path":"src/lib.rs","new_line":line});
        if line == 3 {
            expected["old_line"] = 2.into();
        }
        assert_eq!(sent.payload["position"], expected);
        assert_eq!(sent.payload.as_object().unwrap().len(), 2);
        assert!(
            state
                .requests
                .iter()
                .any(|request| request.path == "/projects/22/repository/tree"
                    && request.query["ref"] == "1".repeat(40))
        );
        assert!(state.requests.iter().any(|request| {
            request
                .target
                .contains("projects/22/repository/files/src%2Flib.rs/raw")
        }));
        assert!(
            !state
                .requests
                .iter()
                .any(|request| request.path.starts_with("/projects/11/repository/"))
        );
    }
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(
            1,
            "src/lib.rs",
            2,
            FindingStatus::Open,
            Some((1, vec!["same", "added", "last"], vec!["replacement"])),
        )],
    );
    let mut host = client.publication_host(Effects::review());
    assert_eq!(
        publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .unwrap()
        .status,
        PublicationStatus::Completed
    );
    let state = fixture.state.lock().unwrap();
    let sent = state
        .writes()
        .into_iter()
        .find(|request| request.path == format!("{MR_PATH}/discussions"))
        .unwrap();
    assert_eq!(
        sent.payload["position"]["line_range"],
        json!({
            "start":{"line_code":"b24749917179fb5e3e613ed2a703fcdcc6cdf9da_1_1","type":"old","old_line":1,"new_line":1},
            "end":{"line_code":"b24749917179fb5e3e613ed2a703fcdcc6cdf9da_2_3","type":"old","old_line":2,"new_line":3}
        })
    );
    assert!(
        sent.payload["body"]
            .as_str()
            .unwrap()
            .contains("```suggestion:-1+1\nreplacement\n```\n\n<!-- review-kit:v1")
    );
}

#[test]
fn unsupported_positions_and_nonregular_or_binary_source_prevent_publication() {
    let mut progress = PublicationProgress::default();
    for mode in ["120000", "160000"] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        fixture
            .state
            .lock()
            .unwrap()
            .modes
            .insert("src/lib.rs".into(), mode.into());
        let error = client
            .clone()
            .lines(
                client.subject(),
                &revision(),
                &SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
            )
            .unwrap_err();
        assert_eq!(
            error.source().unwrap().downcast_ref::<GitLabError>(),
            Some(&GitLabError::UnsupportedSource)
        );
        assert!(fixture.state.lock().unwrap().writes().is_empty());
    }
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture.state.lock().unwrap().files.insert(
        "src/lib.rs".into(),
        b"same\nadded\nlast\noutside\n".to_vec(),
    );
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 4, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    assert!(
        publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress
        )
        .is_err()
    );
    assert!(fixture.state.lock().unwrap().writes().is_empty());
    for bytes in [vec![0xff], b"bad\0text".to_vec()] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        fixture
            .state
            .lock()
            .unwrap()
            .files
            .insert("src/lib.rs".into(), bytes);
        assert!(
            client
                .clone()
                .lines(
                    client.subject(),
                    &revision(),
                    &SourcePath::try_from("src/lib.rs".to_owned()).unwrap()
                )
                .is_err()
        );
    }
}

#[test]
fn unavailable_oversized_or_missing_diff_versions_are_explicit_failures() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let client = fixture.connect();
        {
            let mut state = fixture.state.lock().unwrap();
            match case {
                0 => state.version_state = "overflow".into(),
                1 => state.diffs[0]["collapsed"] = true.into(),
                2 => state.diffs[0]["too_large"] = true.into(),
                3 => state.faults.push(Fault::response(
                    "GET",
                    &format!("{MR_PATH}/versions"),
                    super::support::Response::json(json!([])),
                )),
                _ => unreachable!(),
            }
        }
        assert!(
            client
                .clone()
                .read_changes(client.subject(), &revision())
                .is_err()
        );
        assert!(fixture.state.lock().unwrap().writes().is_empty());
    }
}

#[test]
fn revision_change_after_the_pending_summary_stops_inline_effects() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::drift(&format!("{MR_PATH}/notes")));
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    let mut host = client.publication_host(Effects::review());
    let result = publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Superseded);
    assert!(fixture.state.lock().unwrap().discussions.is_empty());
}

#[test]
fn committed_blob_lines_preserve_carriage_returns_and_read_immutable_fork_refs() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    fixture
        .state
        .lock()
        .unwrap()
        .files
        .insert("src/lib.rs".into(), b"first\r\n\nlast\r".to_vec());
    assert_eq!(
        client
            .clone()
            .lines(
                client.subject(),
                &revision(),
                &SourcePath::try_from("src/lib.rs".to_owned()).unwrap()
            )
            .unwrap(),
        ["first\r", "", "last\r"]
    );
}

#[test]
fn suggestion_readbacks_preserve_terminal_lf_crlf_unterminated_and_deletion_content() {
    let mut progress = PublicationProgress::default();
    for (ending, replacement) in [
        ("\n", vec!["corrected"]),
        ("\r\n", vec!["corrected"]),
        ("", vec!["corrected"]),
        ("\n", vec![]),
    ] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let last = if ending == "\r\n" { "last\r" } else { "last" };
        {
            let mut state = fixture.state.lock().unwrap();
            state.files.insert(
                "src/lib.rs".into(),
                format!("same\nadded\nlast{ending}").into_bytes(),
            );
            let no_newline = if ending.is_empty() {
                "\\ No newline at end of file\n"
            } else {
                ""
            };
            state.diffs[0]["diff"] =
                format!("@@ -1,2 +1,3 @@\n same\n+added\n {last}\n{no_newline}").into();
        }
        let req = request(
            &client,
            vec![finding(
                1,
                "src/lib.rs",
                3,
                FindingStatus::Open,
                Some((3, vec![last], replacement.clone())),
            )],
        );
        let mut host = client.publication_host(Effects::review());
        let result = publish(
            &req,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(
            result.status,
            PublicationStatus::Completed,
            "ending {ending:?}; replacement {replacement:?}; stop {:?}; finalization {:?}",
            result.stop,
            result.finalization_stop
        );
        let state = fixture.state.lock().unwrap();
        let observed = &state.discussions[0]["notes"][0]["suggestions"][0];
        assert_eq!(observed["from_content"], format!("last{ending}"));
        assert_eq!(
            observed["to_content"],
            if replacement.is_empty() {
                String::new()
            } else {
                format!("corrected{ending}")
            }
        );
    }
}

#[test]
fn replacement_source_with_backticks_uses_a_safe_suggestion_fence_and_round_trips() {
    let mut progress = PublicationProgress::default();
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(
            1,
            "src/lib.rs",
            2,
            FindingStatus::Open,
            Some((2, vec!["added"], vec!["```", "replacement"])),
        )],
    );
    let mut host = client.publication_host(Effects::review());
    let result = publish(
        &req,
        &mut host,
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let state = fixture.state.lock().unwrap();
    let body = state.discussions[0]["notes"][0]["body"].as_str().unwrap();
    assert!(body.contains("````suggestion:-0+0\n```\nreplacement\n````\n\n<!-- review-kit:v1"));
    assert_eq!(
        state.discussions[0]["notes"][0]["suggestions"][0]["to_content"],
        "```\nreplacement\n"
    );
}
