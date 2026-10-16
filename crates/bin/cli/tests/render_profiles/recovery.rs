use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    MR_PATH,
    fixture::{
        Fixture, accepted_inputs, assert_accepted_inputs_unchanged, assert_no_writes,
        public_summary, read_json, write_json,
    },
    server::{Fault, Response},
};

#[test]
fn changed_preview_requires_revalidation_before_publication() {
    let fixture = Fixture::new();
    let profile = fixture.export("preview");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let old = fixture.check(&path);
    profile.prefix("finding", "REVISED PREVIEW");
    let before_reads = fixture.http.state.lock().unwrap().requests.len();
    let (output, changed) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, 0, &output, &changed);
    assert!(
        has_diagnostic(&changed, "render_profile_changed"),
        "{changed}"
    );
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        before_reads
    );
    let new = fixture.check(&path);
    assert_ne!(
        old["render_profile"]["fingerprint"],
        new["render_profile"]["fingerprint"]
    );
    let accepted = fixture.accept(&path);
    assert_eq!(accepted["render_profile"], new["render_profile"]);
    let published = fixture.publish(&path);
    assert_eq!(published["render_profile"], new["render_profile"]);
    assert!(
        fixture.http.state.lock().unwrap().discussions[0]["notes"][0]["body"]
            .as_str()
            .unwrap()
            .starts_with("REVISED PREVIEW\n")
    );
}

#[test]
fn publication_plans_bind_assets_and_refresh_an_older_check_identity() {
    let fixture = Fixture::new();
    let profile = fixture.export("plan-preview");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let plan = fixture.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(fixture.writes(), 0);
    profile.prefix("finding", "CHANGED AFTER PLAN");
    let (output, changed) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, 0, &output, &changed);
    assert!(
        has_diagnostic(&changed, "render_profile_changed"),
        "{changed}"
    );
    fixture.check(&path);
    profile.prefix("finding", "REVALIDATED BY PLAN");
    let fresh = fixture.ok(&["publish", "--effects", "review", "--plan"]);
    assert_ne!(fresh["render_profile"], plan["render_profile"]);
    let published = fixture.publish(&path);
    assert_eq!(published["render_profile"], fresh["render_profile"]);
    assert!(
        fixture.http.state.lock().unwrap().discussions[0]["notes"][0]["body"]
            .as_str()
            .unwrap()
            .starts_with("REVALIDATED BY PLAN\n")
    );
}

#[test]
fn refreshed_publication_plan_rejects_unsafe_dynamic_summary_without_refreshing_preview() {
    let fixture = Fixture::new();
    let profile = fixture.export("dynamic-plan");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.check(&path);
    fixture.publish(&path);
    let writes = fixture.writes();
    let (path, mut value) = fixture.followup("dynamic-followup", "open");
    value["scope"] = json!("Changed committed Rust source and the retained root.");
    write_json(&path, &value);
    fixture.accept(&path);
    let receipt = path.parent().unwrap().join("review-render-profile.json");
    let original = fs::read(&receipt).unwrap();
    profile.template(
        "summary",
        "{% if counts.total > 0 %}/close{% else %}Safe summary{% endif %}",
    );
    let (output, rejected) = fixture.run(&["publish", "--effects", "review", "--plan"]);
    assert_no_writes(&fixture, writes, &output, &rejected);
    assert!(has_diagnostic(&rejected, "gitlab_body"), "{rejected}");
    assert_eq!(fs::read(&receipt).unwrap(), original);
}

#[test]
fn unchanged_retry_and_presentation_change_preserve_completed_round_and_roots() {
    let fixture = Fixture::new();
    let profile = fixture.export("unchanged");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    let first = fixture.publish(&path);
    let writes = fixture.writes();
    let notes = fixture.http.state.lock().unwrap().notes.clone();
    let discussions = fixture.http.state.lock().unwrap().discussions.clone();
    let retry = fixture.publish(&path);
    assert_eq!(retry["status"], "already_settled");
    assert_eq!(fixture.writes(), writes);
    let (path, mut value) = fixture.prepare("unchanged-followup", &[super::fixture::ERROR_POLICY]);
    value["findings"][0]["reassessment"] = fixture.previous_finding(&path, 1);
    super::fixture::violated(&mut value);
    write_json(&path, &value);
    profile.prefix("finding", "PRESENTATION CHANGE");
    profile.prefix("summary", "PRESENTATION CHANGE");
    let checked = fixture.check(&path);
    assert!(
        checked["check"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("PRESENTATION CHANGE\n"),
        "{checked}"
    );
    assert!(
        checked["check"]["plan"]["terminal_summary_preview"]
            .as_str()
            .unwrap()
            .starts_with("PRESENTATION CHANGE\n"),
        "{checked}"
    );
    let changed = fixture.publish(&path);
    assert_eq!(changed["run"]["outcome"]["result"]["status"], "unchanged");
    assert_eq!(
        changed["run"]["outcome"]["result"]["round"],
        first["run"]["outcome"]["result"]["round"]
    );
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.writes().len(), writes);
    assert_eq!(state.notes, notes);
    assert_eq!(state.discussions, discussions);
}

