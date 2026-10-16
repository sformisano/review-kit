//! Validated findings and blocked coverage through accepted runs and fresh CLI processes.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "The HTTP fixture also exports adapter-only helpers."
)]
mod support;
use support as server;
#[path = "runs/harness.rs"]
mod harness;

use harness::{Harness, PROOF_POLICY, read_json, write_json};
use serde_json::{Value, json};

fn accepted_blocked(harness: &Harness, validated: bool) -> String {
    let run_id = harness.draft_with_policies(&[harness::ERROR_POLICY, PROOF_POLICY]);
    harness.fill_finding(&run_id);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["decision"] = json!("blocked");
    draft["limitation"] = json!("The integration runner is unavailable.");
    draft["new_findings"][0]["validated"] = json!(validated);
    for row in draft["policies"].as_array_mut().unwrap() {
        if row["policy_id"] == PROOF_POLICY {
            row["outcome"] =
                json!({"kind":"blocked", "limitation":"The integration runner is unavailable."});
        }
    }
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    run_id
}

fn effect_counts(harness: &Harness) -> (usize, usize, usize) {
    (
        harness.writes_to("POST", "/merge_requests/7/notes"),
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        harness.writes_to("PUT", ""),
    )
}

fn assert_settled(harness: &Harness, run_id: &str, result: &Value) {
    assert_eq!(result["status"], "settled", "{result:#}");
    assert_eq!(result["publication"]["status"], "blocked", "{result:#}");
    assert_eq!(harness.manifest(run_id)["completion"], "published");
    assert_eq!(harness.journal()["settled_runs"][0]["outcome"], "published");
}

#[cfg(feature = "fault-injection")]
fn restart_publication(harness: &Harness) -> Value {
    let Some(binary) = std::env::var_os("RKC_TEST_PRODUCTION_BINARY") else {
        return harness.ok(&["publish", "--effects", "review"]);
    };
    let output = assert_cmd::Command::new(binary)
        .timeout(std::time::Duration::from_secs(90))
        .args(["--progress", "none", "--config"])
        .arg(&harness.config)
        .args(["publish", "--effects", "review"])
        .current_dir(&harness.repo)
        .env("HOME", harness.root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .env_remove("RKC_FAULT")
        .env_remove("GITLAB_TOKEN")
        .env(harness::TOKEN_ENV, server::TOKEN)
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(server::TOKEN));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(server::TOKEN));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        output.status.success(),
        "production restart failed: {value:#}"
    );
    value
}

#[test]
fn blocked_run_publishes_validated_finding_and_settles_only_communication() {
    let harness = Harness::new();
    let run_id = accepted_blocked(&harness, true);
    let planned = harness.ok(&["publish", "--plan", "--effects", "review"]);
    assert_eq!(harness.writes(), 0, "planning is read-only");
    assert_eq!(
        planned["run"]["outcome"]["plan"]["round"]["status"], "pending",
        "{planned:#}"
    );
    assert_eq!(
        planned["effects"]["summary_writes"],
        json!([
            {"write":"start", "effect":"create_summary", "within":true},
            {"write":"final", "effect":"update_summary", "within":true}
        ])
    );
    let narrow = harness.ok(&["publish", "--plan", "--effects", "create_summary,finding"]);
    assert_eq!(narrow["effects"]["outside"], json!(["update_summary"]));
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_settled(&harness, &run_id, &published);
    assert_eq!(effect_counts(&harness), (1, 1, 1));
    assert_eq!(harness.discussions(), 1);
    let state = harness.http.state.lock().unwrap();
    let body = state.notes[0]["body"].as_str().unwrap();
    assert!(body.starts_with("### Blocked"));
    assert!(body.contains("The integration runner is unavailable."));
    let public = body.split_once("\n\n<!-- review-kit:state:v3 ").unwrap().0;
    assert!(
        public.contains(
            "\n| Policy baseline-proof-policy | Blocked: see the limitation above | — | "
        )
    );
    assert!(public.contains("\n| Policy rust-error-contracts | Fail | F1 | "));
    assert_eq!(
        public
            .matches("The integration runner is unavailable.")
            .count(),
        1,
        "{public}"
    );
    assert!(body.contains("F1"));
    assert!(!body.contains("This round has not finished posting."));
    drop(state);
    let count = harness.writes();
    assert_eq!(
        harness.ok(&["publish", "--effects", "review"])["status"],
        "already_settled"
    );
    assert_eq!(harness.writes(), count);
}

