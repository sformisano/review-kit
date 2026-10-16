#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "The shared HTTP fixture supplies real adapter requests and provider objects."
)]
mod support;

use support as server;
#[path = "render_profiles/fixture.rs"]
#[allow(
    dead_code,
    reason = "The CLI fixture also provides rendering-profile test helpers."
)]
mod fixture;

use std::{
    fs, io,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use review_kit_application::{
    ports::PortError,
    publication::{Effects, RoundStatus, Write},
    use_cases::{
        publication_run::{ExecutionEvent, ExecutionEvidence},
        publishing::{WritePhase, publish_recorded},
    },
};
use review_kit_domain::FindingStatus;
use review_kit_filesystem_adapter::{FileEvidence, FileProgressStore, Workspace};
use review_kit_gitlab_adapter::{Credential, GitLab, GitLabConfig};
use review_kit_observation::{Observer, OperationEvent, with_observer};
use serde_json::{Value, json};
use support::{Fault, Fixture, MR_PATH, Renderer, TOKEN, finding, request};

fn kill_worker() -> ! {
    let status = Command::new("kill")
        .args(["-KILL", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    std::process::exit(98)
}

struct CrashEvidence {
    inner: FileEvidence<'static>,
    boundary: String,
    prepared: Arc<AtomicBool>,
}

fn third(write: &Write) -> bool {
    matches!(write, Write::Finding { finding, .. } if finding.id().get() == 3)
}

impl ExecutionEvidence for CrashEvidence {
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        let phase = match &event {
            ExecutionEvent::Progress { progress, .. } => progress
                .unresolved
                .as_ref()
                .filter(|pending| third(&pending.write))
                .map(|pending| pending.phase),
            _ => None,
        };
        let third_readback =
            matches!(&event, ExecutionEvent::Readback { write, .. } if third(write));
        let final_readback = matches!(&event, ExecutionEvent::Readback { write: Write::Summary { round, .. }, .. } if round.status == RoundStatus::Completed);
        if phase == Some(WritePhase::Prepared) && self.boundary == "before_prepared" {
            kill_worker();
        }
        self.inner.record(event)?;
        if phase == Some(WritePhase::Prepared) {
            self.prepared.store(true, Ordering::SeqCst);
            if self.boundary == "after_prepared" {
                kill_worker();
            }
        }
        if phase == Some(WritePhase::DispatchPossible)
            && self.boundary == "after_dispatch_checkpoint"
        {
            kill_worker();
        }
        if third_readback && self.boundary == "after_finding_readback" {
            kill_worker();
        }
        if final_readback && self.boundary == "after_final_summary_readback" {
            kill_worker();
        }
        Ok(())
    }
}

#[test]
#[ignore = "Invoked only by the parent test as a fresh publication process."]
fn publication_worker() {
    let server = std::env::var("RKC_RESTART_SERVER").unwrap();
    let root = std::env::var("RKC_RESTART_ROOT").unwrap();
    let boundary = std::env::var("RKC_RESTART_BOUNDARY").unwrap();
    let client = GitLab::connect(
        GitLabConfig::new(&server, Credential::private_token(TOKEN).unwrap()).unwrap(),
        &format!("{server}/group/sub/repo/-/merge_requests/7"),
    )
    .unwrap();
    let request = request(
        &client,
        vec![
            finding(1, "src/lib.rs", 2, FindingStatus::Open, None),
            finding(2, "src/new.rs", 1, FindingStatus::Open, None),
            finding(3, "src/new.rs", 2, FindingStatus::Open, None),
        ],
    );
    let workspace = Workspace::new(Path::new(&root).join("review"));
    let store = FileProgressStore::new(
        Path::new(&root).join("state"),
        client.subject(),
        client.reviewer(),
    );
    let _lock = store.try_lock().unwrap();
    let mut progress = store.load(&workspace).unwrap();
    let prepared = Arc::new(AtomicBool::new(false));
    let flag = prepared.clone();
    let preflight_crash = boundary == "during_adapter_preflight";
    let observer = Observer::json(io::sink()).with_listener(move |event| {
        if preflight_crash
            && flag.load(Ordering::SeqCst)
            && matches!(
                event,
                OperationEvent::Completed {
                    operation: "gitlab.http.read",
                    ..
                }
            )
        {
            kill_worker();
        }
    });
    let mut evidence = CrashEvidence {
        inner: FileEvidence::with_progress_store(&workspace, &store).unwrap(),
        boundary,
        prepared,
    };
    let result = with_observer(&observer, || {
        publish_recorded(
            &request,
            &mut client.publication_host(Effects::review()),
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
            &mut evidence,
        )
    });
    store.save(&progress).unwrap();
    let mut document = match result {
        Ok(result) => json!({
        "status": format!("{:?}", result.status), "round": result.round,
        "attempted": result.attempted.len(), "retried_creations": result.retried_creations.len(),
        }),
        Err(_) => {
            json!({"status": "Rejected", "attempted": 0, "round": progress.authorization.as_ref().map(|grant| grant.round)})
        }
    };
    document["unresolved"] = json!(progress.unresolved.is_some());
    document["retained_retries"] = json!(progress.retried_creations.len());
    fs::write(
        Path::new(&root).join("worker-result.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
}

fn child(fixture: &Fixture, root: &Path, boundary: &str) -> std::process::Output {
    let mut command = assert_cmd::Command::new(std::env::current_exe().unwrap());
    command
        .timeout(std::time::Duration::from_secs(60))
        .args(["--exact", "publication_worker", "--ignored", "--nocapture"])
        .env("RKC_RESTART_SERVER", &fixture.server)
        .env("RKC_RESTART_ROOT", root)
        .env("RKC_RESTART_BOUNDARY", boundary);
    command.output().unwrap()
}

fn result(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("worker-result.json")).unwrap()).unwrap()
}

#[test]
fn sigkill_boundaries_resume_same_round_and_objects_in_fresh_processes() {
    use std::os::unix::process::ExitStatusExt;
    for (boundary, roots_before, phase) in [
        ("before_prepared", 2, None),
        ("after_prepared", 2, Some("prepared")),
        ("during_adapter_preflight", 2, Some("prepared")),
        ("after_dispatch_checkpoint", 2, Some("dispatch_possible")),
        ("after_finding_readback", 3, Some("dispatch_possible")),
        ("after_final_summary_readback", 3, Some("dispatch_possible")),
    ] {
        let fixture = Fixture::new();
        let root = tempfile::tempdir().unwrap();
        let killed = child(&fixture, root.path(), boundary);
        assert_eq!(
            killed.status.signal(),
            Some(9),
            "{boundary}: {}",
            String::from_utf8_lossy(&killed.stderr)
        );
        let client = fixture.connect();
        let store = FileProgressStore::new(
            root.path().join("state"),
            client.subject(),
            client.reviewer(),
        );
        let retained: Value = serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        match phase {
            Some(phase) => assert_eq!(
                retained["unresolved"]["phase"], phase,
                "{boundary}: {retained}"
            ),
            None => assert!(retained["unresolved"].is_null(), "{boundary}: {retained}"),
        }
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.notes.len(), 1, "{boundary}");
        assert_eq!(state.discussions.len(), roots_before, "{boundary}");
        let summary_id = state.notes[0]["id"].clone();
        let roots = state.discussions.clone();
        drop(state);
        let resumed = child(&fixture, root.path(), "resume");
        assert!(
            resumed.status.success(),
            "{boundary}: {}",
            String::from_utf8_lossy(&resumed.stderr)
        );
        let report = result(root.path());
        let expected = if boundary == "after_final_summary_readback" {
            "Unchanged"
        } else {
            "Completed"
        };
        assert_eq!(report["status"], expected, "{boundary}: {report}");
        assert_eq!(report["round"], 1);
        assert_eq!(report["unresolved"], false);
        assert_eq!(
            report["retained_retries"],
            usize::from(boundary == "after_dispatch_checkpoint")
        );
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.notes.len(), 1);
        assert_eq!(state.notes[0]["id"], summary_id);
        assert_eq!(&state.discussions[..roots_before], &roots);
        assert_eq!(state.discussions.len(), 3);
        assert_eq!(
            state
                .writes()
                .iter()
                .filter(|write| write.method == "POST" && write.path.ends_with("/discussions"))
                .count(),
            3
        );
        let count = state.writes().len();
        drop(state);
        let rerun = child(&fixture, root.path(), "resume");
        assert!(rerun.status.success());
        assert_eq!(result(root.path())["status"], "Unchanged");
        assert_eq!(result(root.path())["round"], 1);
        assert_eq!(fixture.state.lock().unwrap().writes().len(), count);
    }
}

