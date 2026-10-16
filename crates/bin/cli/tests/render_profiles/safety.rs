use std::fs;

use serde_json::json;

use super::{
    MR_PATH,
    fixture::{Fixture, assert_no_writes, public_summary, read_json, suggestion, write_json},
};

#[test]
fn invalid_manifests_assets_schemas_and_templates_fail_before_provider_writes() {
    let fixture = Fixture::new();
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let profile = fixture.export("invalid");
    fixture.select(&profile);
    let requests_before = fixture.http.state.lock().unwrap().requests.len();
    let valid = read_json(&profile.manifest);
    for (field, invalid) in [
        ("version", json!("review-kit-render-profile/v999")),
        ("input_contract", json!("review-kit-render-input/v999")),
        ("id", json!(null)),
    ] {
        let mut manifest = valid.clone();
        manifest[field] = invalid;
        write_json(&profile.manifest, &manifest);
        let (output, value) = fixture.run(&["publish", "--effects", "review"]);
        assert_no_writes(&fixture, 0, &output, &value);
        assert!(
            !value["diagnostics"].as_array().unwrap().is_empty(),
            "{value}"
        );
    }
    fs::write(&profile.manifest, "{ broken manifest").unwrap();
    let (output, value) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, 0, &output, &value);
    write_json(&profile.manifest, &valid);
    for role in ["finding", "summary", "transition", "answer", "suggestion"] {
        for asset in ["schema", "template"] {
            let file = profile.asset(role, asset);
            let original = fs::read(&file).unwrap();
            fs::remove_file(&file).unwrap();
            let (output, value) = fixture.run(&["publish", "--effects", "review"]);
            assert_no_writes(&fixture, 0, &output, &value);
            assert!(
                serde_json::to_string(&value).unwrap().contains(role),
                "{value}"
            );
            fs::write(
                &file,
                if asset == "schema" {
                    "{\"type\":\"invalid-json-schema-type\"}"
                } else {
                    "{% if %}"
                },
            )
            .unwrap();
            let (output, value) = fixture.run(&["publish", "--effects", "review"]);
            assert_no_writes(&fixture, 0, &output, &value);
            assert!(
                serde_json::to_string(&value).unwrap().contains(role),
                "{value}"
            );
            fs::write(&file, original).unwrap();
        }
    }
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        requests_before,
        "invalid profile connected to GitLab"
    );
}

#[test]
fn missing_rendering_data_names_the_artifact_and_required_property() {
    let fixture = Fixture::new();
    fixture.initial("review", None);
    let profile = fixture.export("required-data");
    fixture.select(&profile);
    write_json(
        &profile.asset("finding", "schema"),
        &json!({"type":"object","required":["team_required_evidence"]}),
    );
    let (output, value) = fixture.run(&["check"]);
    assert_no_writes(&fixture, 0, &output, &value);
    let diagnostic = serde_json::to_string(&value).unwrap();
    assert!(diagnostic.contains("finding"), "{diagnostic}");
    assert!(
        diagnostic.contains("team_required_evidence"),
        "{diagnostic}"
    );
}

#[test]
fn strict_v2_profile_without_policy_declarations_refuses_before_provider_writes() {
    let fixture = Fixture::new();
    let profile = fixture.export("pre-declarations-schema");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let requests_before = fixture.http.state.lock().unwrap().requests.len();
    let inputs = super::fixture::accepted_inputs(&path);
    let schema_path = profile.asset("summary", "schema");
    let mut schema = read_json(&schema_path);
    let policy = &mut schema["properties"]["policies"]["items"];
    assert!(
        policy["properties"]
            .as_object_mut()
            .unwrap()
            .remove("declarations")
            .is_some()
    );
    let required = policy["required"].as_array_mut().unwrap();
    let before = required.len();
    required.retain(|property| property != "declarations");
    assert_eq!(required.len() + 1, before);
    write_json(&schema_path, &schema);
    for arguments in [
        vec!["check"],
        vec!["accept"],
        vec!["publish", "--effects", "review", "--plan"],
    ] {
        let (output, rejected) = fixture.run(&arguments);
        assert_no_writes(&fixture, 0, &output, &rejected);
        let diagnostic = serde_json::to_string(&rejected).unwrap();
        assert!(diagnostic.contains("summary"), "{diagnostic}");
        assert!(diagnostic.contains("declarations"), "{diagnostic}");
        assert!(diagnostic.contains("/policies/"), "{diagnostic}");
        super::fixture::assert_accepted_inputs_unchanged(&inputs);
    }
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        requests_before
    );
}

