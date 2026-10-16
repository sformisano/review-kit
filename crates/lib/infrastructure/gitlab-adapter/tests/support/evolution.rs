use super::support::PublicationProgress;
use review_kit_application::{
    ports::{CommittedSource, ReviewReader},
    publication::Effects,
    use_cases::{
        discovery_bundle::discover_bundle,
        publishing::{PublicationStatus, publish},
    },
};
use review_kit_domain::{Assessment, AssessmentDraft, FindingStatus, Revision, SourcePath};
use serde_json::json;

use super::support::{Fixture, Renderer, finding, request, revision, transition};

#[derive(Clone, Copy, Debug)]
enum RangeTracking {
    Retained,
    Traced,
}

fn push_with_added_prefix(fixture: &Fixture, tracking: Option<RangeTracking>) {
    let mut state = fixture.state.lock().unwrap();
    state.retain_revision();
    state.historical_head = Some(state.head.clone());
    state.head = "4".repeat(40);
    state
        .files
        .insert("src/lib.rs".into(), b"prefix\nsame\nadded\nlast\n".to_vec());
    state.diffs[0]["diff"] = "@@ -1,2 +1,4 @@\n+prefix\n same\n+added\n last\n".into();
    let head = state.head.clone();
    let root = &mut state.discussions[0]["notes"][0];
    root["position"]["head_sha"] = head.into();
    root["position"]["new_line"] = 3.into();
    if matches!(tracking, Some(RangeTracking::Traced)) {
        root["position"]["line_range"] = json!({
            "start": {
                "line_code": "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_1_2",
                "type": null, "old_line": 1, "new_line": 2
            },
            "end": {
                "line_code": "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_2_4",
                "type": null, "old_line": 2, "new_line": 4
            }
        });
    }
    for suggestion in root["suggestions"].as_array_mut().unwrap() {
        suggestion["from_line"] = (suggestion["from_line"].as_u64().unwrap() + 1).into();
        suggestion["to_line"] = (suggestion["to_line"].as_u64().unwrap() + 1).into();
    }
}

#[test]
fn tracked_findings_retain_original_roots_after_a_push_followup_and_restart() {
    for tracking in [
        None,
        Some(RangeTracking::Retained),
        Some(RangeTracking::Traced),
    ] {
        let mut progress = PublicationProgress::default();
        let fixture = Fixture::new();
        let client = fixture.connect();
        let range =
            tracking
                .is_some()
                .then_some((1, vec!["same", "added", "last"], vec!["replacement"]));
        let initial = request(
            &client,
            vec![finding(
                1,
                "src/lib.rs",
                2,
                FindingStatus::Open,
                range.clone(),
            )],
        );
        let mut host = client.publication_host(Effects::review());
        let first = publish(
            &initial,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(first.status, PublicationStatus::Completed);
        let original = first.final_snapshot.findings[0].clone();
        let original_range =
            fixture.state.lock().unwrap().discussions[0]["notes"][0]["position"]["line_range"]
                .clone();
        push_with_added_prefix(&fixture, tracking);
        {
            let state = fixture.state.lock().unwrap();
            let range = &state.discussions[0]["notes"][0]["position"]["line_range"];
            match tracking {
                Some(RangeTracking::Retained) => assert_eq!(range, &original_range),
                Some(RangeTracking::Traced) => {
                    assert!(range["start"]["type"].is_null());
                    assert!(range["end"]["type"].is_null());
                    assert_eq!(range["start"]["new_line"], 2);
                    assert_eq!(range["end"]["new_line"], 4);
                    assert_ne!(range, &original_range);
                }
                None => assert!(range.is_null()),
            }
        }

        let observed = client.clone().discover(client.subject()).unwrap();
        assert_eq!(
            observed.findings.as_slice(),
            std::slice::from_ref(&original)
        );
        assert_eq!(observed.revision.head(), "4".repeat(40));
        let path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
        assert_eq!(
            client
                .clone()
                .lines(client.subject(), &revision(), &path)
                .unwrap(),
            ["same", "added", "last"]
        );
        assert_eq!(
            client
                .clone()
                .lines(client.subject(), &observed.revision, &path)
                .unwrap(),
            ["prefix", "same", "added", "last"]
        );

        let mut fixed = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, range)],
        );
        let old = &fixed.assessment;
        fixed.assessment = Assessment::new(AssessmentDraft {
            subject: old.subject().clone(),
            reviewer: old.reviewer().clone(),
            revision: Revision::new("4".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap(),
            decision: old.decision(),
            depth: old.depth().into(),
            scope: old.scope().into(),
            limitation: old.limitation().map(str::to_owned),
            selected_policies: old.selected_policies().to_vec(),
            evaluations: old.evaluations().to_vec(),
            findings: old.findings().to_vec(),
        })
        .unwrap();
        transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
        let completed = publish(
            &fixed,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(completed.status, PublicationStatus::Completed);

        let restarted = fixture.connect();
        let bundle = discover_bundle(
            &mut restarted.clone(),
            &mut restarted.clone(),
            restarted.subject(),
            restarted.reviewer(),
        )
        .unwrap();
        let mut resolved_original = original;
        resolved_original.resolved = true;
        assert_eq!(bundle.snapshot().findings, [resolved_original]);
        assert_eq!(bundle.findings()[0].status(), FindingStatus::Fixed);
        assert_eq!(bundle.snapshot().summaries[0].history.len(), 1);
        assert_eq!(
            bundle.snapshot().summaries[0].history[0].assessment,
            initial.assessment
        );
        let writes = fixture.state.lock().unwrap().writes().len();
        let mut resumed = restarted.publication_host(Effects::review());
        let unchanged = publish(
            &fixed,
            &mut resumed,
            &mut restarted.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(unchanged.status, PublicationStatus::Unchanged);
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.writes().len(), writes);
        assert_eq!(state.discussions.len(), 1);
        if tracking.is_some() {
            let suggestion = &state.discussions[0]["notes"][0]["suggestions"][0];
            assert_eq!(suggestion["from_line"], 2);
            assert_eq!(suggestion["to_line"], 4);
            assert_eq!(suggestion["from_content"], "same\nadded\nlast\n");
            assert_eq!(suggestion["to_content"], "replacement\n");
        }
    }
}