#[test]
fn a_rejected_retry_keeps_the_creation_budget_spent_across_processes() {
    use std::os::unix::process::ExitStatusExt;
    let fixture = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let killed = child(&fixture, root.path(), "after_dispatch_checkpoint");
    assert_eq!(killed.status.signal(), Some(9));
    fixture.state.lock().unwrap().faults.push(Fault::response(
        "POST",
        &format!("{MR_PATH}/discussions"),
        support::Response::status(400),
    ));
    let retry = child(&fixture, root.path(), "resume");
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_eq!(result(root.path())["status"], "Uncertain");
    assert_eq!(result(root.path())["retained_retries"], 1);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 2);
    let writes = state.writes().len();
    drop(state);
    for _ in 0..2 {
        let blocked = child(&fixture, root.path(), "resume");
        assert!(blocked.status.success());
        assert_eq!(result(root.path())["status"], "Uncertain");
        assert_eq!(result(root.path())["attempted"], 0);
        assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
    }
}

#[test]
fn cli_offline_check_preserves_inputs_while_publication_commands_refuse_a_held_lock() {
    let fixture = fixture::Fixture::new();
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let manifest = fs::read(path.with_file_name("review-run.json")).unwrap();
    let draft = fs::read(&path).unwrap();
    let parsed: Value = serde_json::from_slice(&manifest).unwrap();
    let accepted_path = path
        .parent()
        .unwrap()
        .join("accepted")
        .join(
            parsed["review_assessment"]
                .as_str()
                .unwrap()
                .strip_prefix("sha256:")
                .unwrap(),
        )
        .join("review-assessment.json");
    let accepted = fs::read(&accepted_path).unwrap();
    let client = fixture.http.connect();
    let store = FileProgressStore::new(
        fixture.root.path().join("config/state"),
        client.subject(),
        client.reviewer(),
    );
    let guard = store.try_lock().unwrap();
    let before = fixture.http.state.lock().unwrap().requests.len();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    assert_eq!(checked["status"], "valid");
    assert_eq!(
        fs::read(path.with_file_name("review-run.json")).unwrap(),
        manifest
    );
    assert_eq!(fs::read(&path).unwrap(), draft);
    assert_eq!(fs::read(&accepted_path).unwrap(), accepted);
    assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
    for args in [
        vec!["publish", "--plan"],
        vec!["publish", "--effects", "review"],
        vec!["publish", "--abandon", "--reason", "Close pending review"],
    ] {
        let (output, result) = fixture.run(&args);
        assert_eq!(output.status.code(), Some(5), "{result}");
        assert_eq!(result["diagnostics"][0]["code"], "publication_busy");
        assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
        assert_eq!(
            fs::read(path.with_file_name("review-run.json")).unwrap(),
            manifest
        );
        assert_eq!(fs::read(&path).unwrap(), draft);
        assert_eq!(fs::read(&accepted_path).unwrap(), accepted);
    }
    assert_eq!(fixture.writes(), 0);
    drop(guard);
    let published = fixture.publish(&path);
    assert_eq!(published["run"]["outcome"]["result"]["status"], "completed");
}