#[test]
fn changed_profile_preserves_unknown_write_intent_across_fresh_processes() {
    let fixture = Fixture::lost_responses();
    let profile = fixture.export("unknown");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let (output, first) = fixture.run(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{first}");
    let progress = Path::new(first["progress"].as_str().unwrap());
    let retained = fs::read(progress).unwrap();
    assert!(read_json(progress)["unresolved"].is_object());
    let inputs = accepted_inputs(&path);
    let before = fixture.writes();
    let notes = fixture.http.state.lock().unwrap().notes.clone();
    profile.prefix("finding", "CHANGED UNKNOWN INTENT");
    let before_requests = fixture.http.state.lock().unwrap().requests.len();
    let checked = fixture.check(&path);
    assert_eq!(checked["status"], "valid", "{checked}");
    assert_ne!(checked["render_profile"], first["render_profile"]);
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        before_requests
    );
    assert_eq!(fixture.writes(), before);
    assert_accepted_inputs_unchanged(&inputs);
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_accepted_inputs_unchanged(&inputs);
    let (output, changed) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, before, &output, &changed);
    assert!(
        has_diagnostic(&changed, "render_profile_changed"),
        "{changed}"
    );
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_eq!(fixture.http.state.lock().unwrap().notes, notes);
    assert!(fixture.http.state.lock().unwrap().discussions.is_empty());
}

#[test]
fn profile_change_cannot_replace_confirmed_partial_round_intent() {
    let fixture = Fixture::new();
    let profile = fixture.export("partial");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response(
            "POST",
            &format!("{MR_PATH}/discussions"),
            Response::status(403),
        ));
    let (output, first) = fixture.run(&["publish", "--effects", "review"]);
    assert!(!output.status.success(), "{first}");
    assert_eq!(first["run"]["outcome"]["result"]["status"], "partial");
    let progress = Path::new(first["progress"].as_str().unwrap());
    let retained = fs::read(progress).unwrap();
    let inputs = accepted_inputs(&path);
    let notes = fixture.http.state.lock().unwrap().notes.clone();
    let before = fixture.writes();
    profile.prefix("finding", "CHANGED PARTIAL INTENT");
    let before_requests = fixture.http.state.lock().unwrap().requests.len();
    let checked = fixture.check(&path);
    assert_eq!(checked["status"], "valid", "{checked}");
    assert_ne!(checked["render_profile"], first["render_profile"]);
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        before_requests
    );
    assert_eq!(fixture.writes(), before);
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_accepted_inputs_unchanged(&inputs);
    let (output, changed) = fixture.run(&["publish", "--effects", "review"]);
    assert_no_writes(&fixture, before, &output, &changed);
    assert!(
        has_diagnostic(&changed, "render_profile_changed"),
        "{changed}"
    );
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_accepted_inputs_unchanged(&inputs);
    assert_eq!(fixture.http.state.lock().unwrap().notes, notes);
    assert!(fixture.http.state.lock().unwrap().discussions.is_empty());
}