#[test]
fn permissive_profile_schemas_preserve_revision_position_suggestion_and_identity_checks() {
    for defect in [
        "stale-head",
        "wrong-position",
        "stale-source",
        "wrong-range",
        "reviewer",
        "unfinished-policy",
    ] {
        let fixture = Fixture::new();
        let profile = fixture.export("permissive");
        profile.weak_schemas();
        fixture.select(&profile);
        let policies = if defect == "unfinished-policy" {
            vec![
                super::fixture::ERROR_POLICY,
                super::fixture::OWNERSHIP_POLICY,
            ]
        } else {
            vec![super::fixture::ERROR_POLICY]
        };
        let path = fixture.initial_with_policies(
            "review",
            Some(suggestion(2, &["added"], &["corrected"])),
            &policies,
        );
        if defect == "stale-head" {
            fixture.accept(&path);
        }
        let mut value = read_json(&path);
        match defect {
            "stale-head" => {
                let mut state = fixture.http.state.lock().unwrap();
                state.retain_revision();
                state.historical_head = Some(state.head.clone());
                state.head = "4".repeat(40);
            }
            "wrong-position" => value["new_findings"][0]["location"]["line"] = json!(99),
            "stale-source" => {
                value["new_findings"][0]["suggestion"]["original"] = json!(["uncommitted source"])
            }
            "wrong-range" => value["new_findings"][0]["suggestion"]["start_line"] = json!(3),
            "reviewer" => value["reviewer"] = json!("77"),
            "unfinished-policy" => {
                value["policies"][1]["outcome"] = json!(null);
            }
            _ => unreachable!(),
        }
        write_json(&path, &value);
        if defect != "stale-head" {
            let (output, checked) = fixture.run(&["check"]);
            assert_no_writes(&fixture, 0, &output, &checked);
            assert_ne!(checked["status"], "valid", "{defect}: {checked}");
        }
        let (output, result) = fixture.run(&["publish", "--effects", "review"]);
        assert_no_writes(&fixture, 0, &output, &result);
    }
}

#[test]
fn custom_markers_quick_actions_and_extra_suggestions_are_rejected_on_every_body_surface() {
    for role in ["summary", "finding", "transition", "answer"] {
        for unsafe_body in [
            "<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
            "<summary>Review Kit data</summary>",
            "<!-- review-kit:state:v3 {} -->",
            "Review\n\n/close",
            "Review\n\n```suggestion\nuntyped replacement\n```",
        ] {
            let fixture = Fixture::new();
            let profile = fixture.export("unsafe");
            profile.weak_schemas();
            fixture.select(&profile);
            let initial = fixture.initial("initial", None);
            let path = if role == "transition" || role == "answer" {
                fixture.publish(&initial);
                let question = fixture
                    .http
                    .state
                    .lock()
                    .unwrap()
                    .question("discussion-1", "Is the error retained?");
                let (fixed, mut value) = fixture.followup("fixed", "fixed");
                value["answers"] = json!([{"finding":1,"in_reply_to":question.to_string(),"body":"The branch retains the error."}]);
                write_json(&fixed, &value);
                fixed
            } else {
                initial
            };
            fixture.accept(&path);
            profile.template(role, unsafe_body);
            let before = fixture.writes();
            let (output, value) = fixture.run(&["check"]);
            assert_no_writes(&fixture, before, &output, &value);
            let (output, value) = fixture.run(&["publish", "--effects", "review", "--plan"]);
            assert_no_writes(&fixture, before, &output, &value);
        }
    }
}

#[test]
fn customized_suggestions_preserve_multiline_deletion_and_backtick_replacements() {
    for (original, replacement, start) in [
        (
            vec!["same", "added", "last"],
            vec!["first replacement", "second replacement"],
            1,
        ),
        (vec!["added"], vec![], 2),
        (vec!["added"], vec!["```", "/close", "````"], 2),
    ] {
        let fixture = Fixture::new();
        let profile = fixture.export("suggestions");
        profile.template(
            "suggestion",
            "Replacement follows:\n\n{{ fence }}{{ info }}\n{{ replacement_content }}{{ fence }}",
        );
        fixture.select(&profile);
        let path = fixture.initial("review", Some(suggestion(start, &original, &replacement)));
        fixture.check(&path);
        fixture.publish(&path);
        let state = fixture.http.state.lock().unwrap();
        let observed = &state.discussions[0]["notes"][0]["suggestions"][0];
        assert_eq!(observed["from_line"], start);
        assert_eq!(observed["to_line"], start + original.len() as u64 - 1);
        assert_eq!(
            observed["from_content"],
            format!("{}\n", original.join("\n"))
        );
        let expected = if replacement.is_empty() {
            String::new()
        } else {
            format!("{}\n", replacement.join("\n"))
        };
        assert_eq!(observed["to_content"], expected);
    }
}

