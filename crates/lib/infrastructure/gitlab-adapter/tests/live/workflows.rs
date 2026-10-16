use std::error::Error;

use review_kit_application::{
    ports::{CommittedSource, ReviewReader},
    publication::{Answer, Effects, ProviderId, RoundStatus},
    use_cases::{
        discovery_bundle::discover_bundle,
        publishing::{PlanError, PublicationProgress, PublicationStatus, Stop, publish},
    },
};
use review_kit_domain::{FindingId, FindingStatus, SourcePath};
use serde_json::json;

use super::{
    fixture::{Fixture, HEAD, wait_for_diff},
    model::{Renderer, finding, request, transition},
    service::GitLabService,
};

pub fn modified_binary_source_remains_explicit_in_retained_capture(service: &GitLabService) {
    let mut fixture = Fixture::new(service);
    let commits = format!("projects/{}/repository/commits", fixture.project);
    fixture.author.api.post(
        &commits,
        json!({"branch":"main","commit_message":"Base binary source","actions":[{
            "action":"create","file_path":"asset.bin","encoding":"base64","content":"AAEC"
        }]}),
    );
    let head = fixture.author.api.post(
        &commits,
        json!({"branch":"binary-review","start_branch":"main",
        "commit_message":"Modify binary and text source","actions":[
            {"action":"update","file_path":"asset.bin","encoding":"base64","content":"AAED"},
            {"action":"update","file_path":"src/lib.rs","content":HEAD}
        ]}),
    );
    let mr = fixture.author.api.post(
        &format!("projects/{}/merge_requests", fixture.project),
        json!({"source_branch":"binary-review","target_branch":"main",
            "title":"Binary retained source verification"}),
    );
    fixture.mr = mr["iid"].as_u64().unwrap();
    fixture.mr_url = mr["web_url"].as_str().unwrap().into();
    wait_for_diff(
        &fixture.author.api,
        fixture.project,
        fixture.mr,
        head["id"].as_str().unwrap(),
    );
    let client = fixture.connect();
    let revision = fixture.revision();
    let versions = fixture
        .admin
        .get(&format!("{}/versions", fixture.mr_path()));
    let version = versions
        .as_array()
        .unwrap()
        .iter()
        .find(|version| {
            version["head_commit_sha"] == revision.head()
                && version["base_commit_sha"] == revision.comparison()[0]
                && version["start_commit_sha"] == revision.comparison()[1]
        })
        .unwrap();
    let exact = fixture
        .admin
        .get(&format!("{}/versions/{}", fixture.mr_path(), version["id"]));
    let binary_diff = exact["diffs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["new_path"] == "asset.bin")
        .unwrap();
    assert_eq!(binary_diff["new_file"], false);
    assert_eq!(binary_diff["deleted_file"], false);
    assert_eq!(binary_diff["collapsed"], false);
    assert_eq!(binary_diff["too_large"], false);
    println!("Modified binary diff response: {binary_diff}");
    let captured = client
        .capture_source(client.subject(), &revision)
        .unwrap_or_else(|error| panic!("binary capture failed: {error}; exact diff: {exact}"));
    let bytes = captured.encode().unwrap();
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let binary = envelope["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|blob| blob["path"] == "asset.bin")
        .unwrap();
    assert_eq!(binary["source"]["kind"], "unsupported");
    let mut retained = review_kit_gitlab_adapter::RetainedSource::decode(&bytes).unwrap();
    let text_path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
    assert_eq!(
        retained
            .lines(client.subject(), &revision, &text_path)
            .unwrap(),
        HEAD.lines().collect::<Vec<_>>()
    );
    let text_finding = finding(&revision, 1, "src/lib.rs", FindingStatus::Open, None);
    retained
        .validate_position(client.subject(), &revision, &text_finding)
        .unwrap();
    let binary_path = SourcePath::try_from("asset.bin".to_owned()).unwrap();
    let binary_finding = finding(&revision, 2, "asset.bin", FindingStatus::Open, None);
    let errors = [
        retained
            .lines(client.subject(), &revision, &binary_path)
            .unwrap_err(),
        retained
            .validate_position(client.subject(), &revision, &binary_finding)
            .unwrap_err(),
        client
            .clone()
            .lines(client.subject(), &revision, &binary_path)
            .unwrap_err(),
        client
            .clone()
            .validate_position(client.subject(), &revision, &binary_finding)
            .unwrap_err(),
    ];
    for error in errors {
        assert!(matches!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<review_kit_gitlab_adapter::GitLabError>()),
            Some(review_kit_gitlab_adapter::GitLabError::UnsupportedSource)
        ));
    }
    fixture.cleanup();
}