#[test]
fn blocked_finding_publication_is_read_only_or_unfinished_under_insufficient_authority() {
    for requested in ["none", "create_summary", "create_summary,finding"] {
        let harness = Harness::new();
        let run_id = accepted_blocked(&harness, true);
        let (output, result) = harness.rkc(&["publish", "--effects", requested]);
        if requested == "none" {
            assert!(output.status.success());
            assert_eq!(result["status"], "planned");
            assert_eq!(harness.writes(), 0);
        } else {
            assert_eq!(output.status.code(), Some(4));
            assert_eq!(result["status"], "unfinished");
            let finding_sent = requested == "create_summary,finding";
            assert_eq!(effect_counts(&harness), (1, usize::from(finding_sent), 0));
            assert_eq!(harness.journal()["settled_runs"], json!([]));
            assert_eq!(
                result["effects"]["outside"],
                if finding_sent {
                    json!(["update_summary"])
                } else {
                    json!(["update_summary", "finding"])
                }
            );
        }
        assert_eq!(harness.manifest(&run_id)["completion"], json!(null));
    }
    let harness = Harness::new();
    accepted_blocked(&harness, true);
    let mut config = read_json(&harness.config);
    config["gitlab"]["authority"] = json!([]);
    write_json(&harness.config, &config);
    let (output, result) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(result["status"], "unfinished");
    assert_eq!(harness.writes(), 0);
}

#[test]
fn blocked_run_does_not_admit_an_unvalidated_candidate_as_a_finding_effect() {
    let harness = Harness::new();
    let run_id = accepted_blocked(&harness, false);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_settled(&harness, &run_id, &published);
    assert_eq!(effect_counts(&harness), (1, 0, 0));
    assert_eq!(harness.discussions(), 0);
    let summary = harness.http.state.lock().unwrap().notes[0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(summary.starts_with("### Blocked"), "{summary}");
    assert!(
        !summary.contains("Not posted yet"),
        "a withheld finding is not a failed post: {summary}"
    );
}

#[cfg(feature = "fault-injection")]
#[test]
fn blocked_run_restarts_after_summary_or_finding_without_settling_unfinished_effects() {
    use std::os::unix::process::ExitStatusExt;
    for verified in [1, 2] {
        let harness = Harness::new();
        let run_id = accepted_blocked(&harness, true);
        let fault = format!("publish.after_verified:{verified}");
        let (output, _) = harness.rkc_env(
            &harness.repo,
            &["publish", "--effects", "review"],
            &[("RKC_FAULT", &fault)],
        );
        assert_eq!(output.status.signal(), Some(6));
        assert_eq!(harness.manifest(&run_id)["completion"], json!(null));
        let before = harness.journal();
        assert_eq!(before["settled_runs"], json!([]));
        assert_eq!(before["coverage_binding"]["run_id"], run_id);
        assert_eq!(effect_counts(&harness), (1, usize::from(verified == 2), 0));
        assert!(
            harness.http.state.lock().unwrap().notes[0]["body"]
                .as_str()
                .unwrap()
                .contains("This round has not finished posting.")
        );
        let planned = harness.ok(&["publish", "--plan", "--effects", "review"]);
        assert_eq!(
            planned["effects"]["summary_writes"],
            json!([
                {"write":"start", "effect":"update_summary", "within":true},
                {"write":"final", "effect":"update_summary", "within":true}
            ]),
            "remaining finding actions can be empty: {planned:#}"
        );
        let restarted = restart_publication(&harness);
        assert_settled(&harness, &run_id, &restarted);
        assert_eq!(
            effect_counts(&harness),
            (1, 1, if verified == 2 { 2 } else { 1 })
        );
        let state = harness.http.state.lock().unwrap();
        let writes = state.writes();
        let updates: Vec<_> = writes
            .iter()
            .filter(|request| request.method == "PUT")
            .collect();
        if verified == 2 {
            assert!(
                updates[0].payload["body"]
                    .as_str()
                    .unwrap()
                    .contains("This round has not finished posting.")
            );
            assert!(updates[0].payload["body"].as_str().unwrap().contains("F1"));
        }
        let final_body = &updates.last().unwrap().payload["body"];
        assert!(final_body.as_str().unwrap().starts_with("### Blocked"));
        assert!(
            !final_body
                .as_str()
                .unwrap()
                .contains("This round has not finished posting.")
        );
        assert_eq!(state.notes[0]["body"], *final_body);
        drop(state);
        let after = harness.journal();
        assert_eq!(after["coverage_binding"], before["coverage_binding"]);
        assert_eq!(after["authorization"], before["authorization"]);
        assert_eq!(after["retried_creations"], before["retried_creations"]);
    }
}
