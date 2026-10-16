use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    MR_PATH,
    fixture::{
        ERROR_POLICY, Fixture, OWNERSHIP_POLICY, POLICY_SET, Profile, finding, public_summary,
        read_json, suggestion, text, violated, write_json,
    },
};

const FINDINGS_HEADER: &str = "| ID | Finding | Severity | Status | This round |";
const HISTORY_HEADER: &str = "| ID | Finding | Severity | Status | That round |";
const POLICIES_HEADER: &str = "| Policy | Result | Open findings | Reason |";
const REASON: &str = "The changed Rust source is covered by this policy.";
/// The embedded summary template that rendered rounds before policies were
/// grouped by applicability.
const PREVIOUS_DEFAULT_SUMMARY: &str = include_str!("previous-default-summary.j2");

#[test]
fn embedded_findings_use_declared_names_and_a_severity_footer_without_evidence() {
    let fixture = Fixture::new();
    let path = fixture.initial("review", None);
    let checked = fixture.check(&path);
    let body = checked["check"]["plan"]["actions"][0]["body"]
        .as_str()
        .unwrap();
    assert!(
        body.starts_with("**F1: Preserve the actionable error**\n"),
        "{body}"
    );
    assert_eq!(
        body.trim_end().lines().last(),
        Some("*P2 · rust-error-contracts*"),
        "{body}"
    );
    for forbidden in ["Evidence", "Blocking", "Co-Authored-By", "Generated with"] {
        assert!(!body.contains(forbidden), "{body}");
    }
    let authored = read_json(&path)["new_findings"][0].clone();
    assert_eq!(body.matches(authored["body"].as_str().unwrap()).count(), 1);
    assert!(!body.contains(authored["evidence"].as_str().unwrap()));
    let mut value = read_json(&path);
    value["new_findings"][0]["disposition"] = json!("advisory");
    write_json(&path, &value);
    let checked = fixture.check(&path);
    let advisory = checked["check"]["plan"]["actions"][0]["body"]
        .as_str()
        .unwrap();
    assert_eq!(advisory, body);
}

#[test]
fn embedded_summary_has_details_verified_links_and_declared_policy_results() {
    let fixture = Fixture::new();
    let path = fixture.initial_with_policies("review", None, &[ERROR_POLICY, OWNERSHIP_POLICY]);
    fixture.publish(&path);
    let summary = fixture.summary();
    let body = public_summary(&summary);
    assert!(
        body.starts_with("### Changes required\n\nRound 1 · 1 open\n"),
        "{body}"
    );
    assert!(body.contains(FINDINGS_HEADER), "{body}");
    assert!(body.contains(POLICIES_HEADER), "{body}");
    assert!(body.contains("<summary>Review details</summary>"), "{body}");
    for line in [
        "- Workflow: codebase-policies-review, policy set rust-codebase-policies",
        "- Scope: Changed committed Rust source.",
        "- Depth: Checked failure handling and regression evidence.",
        &format!("| rust-ownership-design | Pass | — | {REASON} |"),
        &format!("| rust-error-contracts | Fail | F1 | {REASON} |"),
    ] {
        assert!(
            body.lines().any(|value| value == line),
            "missing {line}: {body}"
        );
    }
    let state = fixture.http.state.lock().unwrap();
    let root = &state.discussions[0]["notes"][0];
    assert!(body.contains(&format!("#note_{}", root["id"])), "{body}");
    assert!(!body.contains("PRIVATE PROFILE CONTEXT"));
    assert!(!body.contains(text(fixture.root.path())));
    assert_eq!(state.notes.len(), 1);
}

#[test]
fn historical_rows_survive_fix_recurrence_and_later_policy_exclusion() {
    let fixture = Fixture::new();
    let initial = fixture.initial("initial", None);
    fixture.publish(&initial);
    let original = fixture.http.state.lock().unwrap().discussions[0].clone();
    let (fixed, value) = fixture.followup("fixed", "fixed");
    write_json(&fixed, &value);
    fixture.publish(&fixed);
    let summary = fixture.summary();
    let first = history_section(public_summary(&summary), 1);
    assert!(first.contains(HISTORY_HEADER), "{first}");
    let row = finding_row(first);
    assert!(row.ends_with("| Open | Posted |"), "{row}");
    assert!(finding_row(public_summary(&summary)).contains("| Fixed |"));
    let (recurrent, value) = fixture.followup("recurrence", "open");
    write_json(&recurrent, &value);
    fixture.publish(&recurrent);
    let summary = fixture.summary();
    let body = public_summary(&summary);
    assert_eq!(
        top_level_sections(body),
        [
            "Round 1 · Changes required",
            "Round 2 · Ready",
            "Review details"
        ],
        "{body}"
    );
    assert!(finding_row(body).contains("| Open |"));
    assert!(finding_row(history_section(body, 1)).contains("| Open |"));
    assert!(finding_row(history_section(body, 2)).contains("| Fixed |"));
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(state.discussions[0]["id"], original["id"]);
    assert_eq!(
        state.discussions[0]["notes"][0]["id"],
        original["notes"][0]["id"]
    );
    assert_eq!(
        state.discussions[0]["notes"][0]["body"],
        original["notes"][0]["body"]
    );
    drop(state);

    let (closed, value) = fixture.followup("closed", "fixed");
    write_json(&closed, &value);
    fixture.publish(&closed);
    let (excluded, mut value) = fixture.prepare("excluded", &[OWNERSHIP_POLICY]);
    value["findings"][0]["reassessment"] = fixture.previous_finding(&excluded, 1);
    write_json(&excluded, &value);
    fixture.publish(&excluded);
    let summary = fixture.summary();
    let body = public_summary(&summary);
    assert!(
        body.contains("\n| rust-error-contracts | 1, 2, 3, 4 |\n"),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "\n| rust-ownership-design | Pass | — | {REASON} |\n"
        )),
        "{body}"
    );
}