pub fn initial_and_clean_reviews_survive_restart_and_unchanged_rerun(service: &GitLabService) {
    for with_finding in [false, true] {
        let fixture = Fixture::new(service);
        let client = fixture.connect();
        let revision = fixture.revision();
        let findings = if with_finding {
            vec![finding(
                &revision,
                1,
                "src/lib.rs",
                FindingStatus::Open,
                None,
            )]
        } else {
            vec![]
        };
        let req = request(&client, &revision, findings);
        let mut progress = PublicationProgress::default();
        let result = publish(
            &req,
            &mut client.publication_host(Effects::review()),
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(
            result.status,
            PublicationStatus::Completed,
            "stop: {:?}",
            result.stop
        );
        let notes = fixture.notes();
        let summary = notes
            .iter()
            .find(|n| {
                n["body"]
                    .as_str()
                    .unwrap()
                    .ends_with("<!-- review-kit:v1 {\"kind\":\"summary\"} -->")
            })
            .unwrap();
        assert_eq!(summary["author"]["id"], fixture.reviewer.id);
        assert!(
            !summary["body"]
                .as_str()
                .unwrap()
                .contains("PRIVATE fixture context")
        );
        if with_finding {
            let root = fixture.admin.get(&format!(
                "{}/discussions/{}",
                fixture.mr_path(),
                result.final_snapshot.findings[0].id.as_str()
            ));
            assert_eq!(root["notes"][0]["author"]["id"], fixture.reviewer.id);
            assert_eq!(root["notes"][0]["position"]["head_sha"], revision.head());
            assert_eq!(root["notes"][0]["position"]["new_line"], 3);
            assert_eq!(root["notes"][0]["position"]["new_path"], "src/lib.rs");
            assert!(
                root["notes"][0]["body"]
                    .as_str()
                    .unwrap()
                    .contains("F1: Finding 1")
            );
        }
        let restarted = fixture.connect();
        let bundle = discover_bundle(
            &mut restarted.clone(),
            &mut restarted.clone(),
            restarted.subject(),
            restarted.reviewer(),
        )
        .unwrap();
        assert_eq!(bundle.findings().len(), usize::from(with_finding));
        assert_eq!(
            bundle.snapshot().summaries[0].current.status,
            RoundStatus::Completed
        );
        assert_eq!(bundle.changes().files.len(), 4);
        assert!(bundle.changes().unified_diff.contains("+added"));
        let path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
        assert_eq!(
            restarted
                .clone()
                .lines(restarted.subject(), &revision, &path)
                .unwrap(),
            HEAD.lines().collect::<Vec<_>>()
        );
        let unchanged = publish(
            &req,
            &mut restarted.publication_host(Effects::review()),
            &mut restarted.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(unchanged.status, PublicationStatus::Unchanged);
        assert_eq!(unchanged.round, result.round);
        assert!(unchanged.attempted.is_empty());
        assert_eq!(fixture.notes(), notes);
        fixture.cleanup();
    }
}

pub fn single_multiline_and_deletion_suggestions_match_server_content(service: &GitLabService) {
    let fixture = Fixture::new(service);
    let client = fixture.connect();
    let revision = fixture.revision();
    let req = request(
        &client,
        &revision,
        vec![
            finding(
                &revision,
                1,
                "single.rs",
                FindingStatus::Open,
                Some((3, vec!["added"], vec!["replacement"])),
            ),
            finding(
                &revision,
                2,
                "multi.rs",
                FindingStatus::Open,
                Some((2, vec!["same", "added", "last"], vec!["one", "two"])),
            ),
            finding(
                &revision,
                3,
                "delete.rs",
                FindingStatus::Open,
                Some((3, vec!["added"], vec![])),
            ),
        ],
    );
    let result = publish(
        &req,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    )
    .unwrap();
    assert_eq!(
        result.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        result.stop
    );
    for root in &result.final_snapshot.findings {
        let raw = fixture.admin.get(&format!(
            "{}/discussions/{}",
            fixture.mr_path(),
            root.id.as_str()
        ));
        let suggestions = raw["notes"][0]["suggestions"].as_array().unwrap();
        assert_eq!(suggestions.len(), 1);
        let (from, to, first, last) = match root.finding.id().get() {
            1 => ("added\n", "replacement\n", 3, 3),
            2 => ("same\nadded\nlast\n", "one\ntwo\n", 2, 4),
            3 => ("added\n", "", 3, 3),
            _ => unreachable!(),
        };
        assert_eq!(suggestions[0]["from_content"], from);
        assert_eq!(suggestions[0]["to_content"], to);
        assert_eq!(suggestions[0]["from_line"], first);
        assert_eq!(suggestions[0]["to_line"], last);
    }
    let restarted = fixture.connect();
    assert_eq!(
        restarted
            .clone()
            .discover(restarted.subject())
            .unwrap()
            .findings,
        result.final_snapshot.findings
    );
    fixture.push_prefix();
    let current = fixture.revision();
    let tracked = fixture.connect().discover(client.subject()).unwrap();
    assert_eq!(tracked.findings, result.final_snapshot.findings);
    for root in &tracked.findings {
        let raw = fixture.admin.get(&format!(
            "{}/discussions/{}",
            fixture.mr_path(),
            root.id.as_str()
        ));
        let note = &raw["notes"][0];
        assert_eq!(note["position"]["head_sha"], current.head());
        assert_eq!(note["position"]["new_line"], 4);
        let suggestion = &note["suggestions"][0];
        let (start, end) = if root.finding.id().get() == 2 {
            (3, 5)
        } else {
            (4, 4)
        };
        assert_eq!(suggestion["from_line"], start);
        assert_eq!(suggestion["to_line"], end);
    }
    fixture.cleanup();
}

pub fn author_answer_followup_resolution_and_reopening_preserve_roots(service: &GitLabService) {
    let fixture = Fixture::new(service);
    let client = fixture.connect();
    let original_revision = fixture.revision();
    let mut progress = PublicationProgress::default();
    let open = finding(
        &original_revision,
        1,
        "src/lib.rs",
        FindingStatus::Open,
        None,
    );
    let initial = request(&client, &original_revision, vec![open.clone()]);
    let first = publish(
        &initial,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(
        first.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        first.stop
    );
    let root = &first.final_snapshot.findings[0];
    let question = fixture.author.api.post(
        &format!(
            "{}/discussions/{}/notes",
            fixture.mr_path(),
            root.id.as_str()
        ),
        json!({"body":"Why does this line need correction?"}),
    );
    let foreign = fixture.author.api.post(
        &format!("{}/notes", fixture.mr_path()),
        json!({"body":"Author-owned general comment."}),
    );
    let mut answered = initial.clone();
    answered.answer_targets = client.clone().discover(client.subject()).unwrap().replies;
    answered.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: ProviderId::new(question["id"].to_string()).unwrap(),
        body: "The added line discards required work.".into(),
    });
    let answer = publish(
        &answered,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(
        answer.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        answer.stop
    );
    let raw = fixture.admin.get(&format!(
        "{}/discussions/{}",
        fixture.mr_path(),
        root.id.as_str()
    ));
    assert!(raw["notes"].as_array().unwrap().iter().any(|n| {
        n["author"]["id"] == fixture.reviewer.id
            && n["body"]
                .as_str()
                .unwrap()
                .contains("Answer: The added line discards required work.")
    }));
    fixture.push_prefix();
    let current = fixture.revision();
    assert_ne!(current, original_revision);
    let snapshot = fixture.connect().discover(client.subject()).unwrap();
    assert_eq!(snapshot.findings[0].root_note, root.root_note);
    let mut fixed = request(
        &client,
        &current,
        vec![finding(
            &original_revision,
            1,
            "src/lib.rs",
            FindingStatus::Fixed,
            None,
        )],
    );
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    let resolved = publish(
        &fixed,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(
        resolved.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        resolved.stop
    );
    let raw = fixture.admin.get(&format!(
        "{}/discussions/{}",
        fixture.mr_path(),
        root.id.as_str()
    ));
    assert_eq!(raw["notes"][0]["resolved"], true);
    let mut reopened = request(&client, &current, vec![open]);
    transition(&mut reopened, 1, FindingStatus::Fixed, FindingStatus::Open);
    let result = publish(
        &reopened,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut progress,
    )
    .unwrap();
    assert_eq!(
        result.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        result.stop
    );
    let raw = fixture.admin.get(&format!(
        "{}/discussions/{}",
        fixture.mr_path(),
        root.id.as_str()
    ));
    assert_eq!(raw["notes"][0]["resolved"], false);
    assert_eq!(raw["notes"][0]["id"].to_string(), root.root_note.as_str());
    let restarted = fixture.connect();
    let bundle = discover_bundle(
        &mut restarted.clone(),
        &mut restarted.clone(),
        restarted.subject(),
        restarted.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings()[0].status(), FindingStatus::Open);
    assert_eq!(bundle.snapshot().summaries[0].history.len(), 3);
    assert_eq!(
        fixture
            .admin
            .get(&format!("{}/notes/{}", fixture.mr_path(), foreign["id"])),
        foreign
    );
    fixture.cleanup();
}

pub fn stale_revision_and_read_only_token_stop_without_provider_effects(service: &GitLabService) {
    let fixture = Fixture::new(service);
    let client = fixture.connect();
    let old = fixture.revision();
    let request = request(&client, &old, vec![]);
    let restricted = fixture.connect_as(&fixture.restricted);
    let result = publish(
        &request,
        &mut restricted.publication_host(Effects::review()),
        &mut restricted.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    )
    .unwrap();
    assert_ne!(result.status, PublicationStatus::Completed);
    assert!(matches!(result.stop, Some(Stop::Rejected(_))));
    assert!(result.verified.is_empty());
    assert!(
        !fixture
            .notes()
            .iter()
            .any(|n| n["author"]["id"] == fixture.reviewer.id)
    );
    fixture.push_prefix();
    // GitLab adds push system notes asynchronously.
    let user_notes = || {
        fixture
            .notes()
            .into_iter()
            .filter(|note| note["system"] == false)
            .collect::<Vec<_>>()
    };
    let before = user_notes();
    let result = publish(
        &request,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    );
    assert!(matches!(result, Err(PlanError::ChangedRevision)));
    assert_eq!(user_notes(), before);
    fixture.cleanup();
}

pub fn fork_source_and_paginated_notes_use_the_correct_project(service: &GitLabService) {
    let mut fixture = Fixture::new(service);
    fixture.fork();
    let client = fixture.connect();
    let revision = fixture.revision();
    let path = SourcePath::try_from("src/lib.rs".to_owned()).unwrap();
    assert_eq!(
        client
            .clone()
            .lines(client.subject(), &revision, &path)
            .unwrap(),
        ["first", "same", "fork-only", "last"]
    );
    for index in 0..101 {
        fixture.author.api.post(
            &format!("{}/notes", fixture.mr_path()),
            json!({"body":format!("Foreign note {index}")}),
        );
    }
    let req = request(
        &client,
        &revision,
        vec![finding(
            &revision,
            1,
            "src/lib.rs",
            FindingStatus::Open,
            None,
        )],
    );
    let result = publish(
        &req,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    )
    .unwrap();
    assert_eq!(
        result.status,
        PublicationStatus::Completed,
        "stop: {:?}",
        result.stop
    );
    for index in 101..202 {
        fixture.author.api.post(
            &format!("{}/notes", fixture.mr_path()),
            json!({"body":format!("Foreign note {index}")}),
        );
    }
    // Owned records lie beyond page one in both ascending and descending lists.
    for collection in ["notes", "discussions"] {
        let page = fixture.admin.get(&format!(
            "{}/{collection}?per_page=100&page=1",
            fixture.mr_path()
        ));
        assert_eq!(page.as_array().unwrap().len(), 100);
        assert!(!page.as_array().unwrap().iter().any(|item| {
            let note = if collection == "notes" {
                item
            } else {
                &item["notes"][0]
            };
            note["author"]["id"] == fixture.reviewer.id
        }));
    }
    let notes = fixture.notes();
    assert_eq!(
        notes
            .iter()
            .filter(|n| n["author"]["id"] == fixture.author.id && !n["system"].as_bool().unwrap())
            .count(),
        202
    );
    let restarted = fixture.connect();
    let bundle = discover_bundle(
        &mut restarted.clone(),
        &mut restarted.clone(),
        restarted.subject(),
        restarted.reviewer(),
    )
    .unwrap();
    assert_eq!(bundle.findings().len(), 1);
    assert_eq!(bundle.snapshot().summaries.len(), 1);
    assert_eq!(
        fixture
            .discussions()
            .iter()
            .filter(|d| d["notes"][0]["author"]["id"] == fixture.reviewer.id)
            .count(),
        2
    );
    fixture.cleanup();
}
