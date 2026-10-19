use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as Process, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{TOKEN, server};

pub const ERROR_POLICY: &str = "rust-error-contracts";
pub const OWNERSHIP_POLICY: &str = "rust-ownership-design";
pub const POLICY_SET: &str = "rust-codebase-policies";
/// The summary template embedded in rkc 0.1.0.
pub const PREVIOUS_DEFAULT_SUMMARY: &str = include_str!("previous-default-summary.j2");

pub struct Fixture {
    pub root: TempDir,
    pub http: server::Fixture,
    pub config: PathBuf,
    commands: AtomicUsize,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_http(server::Fixture::new())
    }

    pub fn lost_responses() -> Self {
        Self::with_http(server::Fixture::with_lost_responses())
    }

    fn with_http(http: server::Fixture) -> Self {
        let root = tempfile::Builder::new()
            .prefix("rkc-render-profile-")
            .tempdir()
            .unwrap();
        git(root.path(), &["init", "-q"]);
        git(
            root.path(),
            &["commit", "-q", "--allow-empty", "-m", "initial"],
        );
        let config = root.path().join("config/config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        write_json(
            &config,
            &json!({
                "version":1,"publication_state":"state",
                "review_workflow":"codebase-policies-review",
                "gitlab":{"server":http.server,"token_env":"RKC_PROFILE_TEST_TOKEN",
                    "authority":["create_summary","update_summary","finding","reply","resolve","reopen"]}
            }),
        );
        Self {
            root,
            http,
            config,
            commands: AtomicUsize::new(0),
        }
    }

    pub fn command(&self, config: &Path, directory: &Path, args: &[&str]) -> (Output, Value) {
        self.command_with_home(
            config,
            directory,
            args,
            Some(&self.root.path().join("home")),
        )
    }

    pub fn without_home(&self, args: &[&str]) -> (Output, Value) {
        self.command_with_home(&self.config, self.root.path(), args, None)
    }

    #[allow(
        clippy::panic,
        reason = "Invalid child JSON is a fixture failure with captured output."
    )]
    fn command_with_home(
        &self,
        config: &Path,
        directory: &Path,
        args: &[&str],
        home: Option<&Path>,
    ) -> (Output, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rkc"));
        command
            .timeout(Duration::from_secs(60))
            .arg("--config")
            .arg(config)
            .args(args)
            .current_dir(directory)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("RKC_CONFIG")
            .env("RKC_PROFILE_TEST_TOKEN", TOKEN)
            .env_remove("GITLAB_TOKEN")
            .env_remove("REVIEW_KIT_REVIEWER_TOKEN");
        match home {
            Some(home) => command.env("HOME", home),
            None => command.env_remove("HOME"),
        };
        let output = command.output().unwrap();
        if let Some(evidence) = std::env::var_os("RKC_PROFILE_TEST_EVIDENCE") {
            let child_evidence =
                PathBuf::from(evidence).join(self.root.path().file_name().unwrap());
            fs::create_dir_all(&child_evidence).unwrap();
            let label = format!("{:04}", self.commands.fetch_add(1, Ordering::Relaxed));
            fs::write(
                child_evidence.join(format!("{label}.stdout")),
                &output.stdout,
            )
            .unwrap();
            fs::write(
                child_evidence.join(format!("{label}.stderr")),
                &output.stderr,
            )
            .unwrap();
            let argv = ["--config", text(config)]
                .into_iter()
                .chain(args.iter().copied())
                .collect::<Vec<_>>();
            write_json(
                &child_evidence.join(format!("{label}.json")),
                &json!({
                    "argv": argv,
                    "binary": env!("CARGO_BIN_EXE_rkc"),
                    "config": config,
                    "cwd": directory,
                    "exit_code": output.status.code(),
                    "home_present": home.is_some(),
                }),
            );
        }
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "CLI returned invalid JSON: {error}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
        (output, value)
    }

    pub fn run(&self, args: &[&str]) -> (Output, Value) {
        self.command(&self.config, self.root.path(), args)
    }

    pub fn ok(&self, args: &[&str]) -> Value {
        let (output, value) = self.run(args);
        assert!(output.status.success(), "{args:?}: {value}");
        value
    }

    /// A captured run with a complete authored policy declaration.
    pub fn prepare(&self, name: &str, policies: &[&str]) -> (PathBuf, Value) {
        let named = policies
            .iter()
            .map(|policy| (*policy, *policy))
            .collect::<Vec<_>>();
        self.prepare_named(name, &named, POLICY_SET)
    }

    pub fn prepare_named(
        &self,
        _name: &str,
        policies: &[(&str, &str)],
        policy_set: &str,
    ) -> (PathBuf, Value) {
        let mr = format!("{}/group/sub/repo/-/merge_requests/7", self.http.server);
        let started = self.ok(&["run", "start", "--new", "--mr", &mr]);
        assert_eq!(started["status"], "created", "{started}");
        let directory = self
            .root
            .path()
            .join(started["run_directory"].as_str().unwrap());
        let path = directory.join("review-assessment.draft.json");
        let mut value = read_json(&path);
        value["policy_set"] = json!(policy_set);
        value["policies"] = json!(
            policies
                .iter()
                .map(|(policy, name)| json!({
                    "policy_id": policy, "name": name,
                    "required": false,
                    "reason": "The changed Rust source is covered by this policy.",
                    "outcome": {"kind":"satisfied"},
                }))
                .collect::<Vec<_>>()
        );
        ready(&mut value);
        (path, value)
    }

    pub fn initial(&self, name: &str, suggestion: Option<Value>) -> PathBuf {
        self.initial_with_policies(name, suggestion, &[ERROR_POLICY])
    }

    pub fn initial_with_policies(
        &self,
        name: &str,
        suggestion: Option<Value>,
        policies: &[&str],
    ) -> PathBuf {
        let (path, mut value) = self.prepare(name, policies);
        let mut finding = finding();
        finding["suggestion"] = suggestion.unwrap_or(Value::Null);
        value["new_findings"] = json!([finding]);
        violated(&mut value);
        write_json(&path, &value);
        path
    }

    pub fn check(&self, path: &Path) -> Value {
        self.select_run(path);
        self.ok(&["check"])
    }

    pub fn accept(&self, path: &Path) -> Value {
        self.select_run(path);
        let accepted = self.ok(&["accept"]);
        assert!(
            matches!(accepted["status"].as_str(), Some("stored" | "existing")),
            "{accepted}"
        );
        accepted
    }

    pub fn select_run(&self, path: &Path) {
        let id = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let selected = self.ok(&["run", "select", id]);
        assert_eq!(selected["selected_run_id"], id, "{selected}");
    }

    pub fn publish(&self, path: &Path) -> Value {
        self.select_run(path);
        let manifest = read_json(&path.with_file_name("review-run.json"));
        if manifest["review_assessment"].is_null() {
            self.accept(path);
        }
        self.ok(&["publish", "--effects", "review"])
    }

    pub fn export(&self, directory: &str) -> Profile {
        let directory = self.root.path().join(directory);
        self.ok(&["render-profile", "export", "--dir", text(&directory)]);
        let profile = Profile {
            manifest: directory.join("profile.json"),
        };
        assert!(profile.manifest.is_file());
        profile
    }

    /// Exports this build's profile with the previous default summary
    /// template; together they restore the profile that rkc 0.1.0 embeds.
    pub fn export_previous_default(&self, directory: &str) -> Profile {
        let profile = self.export(directory);
        profile.template("summary", PREVIOUS_DEFAULT_SUMMARY);
        profile
    }

    pub fn select(&self, profile: &Profile) {
        let mut config = read_json(&self.config);
        config["render_profile"] = json!(profile.manifest);
        write_json(&self.config, &config);
    }

    pub fn writes(&self) -> usize {
        self.http.state.lock().unwrap().writes().len()
    }

    pub fn summary(&self) -> String {
        self.http.state.lock().unwrap().notes[0]["body"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    pub fn previous_finding(&self, path: &Path, id: u64) -> Value {
        let context = review_kit_json_adapter::decode::<review_kit_json_adapter::ContextDocument>(
            &fs::read(path.with_file_name("review-context.json")).unwrap(),
        )
        .unwrap();
        let bundle = context.bundle().unwrap();
        let finding = bundle
            .findings()
            .iter()
            .find(|finding| finding.id().get() == id)
            .unwrap();
        serde_json::to_value(review_kit_json_adapter::domain::FindingData::from(finding)).unwrap()
    }

    pub fn followup(&self, name: &str, status: &str) -> (PathBuf, Value) {
        let (path, mut value) = self.prepare(name, &[ERROR_POLICY]);
        let previous = self.previous_finding(&path, 1);
        let mut finding = previous.clone();
        finding["status"] = json!(status);
        finding["evidence"] = json!(format!("Verified {status} state from current source."));
        value["findings"][0]["reassessment"] = finding;
        if previous["status"] != status {
            value["transitions"] = json!([{
                "finding":1,"from":previous["status"],"to":status,
                "body":format!("Current committed source establishes the {status} state.")
            }]);
        }
        if status == "open" {
            violated(&mut value);
        }
        (path, value)
    }
}

pub struct Profile {
    pub manifest: PathBuf,
}

impl Profile {
    pub fn asset(&self, role: &str, asset: &str) -> PathBuf {
        let manifest = read_json(&self.manifest);
        self.manifest
            .parent()
            .unwrap()
            .join(manifest["artifacts"][role][asset].as_str().unwrap())
    }

    pub fn template(&self, role: &str, template: &str) {
        fs::write(self.asset(role, "template"), template).unwrap();
    }

    pub fn prefix(&self, role: &str, prefix: &str) {
        let path = self.asset(role, "template");
        let body = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("{prefix}\n{body}")).unwrap();
    }

    pub fn weak_schemas(&self) {
        for role in ["finding", "summary", "transition", "answer", "suggestion"] {
            write_json(&self.asset(role, "schema"), &json!({"type":"object"}));
        }
    }
}