#[test]
fn a_late_original_creation_conflicts_after_one_resend_without_more_writes() {
    let fixture = Fixture::with_lost_responses();
    let root = tempfile::tempdir().unwrap();
    let target = format!("{MR_PATH}/notes");
    fixture
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&target, false));
    let first = child(&fixture, root.path(), "resume");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(result(root.path())["status"], "Uncertain");
    let mut state = fixture.state.lock().unwrap();
    assert!(state.notes.is_empty());
    let original = state
        .writes()
        .into_iter()
        .find(|request| request.path == target)
        .unwrap()
        .clone();
    state
        .faults
        .push(Fault::late_creation(&target, original.clone()));
    drop(state);
    let resend = child(&fixture, root.path(), "resume");
    assert!(
        resend.status.success(),
        "{}",
        String::from_utf8_lossy(&resend.stderr)
    );
    assert_eq!(result(root.path())["status"], "Uncertain");
    assert_eq!(result(root.path())["retained_retries"], 1);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.notes.len(), 2);
    assert_ne!(state.notes[0]["id"], state.notes[1]["id"]);
    assert_eq!(state.notes[0]["body"], state.notes[1]["body"]);
    assert_eq!(state.writes().len(), 2);
    assert_eq!(state.writes()[1].payload, original.payload);
    let notes = state.notes.clone();
    drop(state);
    for _ in 0..2 {
        let blocked = child(&fixture, root.path(), "resume");
        assert!(
            blocked.status.success(),
            "{}",
            String::from_utf8_lossy(&blocked.stderr)
        );
        assert_eq!(result(root.path())["status"], "Rejected");
        assert_eq!(result(root.path())["retained_retries"], 1);
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.writes().len(), 2);
        assert_eq!(state.notes, notes);
    }
}