#[test]
fn rounds_published_in_the_previous_default_layout_stay_readable_with_the_same_writes() {
    let mut writes = Vec::new();
    let mut bodies = Vec::new();
    for previous in [false, true] {
        let fixture = Fixture::new();
        if previous {
            let profile = fixture.export("previous");
            profile.template("summary", PREVIOUS_DEFAULT_SUMMARY);
            fixture.select(&profile);
        }
        fixture.publish(&fixture.initial("initial", None));
        if previous {
            assert!(
                public_summary(&fixture.summary())
                    .contains("| Policy | Applies | Result | Open findings |")
            );
            let mut config = read_json(&fixture.config);
            config.as_object_mut().unwrap().remove("render_profile");
            write_json(&fixture.config, &config);
        }
        let before = fixture.writes();
        let (fixed, value) = fixture.followup("fixed", "fixed");
        write_json(&fixed, &value);
        fixture.publish(&fixed);
        let state = fixture.http.state.lock().unwrap();
        writes.push(
            state.writes()[before..]
                .iter()
                .map(|request| (request.method.clone(), request.path.clone()))
                .collect::<Vec<_>>(),
        );
        drop(state);
        bodies.push(
            public_summary(&fixture.summary()).replace(fixture.http.server.as_str(), "SERVER"),
        );
    }
    assert_eq!(writes[0], writes[1]);
    assert_eq!(bodies[0], bodies[1]);
    let body = &bodies[1];
    assert_eq!(body.matches(POLICIES_HEADER).count(), 1, "{body}");
    assert!(finding_row(body).contains("| Fixed |"), "{body}");
    let first = history_section(body, 1);
    assert!(
        first.contains(&format!("\n| rust-error-contracts | Fail | {REASON} |\n")),
        "{first}"
    );
    assert!(finding_row(first).ends_with("| Open | Posted |"), "{first}");
}

#[test]
fn exported_profile_matches_embedded_rendering_and_preserves_existing_files() {
    let fixture = Fixture::new();
    let path = fixture.initial("review", Some(suggestion(2, &["added"], &["corrected"])));
    let embedded = fixture.check(&path);
    let profile = fixture.export("profile");
    fixture.select(&profile);
    let exported = fixture.check(&path);
    assert_eq!(embedded["check"]["plan"], exported["check"]["plan"]);
    let original = fs::read(&profile.manifest).unwrap();
    fs::write(
        profile.asset("finding", "template"),
        "caller-owned template",
    )
    .unwrap();
    let (output, value) = fixture.run(&[
        "render-profile",
        "export",
        "--dir",
        text(profile.manifest.parent().unwrap()),
    ]);
    assert!(!output.status.success(), "{value}");
    assert_eq!(fs::read(&profile.manifest).unwrap(), original);
    assert_eq!(
        fs::read_to_string(profile.asset("finding", "template")).unwrap(),
        "caller-owned template"
    );
}

#[test]
fn retained_policy_names_survive_removal_from_current_assessment_coverage() {
    let fixture = Fixture::new();
    let path = fixture.initial("initial", None);
    fixture.publish(&path);
    let (fixed, value) = fixture.followup("fixed", "fixed");
    write_json(&fixed, &value);
    fixture.publish(&fixed);
    let (removed, mut value) = fixture.prepare("removed", &[OWNERSHIP_POLICY]);
    value["findings"][0]["reassessment"] = fixture.previous_finding(&removed, 1);
    write_json(&removed, &value);
    fixture.publish(&removed);
    let summary = fixture.summary();
    assert!(
        public_summary(&summary).contains("\n| rust-error-contracts | 1, 2 |\n"),
        "{summary}"
    );
}