pub fn ready(value: &mut Value) {
    value["decision"] = json!("ready");
    value["scope"] = json!("Changed committed Rust source.");
    value["depth"] = json!("Checked failure handling and regression evidence.");
    value["local_context"] = json!("PRIVATE PROFILE CONTEXT");
    for policy in value["policies"].as_array_mut().unwrap() {
        policy["outcome"] = json!({"kind":"satisfied"});
    }
}

pub fn violated(value: &mut Value) {
    value["decision"] = json!("changes_required");
    for policy in value["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind":"violated","evidence":"The failure path discards the actionable error."});
        }
    }
}

pub fn finding() -> Value {
    json!({"id":1,"policy_id":ERROR_POLICY,"title":"Preserve the actionable error",
        "body":"The failure branch discards the cause when the operation fails, preventing callers from diagnosing it.",
        "severity":"p2","disposition":"blocking","status":"open",
        "evidence":"The committed failure branch returns without its error cause.","validated":true,
        "location":{"path":"src/lib.rs","line":2},"suggestion":null})
}

pub fn suggestion(start: u64, original: &[&str], replacement: &[&str]) -> Value {
    json!({"location":{"path":"src/lib.rs","line":2},
        "revision":{"head":"1".repeat(40),"comparison":["2".repeat(40),"3".repeat(40)]},
        "start_line":start,"original":original,"replacement":replacement,
        "evidence":"Committed source and the correction match the reviewed failure branch."})
}

