#![allow(clippy::unwrap_used)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct ChildGuard(std::process::Child);

impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(root: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rkc"));
    command
        .env("HOME", root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .env_remove("GITLAB_TOKEN")
        .env_remove("REVIEW_KIT_REVIEWER_TOKEN")
        .current_dir(root.path());
    command
}

fn export_writes_complete_progress_lines(root: &tempfile::TempDir, options: &[&str]) {
    let output = command(root)
        .args(options)
        .args(["render-profile", "export", "--dir", "profile"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "exported");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("rkc: Exporting rendering profile\n"));
    assert!(stderr.ends_with("rkc: Command finished\n"));
    assert!(!stderr.contains('\r'));
    assert!(!stderr.contains('\x1b'));
}

#[test]
fn redirected_auto_progress_writes_complete_lines() {
    let root = tempfile::tempdir().unwrap();
    export_writes_complete_progress_lines(&root, &["--progress=auto"]);
}

#[test]
fn default_progress_and_file_logging_keep_one_stdout_document() {
    let root = tempfile::tempdir().unwrap();
    export_writes_complete_progress_lines(&root, &[]);
    let logs = fs::read_to_string(root.path().join(".review-kit/logs/review-kit.jsonl")).unwrap();
    for line in logs.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        assert!(record["execution"].is_object());
    }
    assert!(logs.contains("cli.render_profile.export"));
    assert!(!logs.contains("rkc: "));
}

#[test]
fn none_keeps_errors_and_business_output() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".review-kit")).unwrap();
    fs::write(root.path().join(".review-kit/config.toml"), "version = 2\n").unwrap();
    let plain = command(&root)
        .args([
            "--progress=plain",
            "run",
            "start",
            "--mr",
            "https://gitlab.example.test/PRIVATE_POLICY/-/merge_requests/1",
        ])
        .output()
        .unwrap();
    let none = command(&root)
        .args([
            "run",
            "start",
            "--progress=none",
            "--mr",
            "https://gitlab.example.test/PRIVATE_POLICY/-/merge_requests/1",
        ])
        .output()
        .unwrap();
    assert_eq!(plain.status.code(), Some(2));
    assert_eq!(none.status.code(), plain.status.code());
    assert_eq!(
        serde_json::from_slice::<Value>(&none.stdout).unwrap(),
        serde_json::from_slice::<Value>(&plain.stdout).unwrap()
    );
    assert!(none.stderr.is_empty());
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert!(stderr.contains("Reading configuration"));
    assert!(!stderr.contains("PRIVATE_POLICY"));
}

#[test]
fn stderr_logs_remain_complete_json_records_alongside_progress() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".review-kit")).unwrap();
    fs::write(
        root.path().join(".review-kit/config.toml"),
        "[logging]\ndestination = \"stderr\"\n",
    )
    .unwrap();
    let output = command(&root)
        .args([
            "--progress=plain",
            "render-profile",
            "export",
            "--dir",
            "profile",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let _: Value = serde_json::from_slice(&output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let mut progress = 0;
    let mut logs = 0;
    for line in stderr.lines() {
        if line.starts_with("rkc: ") {
            progress += 1;
        } else {
            let record: Value = serde_json::from_str(line).unwrap();
            assert!(record["execution"].is_object());
            logs += 1;
        }
    }
    assert!(progress > 0 && logs > 0);
}

#[test]
fn default_progress_reports_flushed_activity_and_waiting_during_a_slow_provider_read() {
    slow_read(None, true);
}

#[test]
fn none_is_quiet_during_a_blocked_provider_read() {
    slow_read(Some("none"), false);
}

/// `mode` of `None` omits `--progress`.
fn slow_read(mode: Option<&str>, wait_for_heartbeat: bool) {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&worktree)
            .status()
            .unwrap()
            .success()
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let (requested_tx, requested_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let provider = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        requested_tx.send(first).unwrap();
        release_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    });
    let config = root.path().join("config.json");
    fs::write(&config, serde_json::to_vec(&json!({"version": 1, "gitlab": {"server": server, "token_env": "RKC_PROGRESS_TEST_TOKEN", "authority": []}})).unwrap()).unwrap();
    let mut rkc = command(&root);
    if let Some(mode) = mode {
        rkc.arg(format!("--progress={mode}"));
    }
    let mut child = ChildGuard(
        rkc.arg("--config")
            .arg(config)
            .env("RKC_PROGRESS_TEST_TOKEN", "PRIVATE_TEST_TOKEN")
            .current_dir(&worktree)
            .args([
                "run",
                "start",
                "--mr",
                &format!("{server}/private/PRIVATE_REPOSITORY/-/merge_requests/7"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let (line_tx, line_rx) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    let collector = thread::spawn(move || {
        let mut lines = String::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            let _ = line_tx.send(line.clone());
            lines.push_str(&line);
            lines.push('\n');
        }
        lines
    });
    let request = requested_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(request.starts_with("GET "));
    let mut heartbeat = false;
    let mut activity = false;
    if wait_for_heartbeat {
        while let Ok(line) = line_rx.recv_timeout(Duration::from_secs(15)) {
            activity |= line.contains("Connecting to GitLab");
            if line.contains("Still working:") {
                heartbeat = true;
                break;
            }
        }
    }
    let pending = child.try_wait().unwrap().is_none();
    release_tx.send(()).unwrap();
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    let status = child.wait().unwrap();
    provider.join().unwrap();
    let stderr = collector.join().unwrap();
    assert!(
        pending,
        "the provider read must still be pending during observation"
    );
    assert_eq!(status.code(), Some(7));
    let value: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(value["status"], "error");
    if wait_for_heartbeat {
        assert!(activity && heartbeat, "{stderr}");
    } else {
        assert!(stderr.is_empty(), "{stderr}");
    }
    for private in ["PRIVATE_TEST_TOKEN", "PRIVATE_REPOSITORY", &server] {
        assert!(!stderr.contains(private));
    }
    assert!(!stderr.contains('\r'));
    assert!(!stderr.contains('\x1b'));
}

#[test]
#[cfg(target_os = "linux")]
fn failing_stderr_does_not_replace_result_or_exit() {
    let root = tempfile::tempdir().unwrap();
    let healthy = command(&root)
        .args([
            "--progress=none",
            "run",
            "start",
            "--mr",
            "https://gitlab.example.test/team/project/-/merge_requests/1",
        ])
        .output()
        .unwrap();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let failed = command(&root)
        .args([
            "--progress=plain",
            "run",
            "start",
            "--mr",
            "https://gitlab.example.test/team/project/-/merge_requests/1",
        ])
        .stderr(full)
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), healthy.status.code());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap(),
        serde_json::from_slice::<Value>(&healthy.stdout).unwrap()
    );
}