#[test]
fn renamed_inventory_policy_creates_a_successor_round_and_retains_historical_names() {
    let fixture = Fixture::new();
    let (initial, mut assessment) = fixture.prepare_named(
        "original-name",
        &[(ERROR_POLICY, "Error handling")],
        POLICY_SET,
    );
    assessment["new_findings"] = json!([finding()]);
    violated(&mut assessment);
    write_json(&initial, &assessment);
    let original_check = fixture.check(&initial);
    let first = fixture.publish(&initial);
    assert_eq!(first["run"]["outcome"]["result"]["round"], 1);
    let roots = fixture.http.state.lock().unwrap().discussions.clone();
    let (renamed, mut assessment) = fixture.prepare_named(
        "renamed",
        &[(ERROR_POLICY, "Actionable errors")],
        POLICY_SET,
    );
    assessment["findings"][0]["reassessment"] = fixture.previous_finding(&renamed, 1);
    violated(&mut assessment);
    write_json(&renamed, &assessment);
    let checked = fixture.check(&renamed);
    assert_eq!(
        checked["check"]["plan"]["round"]["assessment"],
        original_check["check"]["plan"]["round"]["assessment"]
    );
    assert_eq!(checked["plan"]["round"], 2, "{checked}");
    assert_eq!(checked["plan"]["unchanged"], false, "{checked}");
    let published = fixture.publish(&renamed);
    assert_eq!(published["run"]["outcome"]["result"]["status"], "completed");
    assert_eq!(published["run"]["outcome"]["result"]["round"], 2);
    let summary = fixture.summary();
    let body = public_summary(&summary);
    let current = body
        .split_once("<summary>Review details</summary>")
        .unwrap()
        .1;
    assert!(
        current.contains(&format!("\n| Actionable errors | Fail | F1 | {REASON} |\n")),
        "{body}"
    );
    assert!(!current.contains("| Error handling |"), "{body}");
    let historical = history_section(body, 1);
    assert!(
        historical.contains(&format!("\n| Error handling | Fail | {REASON} |\n")),
        "{historical}"
    );
    assert!(
        !historical.contains("| Actionable errors |"),
        "{historical}"
    );
    assert_eq!(fixture.http.state.lock().unwrap().discussions, roots);
}

#[test]
fn missing_snapshot_is_marked_unavailable_without_invented_rows() {
    let fixture = Fixture::new();
    let path = fixture.initial("initial", None);
    fixture.publish(&path);
    {
        let mut state = fixture.http.state.lock().unwrap();
        let body = state.notes[0]["body"].as_str().unwrap();
        let (presentation, retained) = body.split_once("\n\n<!-- review-kit:state:v3 ").unwrap();
        let (bytes, attachment) = retained.split_once(" -->\n\n<!-- review-kit:v1 ").unwrap();
        let mut record: Value = serde_json::from_str(bytes).unwrap();
        record["current"]
            .as_object_mut()
            .unwrap()
            .remove("communicated_findings");
        state.notes[0]["body"] = json!(format!(
            "{presentation}\n\n<!-- review-kit:state:v3 {} -->\n\n<!-- review-kit:v1 {attachment}",
            serde_json::to_string(&record).unwrap()
        ));
    }
    let (fixed, value) = fixture.followup("fixed", "fixed");
    write_json(&fixed, &value);
    fixture.publish(&fixed);
    let summary = fixture.summary();
    let first = history_section(public_summary(&summary), 1);
    assert!(
        first.contains("\nFindings for this round were not recorded.\n"),
        "{first}"
    );
    assert!(
        !first.lines().any(|line| line.starts_with("| F1 | ")),
        "{first}"
    );
}