#[test]
fn changed_tracked_anchor_or_suggestion_range_retains_findings_for_reassessment() {
    let mut progress = PublicationProgress::default();
    for (with_suggestion, changed_anchor) in [(false, true), (true, false), (true, true)] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let range =
            with_suggestion.then_some((1, vec!["same", "added", "last"], vec!["replacement"]));
        let initial = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, range)],
        );
        let mut host = client.publication_host(Effects::review());
        assert_eq!(
            publish(
                &initial,
                &mut host,
                &mut client.clone(),
                &mut Renderer::default(),
                &mut progress
            )
            .unwrap()
            .status,
            PublicationStatus::Completed
        );
        push_with_added_prefix(&fixture, with_suggestion.then_some(RangeTracking::Traced));
        {
            let mut state = fixture.state.lock().unwrap();
            if !changed_anchor {
                state.files.insert(
                    "src/lib.rs".into(),
                    b"prefix\nchanged\nadded\nlast\n".to_vec(),
                );
                state.diffs[0]["diff"] =
                    "@@ -1,2 +1,4 @@\n-same\n+prefix\n+changed\n+added\n last\n".into();
                state.discussions[0]["notes"][0]["position"]["line_range"]["start"] = json!({
                    "line_code": "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_2_2",
                    "type": "new", "new_line": 2
                });
            } else {
                state.files.insert(
                    "src/lib.rs".into(),
                    b"prefix\nsame\nchanged\nlast\n".to_vec(),
                );
                state.diffs[0]["diff"] =
                    "@@ -1,2 +1,4 @@\n+prefix\n same\n+changed\n last\n".into();
                if with_suggestion {
                    state.discussions[0]["notes"][0]["position"]["line_range"] = json!({
                        "start": {
                            "line_code": "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_2_3",
                            "type": "new", "new_line": 3
                        },
                        "end": {
                            "line_code": "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_2_4",
                            "type": "old", "old_line": 2, "new_line": 4
                        }
                    });
                }
            }
        }
        let writes = fixture.state.lock().unwrap().writes().len();
        let observed = client.clone().discover(client.subject()).unwrap();
        let (_, outdated) = client.discover_with_outdated_positions().unwrap();
        assert_eq!(outdated, [initial.assessment.findings()[0].id()]);
        assert_eq!(
            observed.findings[0].finding,
            initial.assessment.findings()[0]
        );
        assert_eq!(observed.findings[0].revision, revision());
        assert_eq!(
            fixture.connect().discover(client.subject()).unwrap(),
            observed
        );
        assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
        if with_suggestion && !changed_anchor {
            {
                let mut state = fixture.state.lock().unwrap();
                state
                    .files
                    .insert("src/lib.rs".into(), b"added\nlast\n".to_vec());
                state.diffs[0]["diff"] = "@@ -1,2 +1,2 @@\n-same\n+added\n last\n".into();
                state.discussions[0]["notes"][0]["position"]["new_line"] = 1.into();
            }
            let (snapshot, outdated) = client.discover_with_outdated_positions().unwrap();
            assert_eq!(snapshot.findings, observed.findings);
            assert_eq!(outdated, [initial.assessment.findings()[0].id()]);
        }
        if with_suggestion && changed_anchor {
            assert_outdated_history_conflicts(&fixture);
        }
    }
}