#[test]
fn unchanged_profile_recovers_late_committed_exact_body_without_duplicate_creation() {
    let fixture = Fixture::lost_responses();
    let profile = fixture.export("late");
    profile.prefix("finding", "EXACT RETAINED BODY");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let (output, first) = fixture.run(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{first}");
    {
        let mut state = fixture.http.state.lock().unwrap();
        let request = state
            .writes()
            .into_iter()
            .find(|request| request.path == format!("{MR_PATH}/discussions"))
            .unwrap()
            .clone();
        state.commit(&request);
    }
    let recovered = fixture.publish(&path);
    assert_eq!(recovered["run"]["outcome"]["result"]["status"], "completed");
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(
        state
            .writes()
            .iter()
            .filter(|request| request.method == "POST"
                && request.path == format!("{MR_PATH}/discussions"))
            .count(),
        1
    );
    assert!(
        state.discussions[0]["notes"][0]["body"]
            .as_str()
            .unwrap()
            .starts_with("EXACT RETAINED BODY\n")
    );
}

#[test]
fn verified_resolution_receipt_survives_terminal_summary_rejection_and_fresh_cli_recovery() {
    let fixture = Fixture::new();
    let profile = fixture.export("resolution-receipts");
    profile.prefix("summary", "TEAM RECEIPT SUMMARY");
    fixture.select(&profile);
    let initial = fixture.initial("initial", None);
    fixture.publish(&initial);
    let root = fixture.http.state.lock().unwrap().discussions[0].clone();
    let (fixed, value) = fixture.followup("fixed", "fixed");
    write_json(&fixed, &value);
    fixture.accept(&fixed);
    {
        let mut state = fixture.http.state.lock().unwrap();
        let path = format!("{MR_PATH}/notes/{}", state.notes[0]["id"]);
        state.faults.push(Fault::response_after(
            "PUT",
            &path,
            1,
            Response::status(403),
        ));
    }
    let (output, failed) = fixture.run(&["publish", "--effects", "review"]);
    assert!(!output.status.success(), "{failed}");
    assert_eq!(failed["run"]["outcome"]["result"]["status"], "partial");
    assert_eq!(failed["run"]["outcome"]["result"]["round"], 2);
    let progress = read_json(Path::new(failed["progress"].as_str().unwrap()));
    let receipts = progress["verified_resolutions"].as_array().unwrap();
    assert_eq!(receipts.len(), 1, "{progress}");
    assert_eq!(receipts[0]["round"], 2);
    assert_eq!(receipts[0]["thread"], root["id"]);
    assert_eq!(receipts[0]["resolved"], true);
    let (transitions, resolutions) = {
        let state = fixture.http.state.lock().unwrap();
        assert_eq!(state.discussions[0]["notes"][0]["resolved"], true);
        assert_eq!(
            state.discussions[0]["notes"][0]["id"],
            root["notes"][0]["id"]
        );
        assert!(state.faults.is_empty());
        (
            state
                .writes()
                .iter()
                .filter(|request| {
                    request.method == "POST"
                        && request.path.ends_with("/discussions/discussion-1/notes")
                })
                .count(),
            state
                .writes()
                .iter()
                .filter(|request| {
                    request.method == "PUT" && request.path.ends_with("/discussions/discussion-1")
                })
                .count(),
        )
    };
    assert_eq!(transitions, 1);
    assert_eq!(resolutions, 1);

    let recovered = fixture.publish(&fixed);
    assert_eq!(recovered["run"]["outcome"]["result"]["status"], "completed");
    assert_eq!(recovered["run"]["outcome"]["result"]["round"], 2);
    {
        let state = fixture.http.state.lock().unwrap();
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|request| request.method == "POST"
                    && request.path.ends_with("/discussions/discussion-1/notes"))
                .count(),
            transitions
        );
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|request| request.method == "PUT"
                    && request.path.ends_with("/discussions/discussion-1"))
                .count(),
            resolutions
        );
        assert_eq!(state.discussions.len(), 1);
    }
    let summary = fixture.summary();
    let row = public_summary(&summary)
        .lines()
        .find(|line| line.starts_with("| F1 | "))
        .unwrap();
    assert!(row.contains("| Fixed |"), "{row}");
    assert!(row.ends_with("| Fixed | Open → Fixed, resolved |"), "{row}");

    let (recurrent, value) = fixture.followup("recurrent", "open");
    write_json(&recurrent, &value);
    let subsequent = fixture.publish(&recurrent);
    assert_eq!(subsequent["run"]["outcome"]["result"]["round"], 3);
    let summary = fixture.summary();
    let historical = public_summary(&summary)
        .split_once("<summary>Round 2")
        .unwrap()
        .1
        .split_once("</details>")
        .unwrap()
        .0;
    let row = historical
        .lines()
        .find(|line| line.starts_with("| F1 | "))
        .unwrap();
    assert!(row.contains("| Fixed |"), "{row}");
    assert!(row.ends_with("| Fixed | Open → Fixed, resolved |"), "{row}");
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(
        state.discussions[0]["notes"][0]["id"],
        root["notes"][0]["id"]
    );
    assert_eq!(
        state.discussions[0]["notes"][0]["body"],
        root["notes"][0]["body"]
    );
}