#[test]
fn edited_profile_reaches_preview_publication_and_exact_execution_evidence() {
    let fixture = Fixture::new();
    let profile = fixture.export("team");
    profile.prefix("finding", "TEAM FINDING");
    profile.prefix("summary", "TEAM SUMMARY");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    let checked = fixture.check(&path);
    let identity = checked["render_profile"].clone();
    assert_eq!(identity["id"], "review-kit-default");
    assert_eq!(identity["input_contract"], "review-kit-render-input/v2");
    assert!(!identity["fingerprint"].as_str().unwrap().is_empty());
    let finding = checked["check"]["plan"]["actions"][0]["body"]
        .as_str()
        .unwrap();
    assert!(finding.starts_with("TEAM FINDING\n"));
    let accepted = fixture.accept(&path);
    assert_eq!(accepted["render_profile"], identity);
    let planned = fixture.ok(&["publish", "--effects", "review", "--plan"]);
    assert!(
        planned["run"]["outcome"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM SUMMARY\n")
    );
    let published = fixture.publish(&path);
    assert_eq!(published["render_profile"], identity);
    let state = fixture.http.state.lock().unwrap();
    let sent = state
        .writes()
        .into_iter()
        .find(|request| request.path == format!("{MR_PATH}/discussions"))
        .unwrap();
    assert_eq!(
        sent.payload["body"].as_str().unwrap(),
        format!(
            "{finding}\n\n<!-- review-kit:v1 {{\"kind\":\"finding\",\"finding\":1,\"round\":1}} -->"
        )
    );
    assert!(public_summary(state.notes[0]["body"].as_str().unwrap()).starts_with("TEAM SUMMARY\n"));
    drop(state);
    let evidence = read_json(&path.with_file_name("review-publication-events.json"));
    let events = evidence["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .filter_map(|event| event["renderer"].as_str())
            .any(|encoded| serde_json::from_str::<Value>(encoded).unwrap() == identity)
    );
    assert!(
        serde_json::to_string(&evidence)
            .unwrap()
            .contains("TEAM FINDING")
    );
}

#[test]
fn version_1_profile_reports_its_contract_and_publishes_its_own_layout() {
    let fixture = Fixture::new();
    fixture.select(&Profile {
        manifest: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/render-profiles/compact/profile.json"),
    });
    let path = fixture.initial("review", None);
    let checked = fixture.check(&path);
    assert_eq!(
        checked["render_profile"]["input_contract"],
        "review-kit-render-input/v1"
    );
    fixture.publish(&path);
    let summary = fixture.summary();
    assert!(
        public_summary(&summary).starts_with("## Review 1: "),
        "{summary}"
    );
}

#[test]
fn custom_profiles_render_all_public_artifact_roles() {
    let fixture = Fixture::new();
    let profile = fixture.export("all-roles");
    profile.prefix("finding", "TEAM FINDING");
    profile.prefix("summary", "TEAM SUMMARY");
    profile.prefix("suggestion", "TEAM SUGGESTION");
    profile.template("transition", "TEAM TRANSITION F{{ finding.id }} {{ previous_status }} -> {{ new_status }}\n\n{{ explanation }}");
    profile.template(
        "answer",
        "TEAM ANSWER F{{ finding.id }} QUESTION {{ question.id }}\n\n{{ body }}",
    );
    fixture.select(&profile);
    let path = fixture.initial("initial", Some(suggestion(2, &["added"], &["corrected"])));
    fixture.publish(&path);
    let question = fixture
        .http
        .state
        .lock()
        .unwrap()
        .question("discussion-1", "Is the failure regression covered?");
    let (fixed, mut value) = fixture.followup("fixed", "fixed");
    value["answers"] = json!([{"finding":1,"in_reply_to":question.to_string(),"body":"The regression covers the failure branch."}]);
    write_json(&fixed, &value);
    fixture.publish(&fixed);
    let state = fixture.http.state.lock().unwrap();
    let root = state.discussions[0]["notes"][0]["body"].as_str().unwrap();
    assert!(root.starts_with("TEAM FINDING\n"));
    assert!(root.contains("TEAM SUGGESTION\n"));
    assert_eq!(
        state.discussions[0]["notes"][0]["suggestions"][0]["to_content"],
        "corrected\n"
    );
    let replies = state.discussions[0]["notes"].as_array().unwrap();
    assert!(replies.iter().any(|note| {
        note["body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM TRANSITION F1 ")
    }));
    assert!(replies.iter().any(|note| {
        note["body"]
            .as_str()
            .unwrap()
            .starts_with(&format!("TEAM ANSWER F1 QUESTION {question}\n"))
    }));
    assert!(
        state.notes[0]["body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM SUMMARY\n")
    );
}

fn history_section(body: &str, number: u64) -> &str {
    body.split_once(&format!("<summary>Round {number}"))
        .unwrap()
        .1
        .split_once("</details>")
        .unwrap()
        .0
}

/// The summaries of the top-level collapsed sections in order; a section
/// opened inside another fails the test.
fn top_level_sections(body: &str) -> Vec<&str> {
    let mut depth = 0_usize;
    let mut summaries = Vec::new();
    for line in body.lines() {
        if line == "<details>" {
            depth += 1;
            assert_eq!(depth, 1, "nested section: {body}");
        } else if line == "</details>" {
            depth = depth.checked_sub(1).expect("unopened section");
        } else if let Some(summary) = line
            .strip_prefix("<summary>")
            .and_then(|rest| rest.strip_suffix("</summary>"))
        {
            summaries.push(summary);
        }
    }
    assert_eq!(depth, 0, "unclosed section: {body}");
    summaries
}

fn finding_row(body: &str) -> &str {
    body.lines()
        .find(|line| line.starts_with("| F1 | "))
        .unwrap()
}