#[path = "../shared/confirmations.rs"]
mod confirmations;

pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Write `value`; an assessment draft first records the confirmations an
/// isolated checker would return for its validated blocking findings.
///
/// Tests of admission itself write their exact bytes with [`std::fs::write`].
pub fn write_json(path: &Path, value: &Value) {
    let mut value = value.clone();
    if confirmations::is_assessment(&value) {
        confirmations::confirm_blocking(&mut value);
    }
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn git(directory: &Path, args: &[&str]) {
    let status = Process::new("git")
        .args([
            "-c",
            "user.name=Review Kit Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

pub fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

pub fn public_summary(body: &str) -> &str {
    body.split("\n\n<!-- review-kit:state:v3 ").next().unwrap()
}

pub fn accepted_inputs(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let directory = path.parent().unwrap();
    let manifest = read_json(&directory.join("review-run.json"));
    let mut files = vec![
        directory.join("review-run.json"),
        directory.join("review-render-profile.json"),
    ];
    let reference = manifest["review_assessment"].as_str().unwrap();
    files.push(
        directory
            .join("accepted")
            .join(reference.strip_prefix("sha256:").unwrap())
            .join("review-assessment.json"),
    );
    files
        .into_iter()
        .map(|file| {
            let bytes = fs::read(&file).unwrap();
            (file, bytes)
        })
        .collect()
}

pub fn assert_accepted_inputs_unchanged(inputs: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in inputs {
        assert_eq!(
            fs::read(path).unwrap(),
            *bytes,
            "{} changed",
            path.display()
        );
    }
}

pub fn assert_no_writes(fixture: &Fixture, before: usize, output: &Output, value: &Value) {
    assert!(!output.status.success(), "unexpected success: {value}");
    assert_eq!(
        fixture.writes(),
        before,
        "rejected input wrote to GitLab: {value}"
    );
}
