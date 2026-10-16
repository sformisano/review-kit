#![allow(clippy::unwrap_used)]

#[path = "process/file_logging.rs"]
mod file_logging;
#[path = "process/home_config.rs"]
mod home_config;
#[path = "process/logging.rs"]
mod logging;
#[path = "process/policy_assessments.rs"]
mod policy_assessments;

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(dead_code, unused_imports, clippy::expect_used)]
mod support;
use support as server;
#[path = "runs/harness.rs"]
mod harness;

use assert_cmd::Command;
use serde_json::{Value, json};
use std::{
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};

struct Fixture {
    harness: harness::Harness,
}

impl Deref for Fixture {
    type Target = harness::Harness;
    fn deref(&self) -> &Self::Target {
        &self.harness
    }
}

impl Fixture {
    fn new() -> Self {
        Self {
            harness: harness::Harness::new(),
        }
    }

    fn run(&self, args: &[&str]) -> (Output, Value) {
        self.harness.rkc(args)
    }

    fn stderr() -> Self {
        let fixture = Self::new();
        fs::create_dir(fixture.root.path().join(".review-kit")).unwrap();
        fs::write(
            fixture.root.path().join(".review-kit/config.toml"),
            "[logging]\ndestination = \"stderr\"\n",
        )
        .unwrap();
        fixture
    }

    fn prepare(&self) {
        self.prepare_named(
            "rust-codebase-policies",
            &[
                ("baseline-integrity-policy", "Integrity"),
                ("baseline-proof-policy", "Proof"),
            ],
        );
    }

    fn prepare_named(&self, policy_set: &str, policies: &[(&str, &str)]) {
        let id = self.start();
        self.declare_policies(&id, &policies.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        let path = self.file(&id, "review-assessment.draft.json");
        let mut inventory = harness::read_json(&path);
        inventory["policy_set"] = json!(policy_set);
        for (row, (_, name)) in inventory["policies"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(policies)
        {
            row["name"] = json!(name);
        }
        write(&path, &inventory);
    }

    fn ready(&self) {
        self.prepare();
        self.fill_ready();
    }

    fn assessment(&self) -> PathBuf {
        self.file(&self.selected(), "review-assessment.draft.json")
    }

    fn fill_ready(&self) {
        let path = self.assessment();
        let mut value = harness::read_json(&path);
        harness::ready(&mut value);
        value["depth"] = json!("Focused review and proof.");
        value["scope"] = json!("Changed Rust library.");
        value["local_context"] = json!("PRIVATE TEST CONTEXT");
        write(&path, &value);
    }
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[test]
fn captured_run_checks_offline_with_aggregate_repairs() {
    let fixture = Fixture::new();
    fixture.prepare();
    let before = fixture.http.state.lock().unwrap().requests.len();
    let (output, invalid) = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(2));
    for code in [
        "missing_decision",
        "missing_depth",
        "missing_scope",
        "missing_policy_judgment",
    ] {
        assert!(
            invalid["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["code"] == code),
            "{invalid}"
        );
    }
    fixture.fill_ready();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    assert_eq!(checked["status"], "valid");
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("PRIVATE TEST CONTEXT")
    );
    assert!(
        fixture
            .file(&fixture.selected(), "rendered/review-summary.md")
            .is_file()
    );
    assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
    fixture.ok(&["accept"]);
    let mut config = harness::read_json(&fixture.config);
    config.as_object_mut().unwrap().remove("gitlab");
    write(&fixture.config, &config);
    let (output, planned) = fixture.run(&["publish", "--plan"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(planned["diagnostics"][0]["code"], "configuration");
    assert_eq!(fixture.writes(), 0);
}

#[test]
fn malformed_assessment_is_a_repairable_input_error() {
    let fixture = Fixture::new();
    fixture.prepare();
    fs::write(fixture.assessment(), b"{ broken").unwrap();
    let (output, invalid) = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(2), "{invalid}");
    assert_eq!(invalid["diagnostics"][0]["code"], "malformed_draft");
    assert!(
        invalid["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("line 1")
    );
}

#[test]
fn a_new_capture_preserves_the_selected_runs_original_offline_evidence() {
    let fixture = Fixture::new();
    fixture.ready();
    let original = fixture.selected();
    let captured = fs::read(fixture.file(&original, "reviewed-source.json")).unwrap();
    fixture.push_commit();
    fixture.ok(&["run", "start", "--new"]);
    fixture.ok(&["run", "select", &original]);
    let before = fixture.http.state.lock().unwrap().requests.len();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    assert_eq!(
        fs::read(fixture.file(&original, "reviewed-source.json")).unwrap(),
        captured
    );
    assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
    let mut source: Value = serde_json::from_slice(&captured).unwrap();
    source["revision"]["head"] = json!("d".repeat(40));
    write(&fixture.file(&original, "reviewed-source.json"), &source);
    let (output, mismatch) = fixture.run(&["check"]);
    assert!(!output.status.success());
    assert_eq!(mismatch["diagnostics"][0]["code"], "hash_mismatch");
    assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
}

#[test]
fn invalid_gitlab_configuration_is_a_repairable_configuration_error() {
    let fixture = Fixture::new();
    let worktree = fixture.root.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&worktree)
            .status()
            .unwrap()
            .success()
    );
    for server in ["http://gitlab.example.com", "invalid url"] {
        write(
            &fixture.config,
            &json!({
                "version": 1,
                "gitlab": {"server": server, "token_env": "RKC_CONFIG_TEST_TOKEN"}
            }),
        );
        let output = Command::new(env!("CARGO_BIN_EXE_rkc"))
            .timeout(Duration::from_secs(60))
            .arg("--config")
            .arg(&fixture.config)
            .args([
                "run",
                "start",
                "--mr",
                "https://gitlab.example.com/fixture/repo/-/merge_requests/1",
            ])
            .current_dir(&worktree)
            .env("HOME", fixture.root.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("RKC_CONFIG")
            .env("RKC_CONFIG_TEST_TOKEN", "test-token")
            .output()
            .unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(2), "{value}");
        assert_eq!(value["diagnostics"][0]["code"], "configuration");
    }
}