fn assert_outdated_history_conflicts(fixture: &Fixture) {
    let original = fixture.state.lock().unwrap().discussions.clone();
    for conflict in ["deleted", "duplicate", "marker", "body", "author"] {
        {
            let mut state = fixture.state.lock().unwrap();
            state.discussions = original.clone();
            match conflict {
                "deleted" => state.discussions.clear(),
                "duplicate" => {
                    let mut copy = state.discussions[0].clone();
                    copy["id"] = "duplicate-thread".into();
                    copy["notes"][0]["id"] = 9001.into();
                    state.discussions.push(copy);
                }
                "marker" => {
                    let body = state.discussions[0]["notes"][0]["body"].as_str().unwrap();
                    state.discussions[0]["notes"][0]["body"] =
                        body.replace("\"round\":1", "\"round\":2").into();
                }
                "body" => {
                    let body = state.discussions[0]["notes"][0]["body"].as_str().unwrap();
                    state.discussions[0]["notes"][0]["body"] = format!("Edited {body}").into();
                }
                "author" => state.discussions[0]["notes"][0]["author"]["id"] = 77.into(),
                _ => unreachable!(),
            }
        }
        let client = fixture.connect();
        let writes = fixture.state.lock().unwrap().writes().len();
        assert!(
            discover_bundle(
                &mut client.clone(),
                &mut client.clone(),
                client.subject(),
                client.reviewer()
            )
            .is_err(),
            "{conflict}"
        );
        assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
    }
}

#[test]
fn incompatible_tracked_positions_prevent_adoption() {
    let mut progress = PublicationProgress::default();
    for mismatch in ["unavailable_revision", "wrong_line", "wrong_comparison"] {
        let fixture = Fixture::new();
        let client = fixture.connect();
        let initial = request(
            &client,
            vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
        );
        let mut host = client.publication_host(Effects::review());
        assert_eq!(
            publish(
                &initial,
                &mut host,
                &mut client.clone(),
                &mut Renderer::default(),
                &mut progress
            )
            .unwrap()
            .status,
            PublicationStatus::Completed
        );
        push_with_added_prefix(&fixture, None);
        {
            let mut state = fixture.state.lock().unwrap();
            let position = &mut state.discussions[0]["notes"][0]["position"];
            match mismatch {
                "unavailable_revision" => position["head_sha"] = "5".repeat(40).into(),
                "wrong_line" => position["new_line"] = 4.into(),
                "wrong_comparison" => position["base_sha"] = "5".repeat(40).into(),
                _ => unreachable!(),
            }
        }
        let writes = fixture.state.lock().unwrap().writes().len();
        assert!(
            client.clone().discover(client.subject()).is_err(),
            "{mismatch}"
        );
        assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
    }
}

#[test]
fn altered_retained_or_traced_multiline_ranges_prevent_adoption() {
    let mut progress = PublicationProgress::default();
    for tracking in [RangeTracking::Retained, RangeTracking::Traced] {
        for mismatch in ["type", "coordinate", "line_code"] {
            let fixture = Fixture::new();
            let client = fixture.connect();
            let initial = request(
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
                    &initial,
                    &mut host,
                    &mut client.clone(),
                    &mut Renderer::default(),
                    &mut progress
                )
                .unwrap()
                .status,
                PublicationStatus::Completed,
            );
            push_with_added_prefix(&fixture, Some(tracking));
            {
                let mut state = fixture.state.lock().unwrap();
                let start =
                    &mut state.discussions[0]["notes"][0]["position"]["line_range"]["start"];
                match mismatch {
                    "type" => start["type"] = "new".into(),
                    "coordinate" => start["new_line"] = 12.into(),
                    "line_code" => {
                        start["line_code"] = "b24749917179fb5e3e613ed2a703fcdcc6cdf9da_1_12".into();
                    }
                    _ => unreachable!(),
                }
            }
            let writes = fixture.state.lock().unwrap().writes().len();
            assert!(
                client.clone().discover(client.subject()).is_err(),
                "{tracking:?}: {mismatch}"
            );
            assert!(
                fixture.connect().discover(client.subject()).is_err(),
                "{tracking:?}: {mismatch} after restart"
            );
            assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
        }
    }
}