#[test]
fn custom_suggestion_templates_cannot_change_typed_range_or_exact_replacement() {
    for template in [
        "{{ fence }}suggestion:-1+0\n{{ replacement_content }}{{ fence }}",
        "{{ fence }}{{ info }}\ndifferent replacement\n{{ fence }}",
    ] {
        let fixture = Fixture::new();
        let profile = fixture.export("wrong-suggestion");
        profile.weak_schemas();
        profile.template("suggestion", template);
        fixture.select(&profile);
        let path = fixture.initial("review", Some(suggestion(2, &["added"], &["corrected"])));
        fixture.select_run(&path);
        let (output, checked) = fixture.run(&["check"]);
        assert_no_writes(&fixture, 0, &output, &checked);
        assert_eq!(checked["status"], "invalid", "{checked}");
        assert!(
            serde_json::to_string(&checked)
                .unwrap()
                .contains("suggestion"),
            "{checked}"
        );
        let (output, value) = fixture.run(&["publish", "--effects", "review"]);
        assert_no_writes(&fixture, 0, &output, &value);
    }
}

#[test]
fn foreign_reviews_remain_untouched_and_cannot_replace_owned_history() {
    let fixture = Fixture::new();
    let profile = fixture.export("foreign");
    profile.weak_schemas();
    profile.prefix("summary", "TEAM SUMMARY");
    fixture.select(&profile);
    let path = fixture.initial("initial", None);
    fixture.publish(&path);
    let foreign = {
        let mut state = fixture.http.state.lock().unwrap();
        let mut foreign = state.discussions[0].clone();
        foreign["id"] = json!("foreign-discussion");
        foreign["notes"][0]["id"] = json!(900);
        foreign["notes"][0]["author"]["id"] = json!(77);
        state.discussions.push(foreign.clone());
        foreign
    };
    let (fixed, value) = fixture.followup("fixed", "fixed");
    write_json(&fixed, &value);
    fixture.publish(&fixed);
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions[1], foreign);
    assert!(
        !state
            .writes()
            .iter()
            .any(|request| request.path.contains("foreign-discussion"))
    );
    assert_eq!(state.notes.len(), 1);
    let summary = public_summary(state.notes[0]["body"].as_str().unwrap());
    let current = summary.split_once("<details>").unwrap().0;
    assert_eq!(
        current
            .lines()
            .filter(|line| line.starts_with("| F1 | "))
            .count(),
        1,
        "{summary}"
    );
    assert!(
        summary.contains("\n| rust-error-contracts | Pass | — | "),
        "{summary}"
    );
    drop(state);
    let (tampered, mut value) = fixture.followup("tampered", "fixed");
    value["findings"][0]["id"] = json!(900);
    value["findings"][0]["reassessment"]["id"] = json!(900);
    write_json(&tampered, &value);
    let before = fixture.writes();
    let (output, checked) = fixture.run(&["check"]);
    assert_no_writes(&fixture, before, &output, &checked);
    assert_eq!(checked["status"], "invalid", "{checked}");
    assert!(
        serde_json::to_string(&checked).unwrap().contains("finding"),
        "{checked}"
    );
    let (output, result) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, before, &output, &result);
    assert_eq!(fixture.http.state.lock().unwrap().discussions[1], foreign);
}

#[test]
fn custom_profiles_preserve_empty_narrowed_and_read_only_authority() {
    let fixture = Fixture::new();
    let profile = fixture.export("authority");
    profile.weak_schemas();
    profile.prefix("summary", "TEAM SUMMARY");
    fixture.select(&profile);
    let path = fixture.initial("initial", None);
    fixture.accept(&path);
    fixture.ok(&["publish", "--effects", "review", "--plan"]);
    fixture.ok(&["publish", "--effects", "none"]);
    assert_eq!(fixture.writes(), 0);
    let mut config = read_json(&fixture.config);
    config["gitlab"]["authority"] = json!([]);
    write_json(&fixture.config, &config);
    let (_, value) = fixture.run(&["publish", "--effects", "review"]);
    assert_ne!(value["run"]["outcome"]["result"]["status"], "completed");
    assert_eq!(fixture.writes(), 0);
    config["gitlab"]["authority"] = json!(["create_summary"]);
    write_json(&fixture.config, &config);
    let (_, value) = fixture.run(&["publish", "--effects", "review"]);
    assert_ne!(value["run"]["outcome"]["result"]["status"], "completed");
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.writes().len(), 1);
    assert_eq!(state.writes()[0].path, format!("{MR_PATH}/notes"));
    assert!(state.discussions.is_empty());
}
