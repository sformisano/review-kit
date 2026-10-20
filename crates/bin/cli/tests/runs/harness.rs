//! Process harness: a real temporary Git worktree, trusted configuration and the
//! controlled HTTP GitLab fixture. Every command runs the built `rkc` binary.
#![allow(dead_code, reason = "Each test binary uses a subset of the harness.")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as Process, Output},
    time::Duration,
};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::server;

pub const TOKEN_ENV: &str = "RKC_RUNS_TEST_TOKEN";
pub const ERROR_POLICY: &str = "rust-error-contracts";
pub const PROOF_POLICY: &str = "baseline-proof-policy";
pub const POLICY_SET: &str = "rust-codebase-policies";

#[path = "../shared/confirmations.rs"]
mod confirmations;

pub struct Harness {
    pub root: TempDir,
    pub http: server::Fixture,
    /// The GitLab base URL in configuration; the fixture itself unless a proxy fronts it.
    pub server: String,
    pub config: PathBuf,
    pub repo: PathBuf,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_http(server::Fixture::new())
    }

    pub fn lost_responses() -> Self {
        Self::with_http(server::Fixture::with_lost_responses())
    }

    pub fn with_http(http: server::Fixture) -> Self {
        let server = http.server.clone();
        Self::with_server(http, server)
    }

    /// A harness whose configuration names `server`, such as a proxy in front of `http`.
    pub fn with_server(http: server::Fixture, server: String) -> Self {
        let root = tempfile::Builder::new()
            .prefix("rkc-runs-")
            .tempdir()
            .unwrap();
        let config = root.path().join("config/config.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        write_json(
            &config,
            &json!({
                "version": 1,
                "publication_state": "state",
                "review_workflow": "codebase-policies-review",
                "gitlab": {"server": server, "token_env": TOKEN_ENV,
                    "authority": ["create_summary","update_summary","finding","reply","resolve","reopen"]}
            }),
        );
        let repo = root.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        Self {
            root,
            http,
            server,
            config,
            repo,
        }
    }

    pub fn mr(&self) -> String {
        self.mr_number(7)
    }

    pub fn mr_number(&self, request: u64) -> String {
        format!("{}/group/sub/repo/-/merge_requests/{request}", self.server)
    }

    pub fn state(&self) -> PathBuf {
        self.root.path().join("config/state")
    }

    /// Run `rkc` in `directory` with this harness's configuration and credential.
    pub fn rkc_in(&self, directory: &Path, args: &[&str]) -> (Output, Value) {
        self.rkc_env(directory, args, &[])
    }

    #[allow(clippy::panic, reason = "Invalid child JSON is a fixture failure.")]
    pub fn rkc_env(
        &self,
        directory: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> (Output, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rkc"));
        command
            .timeout(Duration::from_secs(90))
            .arg("--progress")
            .arg("none")
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(directory)
            .env("HOME", self.root.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("RKC_CONFIG")
            .env_remove("RKC_FAULT")
            .env_remove("GITLAB_TOKEN")
            .env(TOKEN_ENV, server::TOKEN);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        let value = if output.stdout.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "rkc returned invalid JSON: {error}; stdout={}; stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            })
        };
        assert!(!String::from_utf8_lossy(&output.stdout).contains(server::TOKEN));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(server::TOKEN));
        (output, value)
    }

    pub fn rkc(&self, args: &[&str]) -> (Output, Value) {
        self.rkc_in(&self.repo, args)
    }

    pub fn ok(&self, args: &[&str]) -> Value {
        self.ok_in(&self.repo, args)
    }

    pub fn ok_in(&self, directory: &Path, args: &[&str]) -> Value {
        let (output, value) = self.rkc_in(directory, args);
        assert!(output.status.success(), "{args:?} failed: {value:#}");
        value
    }

    /// Assert the exit code and the first diagnostic code.
    pub fn fails(&self, args: &[&str], exit: i32, code: &str) -> Value {
        self.fails_in(&self.repo, args, exit, code)
    }

    pub fn fails_in(&self, directory: &Path, args: &[&str], exit: i32, code: &str) -> Value {
        let (output, value) = self.rkc_in(directory, args);
        assert_eq!(output.status.code(), Some(exit), "{args:?}: {value:#}");
        assert_eq!(first_code(&value), code, "{args:?}: {value:#}");
        value
    }

    pub fn start(&self) -> String {
        let started = self.ok(&["run", "start", "--mr", &self.mr()]);
        assert_eq!(started["status"], "created", "{started}");
        started["run_id"].as_str().unwrap().to_owned()
    }

    pub fn runtime(&self) -> PathBuf {
        self.repo.join(".review-kit")
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runtime().join("review-runs").join(run_id)
    }

    pub fn file(&self, run_id: &str, name: &str) -> PathBuf {
        self.run_dir(run_id).join(name)
    }

    pub fn manifest(&self, run_id: &str) -> Value {
        read_json(&self.file(run_id, "review-run.json"))
    }

    pub fn selected(&self) -> String {
        read_json(&self.runtime().join("review-workspace.json"))["selected_run_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    pub fn declare_policies(&self, run_id: &str, policies: &[&str]) {
        let path = self.file(run_id, "review-assessment.draft.json");
        let mut draft = read_json(&path);
        draft["policy_set"] = json!(POLICY_SET);
        draft["policies"] = json!(
            policies
                .iter()
                .map(|policy| json!({
                    "policy_id": policy,
                    "name": format!("Policy {policy}"),
                    "required": *policy == PROOF_POLICY,
                    "reason": "The changed Rust error path falls under this policy.",
                    "outcome": null,
                }))
                .collect::<Vec<_>>()
        );
        write_json(&path, &draft);
        self.seed_carried(&self.repo, run_id);
    }

    pub fn fill_ready(&self, run_id: &str) {
        let path = self.file(run_id, "review-assessment.draft.json");
        let mut draft = read_json(&path);
        ready(&mut draft);
        write_json(&path, &draft);
    }

    pub fn fill_finding(&self, run_id: &str) {
        let path = self.file(run_id, "review-assessment.draft.json");
        let mut draft = read_json(&path);
        ready(&mut draft);
        draft["new_findings"] = json!([finding()]);
        draft["decision"] = json!("changes_required");
        for policy in draft["policies"].as_array_mut().unwrap() {
            if policy["policy_id"] == ERROR_POLICY {
                policy["outcome"] = json!({"kind":"violated","evidence":"The failure path discards the actionable error."});
            }
        }
        write_json(&path, &draft);
    }

    /// Start a run with explicit selected policies and captured carried-finding data.
    pub fn draft_with_policies(&self, policies: &[&str]) -> String {
        let run_id = self.start();
        self.declare_policies(&run_id, policies);
        run_id
    }

    fn seed_carried(&self, directory: &Path, run_id: &str) {
        let run = directory.join(".review-kit/review-runs").join(run_id);
        let context = review_kit_json_adapter::decode::<review_kit_json_adapter::ContextDocument>(
            &fs::read(run.join("review-context.json")).unwrap(),
        )
        .unwrap();
        let bundle = context.bundle().unwrap();
        let path = run.join("review-assessment.draft.json");
        let mut draft = read_json(&path);
        for slot in draft["findings"].as_array_mut().unwrap() {
            let finding = bundle
                .findings()
                .iter()
                .find(|finding| Some(finding.id().get()) == slot["id"].as_u64())
                .unwrap();
            slot["reassessment"] =
                serde_json::to_value(review_kit_json_adapter::domain::FindingData::from(finding))
                    .unwrap();
        }
        write_json(&path, &draft);
    }

    /// An accepted assessment with one blocking finding.
    pub fn accepted_with_finding(&self) -> String {
        let run_id = self.draft_with_policies(&[ERROR_POLICY, PROOF_POLICY]);
        self.fill_finding(&run_id);
        let accepted = self.ok(&["accept"]);
        assert_eq!(accepted["status"], "stored", "{accepted}");
        run_id
    }

    /// An accepted ready assessment, started with `--new` when requested.
    pub fn ready_run(&self, new: bool) -> String {
        let run_id = if new {
            self.ok(&["run", "start", "--new"])["run_id"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            self.start()
        };
        self.declare_policies(&run_id, &[ERROR_POLICY]);
        self.fill_ready(&run_id);
        self.ok(&["accept"]);
        run_id
    }

    /// Reassess every carried finding as still open with fresh evidence.
    pub fn fill_carried_open(&self, run_id: &str) {
        let path = self.file(run_id, "review-assessment.draft.json");
        let mut draft = read_json(&path);
        carried_open(&mut draft);
        write_json(&path, &draft);
    }

    /// Start a run in `directory`'s worktree that keeps carried findings open, and accept it.
    pub fn follow_up_in(&self, directory: &Path, new: bool) -> String {
        let mr = self.mr();
        let mut args = vec!["run", "start"];
        if new {
            args.push("--new");
        } else {
            args.extend(["--mr", mr.as_str()]);
        }
        let run_id = self.ok_in(directory, &args)["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let run = directory.join(".review-kit/review-runs").join(&run_id);
        let assessment = run.join("review-assessment.draft.json");
        let mut draft = read_json(&assessment);
        draft["policy_set"] = json!(POLICY_SET);
        draft["policies"] = json!([{
            "policy_id": ERROR_POLICY, "name": format!("Policy {ERROR_POLICY}"),
            "required": false,
            "reason": "The carried finding uses this policy.", "outcome": null,
        }]);
        write_json(&assessment, &draft);
        self.seed_carried(directory, &run_id);
        let assessment = run.join("review-assessment.draft.json");
        let mut draft = read_json(&assessment);
        carried_open(&mut draft);
        write_json(&assessment, &draft);
        self.ok_in(directory, &["accept"]);
        run_id
    }

    /// A new run with one blocking finding, started with `--new` beside an open run.
    /// Capture and accept a follow-up that transitions finding 1 `from` → `to`.
    pub fn transition_follow_up(&self, from: &str, to: &str) -> String {
        let run_id = self.start();
        self.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
        let path = self.file(&run_id, "review-assessment.draft.json");
        let mut draft = read_json(&path);
        ready(&mut draft);
        let slot = &mut draft["findings"][0]["reassessment"];
        slot["status"] = json!(to);
        slot["evidence"] = json!(format!("The current source establishes the {to} state."));
        draft["transitions"] = json!([{"finding": 1, "from": from, "to": to,
            "body": format!("The committed change makes this finding {to}.")}]);
        if to == "open" {
            draft["decision"] = json!("changes_required");
            for policy in draft["policies"].as_array_mut().unwrap() {
                if policy["policy_id"] == ERROR_POLICY {
                    policy["outcome"] = json!({"kind": "violated", "evidence": "The failure path discards the cause again."});
                }
            }
        }
        write_json(&path, &draft);
        self.ok(&["accept"]);
        run_id
    }

    pub fn accepted_follow_up(&self) -> String {
        let run_id = self.ok(&["run", "start", "--new"])["run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
        self.fill_finding(&run_id);
        self.ok(&["accept"]);
        run_id
    }

    /// Advance the merge request to a new head whose files and diff are unchanged.
    pub fn push_commit(&self) {
        let mut state = self.http.state.lock().unwrap();
        state.retain_revision();
        state.historical_head = Some(state.head.clone());
        state.head = "4".repeat(40);
    }

    /// Drop the confirmation-enforcement marks from retained review state, as a build
    /// without confirmation records writes it. Returns the number of edited notes.
    pub fn forget_confirmation_records(&self) -> usize {
        let mut state = self.http.state.lock().unwrap();
        let state = &mut *state;
        let mut edited = 0;
        let notes = state.notes.iter_mut().chain(
            state
                .discussions
                .iter_mut()
                .filter_map(|discussion| discussion["notes"].as_array_mut())
                .flatten(),
        );
        for note in notes {
            let Some(body) = note["body"].as_str() else {
                continue;
            };
            let stripped = body.replace(",\"confirmed_validation\":true", "");
            if stripped != body {
                note["body"] = json!(stripped);
                edited += 1;
            }
        }
        edited
    }

    /// Add a general discussion by another user.
    pub fn add_comment(&self, id: &str, body: &str) {
        let mut state = self.http.state.lock().unwrap();
        let note = state.note(body, 77);
        state
            .discussions
            .push(json!({"id": id, "individual_note": true, "notes": [note]}));
    }

    pub fn writes(&self) -> usize {
        self.http.state.lock().unwrap().writes().len()
    }

    /// Provider writes with this method and path suffix.
    pub fn writes_to(&self, method: &str, suffix: &str) -> usize {
        self.http
            .state
            .lock()
            .unwrap()
            .writes()
            .iter()
            .filter(|request| request.method == method && request.path.ends_with(suffix))
            .count()
    }

    pub fn summaries(&self) -> usize {
        self.http.state.lock().unwrap().notes.len()
    }

    pub fn discussions(&self) -> usize {
        self.http.state.lock().unwrap().discussions.len()
    }

    /// The single review-key journal directory under the configured state root.
    pub fn journal_dir(&self) -> Option<PathBuf> {
        let entries = fs::read_dir(self.state()).ok()?;
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.is_dir())
    }

    /// A run file decoded as JSON.
    pub fn run_file_json(&self, run_id: &str, name: &str) -> Value {
        read_json(&self.file(run_id, name))
    }

    /// Replace the run's conversation with `document` and record its hash in
    /// the manifest and in any journal entry that names the run's inputs, as
    /// the rkc that wrote such a run would have.
    pub fn retain_conversation(&self, run_id: &str, document: &Value) {
        let bytes = serde_json::to_vec_pretty(document).unwrap();
        fs::write(self.file(run_id, "review-conversation.json"), &bytes).unwrap();
        let manifest = self.file(run_id, "review-run.json");
        let mut value = read_json(&manifest);
        let previous = value["snapshot_files"]["review-conversation.json"]
            .as_str()
            .unwrap()
            .to_owned();
        let current = sha256(&bytes);
        value["snapshot_files"]["review-conversation.json"] = json!(current);
        fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        if let Some(directory) = self.journal_dir() {
            let journal = directory.join("publication-progress.json");
            let text = fs::read_to_string(&journal).unwrap();
            fs::write(&journal, text.replace(&previous, &current)).unwrap();
        }
    }

    pub fn journal(&self) -> Value {
        read_json(
            &self
                .journal_dir()
                .unwrap()
                .join("publication-progress.json"),
        )
    }
}

pub fn first_code(value: &Value) -> String {
    value["diagnostics"][0]["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

pub fn git(directory: &Path, args: &[&str]) {
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

/// Complete an assessment that keeps every carried finding open with fresh evidence.
pub fn carried_open(draft: &mut Value) {
    ready(draft);
    let mut carried = false;
    for slot in draft["findings"].as_array_mut().unwrap() {
        let mut reassessment = slot["reassessment"].clone();
        reassessment["evidence"] = json!("The current source still discards the cause.");
        slot["reassessment"] = reassessment;
        carried = true;
    }
    if carried {
        draft["decision"] = json!("changes_required");
        for policy in draft["policies"].as_array_mut().unwrap() {
            if policy["policy_id"] == ERROR_POLICY {
                policy["outcome"] = json!({"kind":"violated","evidence":"The failure path still discards the actionable error."});
            }
        }
    }
}

pub fn ready(value: &mut Value) {
    value["decision"] = json!("ready");
    value["scope"] = json!("Changed committed Rust source.");
    value["depth"] = json!("Checked failure handling and regression evidence.");
    value["local_context"] = json!("PRIVATE RUN CONTEXT");
    for policy in value["policies"].as_array_mut().unwrap() {
        if !matches!(
            policy["outcome"]["kind"].as_str(),
            Some("excluded" | "unresolved")
        ) {
            policy["outcome"] = json!({"kind":"satisfied"});
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

#[allow(clippy::panic, reason = "A missing fixture file is a test failure.")]
pub fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes).unwrap()
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

pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("sha256:{:x}", sha2::Sha256::digest(bytes))
}

/// Copy a directory tree, such as a run directory restored from a backup.
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