#[test]
fn abandonment_without_home_or_policy_skills_preserves_profile_safety() {
    let fixture = Fixture::new();
    let profile = fixture.export("abandon");
    profile.prefix("summary", "TEAM ABANDONMENT SUMMARY");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let mut config = read_json(&fixture.config);
    config["gitlab"]["authority"] = json!(["create_summary"]);
    write_json(&fixture.config, &config);
    let (_, first) = fixture.run(&["publish", "--effects", "review"]);
    assert_eq!(first["run"]["outcome"]["result"]["status"], "partial");
    config["gitlab"]["authority"] = json!(["update_summary"]);
    write_json(&fixture.config, &config);
    profile.prefix("summary", "/close");
    let before = fixture.writes();
    let (output, value) = fixture.without_home(&[
        "publish",
        "--abandon",
        "--reason",
        "The review attempt ends without publication.",
    ]);
    assert_no_writes(&fixture, before, &output, &value);
    let template = profile.asset("summary", "template");
    let unsafe_body = fs::read_to_string(&template).unwrap();
    fs::write(&template, unsafe_body.strip_prefix("/close\n").unwrap()).unwrap();
    let (output, abandoned) = fixture.without_home(&[
        "publish",
        "--abandon",
        "--reason",
        "The review attempt ends without publication.",
    ]);
    assert!(output.status.success(), "{abandoned}");
    assert_eq!(abandoned["render_profile"]["id"], "review-kit-default");
    assert!(fixture.summary().starts_with("TEAM ABANDONMENT SUMMARY\n"));
    assert!(
        fixture
            .summary()
            .contains("The review attempt ends without publication.")
    );
    assert!(fixture.http.state.lock().unwrap().discussions.is_empty());
}

#[test]
fn accepted_run_recovery_preserves_uncertain_effects_without_home_or_skills() {
    let fixture = Fixture::lost_responses();
    let (path, mut assessment) = fixture.prepare_named(
        "retained-review",
        &[(super::fixture::ERROR_POLICY, super::fixture::ERROR_POLICY)],
        "retained-policy-set",
    );
    assessment["new_findings"] = json!([super::fixture::finding()]);
    super::fixture::violated(&mut assessment);
    write_json(&path, &assessment);
    fixture.accept(&path);
    assert!(!fixture.root.path().join(".agents").exists());
    let (output, checked) = fixture.without_home(&["check"]);
    assert!(output.status.success(), "{checked}");
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let arguments = ["publish", "--effects", "review"];
    let (output, uncertain) = fixture.without_home(&arguments);
    assert_eq!(output.status.code(), Some(6), "{uncertain}");
    let progress = Path::new(uncertain["progress"].as_str().unwrap());
    let retained = fs::read(progress).unwrap();
    assert!(read_json(progress)["unresolved"].is_object());
    let before = fixture.writes();
    let (output, blocked) = fixture.without_home(&[
        "publish",
        "--abandon",
        "--reason",
        "An uncertain finding cannot be discarded.",
    ]);
    assert_eq!(output.status.code(), Some(6), "{blocked}");
    assert_eq!(blocked["abandon"]["result"]["status"], "uncertain");
    assert_eq!(blocked["abandon"]["result"]["attempted"], json!([]));
    assert_eq!(fixture.writes(), before);
    assert_eq!(fs::read(progress).unwrap(), retained);
    {
        let mut state = fixture.http.state.lock().unwrap();
        let request = state
            .writes()
            .into_iter()
            .find(|request| request.path == format!("{MR_PATH}/discussions"))
            .unwrap()
            .clone();
        state.commit(&request);
    }
    let (output, recovered) = fixture.without_home(&arguments);
    assert!(output.status.success(), "{recovered}");
    assert_eq!(recovered["run"]["outcome"]["result"]["status"], "completed");
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(
        state
            .writes()
            .iter()
            .filter(|request| {
                request.method == "POST" && request.path == format!("{MR_PATH}/discussions")
            })
            .count(),
        1
    );
}

fn has_diagnostic(value: &Value, code: &str) -> bool {
    [
        "/diagnostics",
        "/check/diagnostics",
        "/run/outcome/diagnostics",
    ]
    .iter()
    .filter_map(|path| value.pointer(path))
    .filter_map(Value::as_array)
    .any(|issues| issues.iter().any(|issue| issue["code"] == code))
}
