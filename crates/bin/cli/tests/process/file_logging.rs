use super::*;

fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rkc"));
    command
        .timeout(Duration::from_secs(10))
        .arg("--progress=none")
        .current_dir(&fixture.repo)
        .env("HOME", fixture.root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG");
    command
}

fn log_path(fixture: &Fixture) -> PathBuf {
    fixture
        .root
        .path()
        .join(".review-kit/logs/review-kit.jsonl")
}

fn read_records(path: &Path) -> Vec<Value> {
    let bytes = fs::read_to_string(path).unwrap();
    assert!(!bytes.is_empty());
    assert!(bytes.ends_with('\n'));
    bytes
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
        .collect()
}

fn configure(fixture: &Fixture, text: &str) {
    fs::create_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    fs::write(fixture.root.path().join(".review-kit/config.toml"), text).unwrap();
}

fn bootstrap(records: &[Value]) -> &Value {
    &records
        .iter()
        .rev()
        .find(|record| record["operation"] == "cli.run" && record["phase"] == "started")
        .unwrap()["input"]["logging"]
}

#[test]
fn default_file_covers_help_version_and_invalid_arguments_before_parsing() {
    for (args, exit) in [
        (vec!["--help"], 0),
        (vec!["--version"], 0),
        (vec!["--PRIVATE_ARGUMENT_SENTINEL"], 2),
    ] {
        let fixture = Fixture::new();
        let output = command(&fixture).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(exit));
        assert!(output.stderr.is_empty());
        let records = read_records(&log_path(&fixture));
        assert_eq!(records.first().unwrap()["operation"], "cli.run");
        assert_eq!(records.first().unwrap()["phase"], "started");
        assert_eq!(records.last().unwrap()["output"]["exit"], exit);
        assert_eq!(bootstrap(&records)["destination"], "file");
        assert_eq!(bootstrap(&records)["configuration"], "default");
        assert_eq!(bootstrap(&records)["fallback"], false);
        let text = fs::read_to_string(log_path(&fixture)).unwrap();
        assert!(!text.contains("PRIVATE_ARGUMENT_SENTINEL"));
        assert!(!text.contains(fixture.root.path().to_str().unwrap()));
    }
}

#[test]
fn default_file_contains_offline_work_with_correlated_redacted_records() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture.fill_ready();
    let (output, result) = fixture.run(&["check"]);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["status"], "valid");
    assert!(output.stderr.is_empty());
    let records = read_records(&log_path(&fixture));
    for operation in [
        "cli.check",
        "config.load",
        "json.decode",
        "application.render.summary",
    ] {
        assert!(
            records
                .iter()
                .any(|r| r["operation"] == operation && r["phase"] == "started")
        );
    }
    let starts: std::collections::HashMap<_, _> = records
        .iter()
        .filter(|r| r["phase"] == "started")
        .map(|r| (r["execution_id"].as_str().unwrap(), r))
        .collect();
    for completed in records.iter().filter(|r| r["phase"] == "completed") {
        assert!(starts.contains_key(completed["execution_id"].as_str().unwrap()));
        assert!(completed["duration_us"].is_u64());
        if let Some(parent) = completed["parent_execution_id"].as_str() {
            assert!(starts.contains_key(parent));
        }
    }
    let text = fs::read_to_string(log_path(&fixture)).unwrap();
    for secret in [
        "PRIVATE TEST CONTEXT",
        "file:///policies/",
        "review-policy-list.md",
        "Focused review and proof.",
        "group/sub/repo",
        "127.0.0.1",
        fixture.root.path().to_str().unwrap(),
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
}

#[test]
fn configured_log_directories_resolve_from_the_home_independent_of_cwd() {
    for setting in ["custom/nested", "~/tilde-logs", "absolute"] {
        let fixture = Fixture::new();
        let (value, expected) = match setting {
            "absolute" => {
                let path = fixture.root.path().join("absolute-logs");
                (path.to_str().unwrap().to_owned(), path)
            }
            "~/tilde-logs" => (setting.to_owned(), fixture.root.path().join("tilde-logs")),
            _ => (
                setting.to_owned(),
                fixture.root.path().join(".review-kit/custom/nested"),
            ),
        };
        configure(&fixture, &format!("[logging]\ndirectory = {value:?}\n"));
        fs::create_dir(fixture.root.path().join("elsewhere")).unwrap();
        let output = command(&fixture)
            .current_dir(fixture.root.path().join("elsewhere"))
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            bootstrap(&read_records(&expected.join("review-kit.jsonl")))["destination"],
            "file"
        );
        assert_eq!(
            bootstrap(&read_records(&expected.join("review-kit.jsonl")))["configuration"],
            "configured"
        );
    }
}

#[test]
fn invalid_logging_settings_use_default_file_and_preserve_review_configuration() {
    for logging in [
        "[logging]\ndestination = \"PRIVATE_UNSUPPORTED_DESTINATION\"\n",
        "[logging]\ndirectory = 7\n",
        "[logging]\nunknown = \"PRIVATE_UNKNOWN_VALUE\"\n",
    ] {
        let fixture = Fixture::new();
        fixture.ready();
        configure(&fixture, &format!("version = 1\n{logging}"));
        let output = command(&fixture).args(["check"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.stderr.is_empty());
        let records = read_records(&log_path(&fixture));
        assert_eq!(bootstrap(&records)["destination"], "file");
        assert_eq!(bootstrap(&records)["fallback"], true);
        assert_eq!(bootstrap(&records)["configuration"], "invalid");
        assert!(
            !fs::read_to_string(log_path(&fixture))
                .unwrap()
                .contains("PRIVATE_")
        );
    }
}

#[test]
fn malformed_home_config_uses_safe_default_logging_before_the_command_error() {
    let fixture = Fixture::new();
    fixture.ready();
    configure(&fixture, "PRIVATE_INVALID_TOML = [\n");
    let output = command(&fixture).args(["check"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["diagnostics"][0]["code"], "configuration");
    let records = read_records(&log_path(&fixture));
    assert_eq!(bootstrap(&records)["destination"], "file");
    assert_eq!(bootstrap(&records)["fallback"], true);
    assert_eq!(bootstrap(&records)["configuration"], "invalid");
    let text = fs::read_to_string(log_path(&fixture)).unwrap();
    assert!(!text.contains("PRIVATE_INVALID_TOML"));
    assert!(!text.contains(fixture.root.path().to_str().unwrap()));
}

#[test]
fn missing_home_uses_stderr_for_bootstrap_and_keeps_version_text() {
    let fixture = Fixture::new();
    let output = command(&fixture)
        .env_remove("HOME")
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("rkc"));
    let records = super::logging::records(&output);
    assert_eq!(bootstrap(&records)["destination"], "stderr");
    assert_eq!(bootstrap(&records)["fallback"], true);
    assert_eq!(bootstrap(&records)["configuration"], "unavailable");
    assert!(!log_path(&fixture).exists());
}

#[test]
fn unavailable_log_destination_uses_stderr_without_changing_business_result() {
    let fixture = Fixture::new();
    fixture.prepare();
    fixture.fill_ready();
    let (_, expected) = fixture.run(&["check"]);
    fs::write(fixture.root.path().join("blocked"), "file").unwrap();
    configure(&fixture, "[logging]\ndirectory = \"../blocked/logs\"\n");
    let (output, result) = fixture.run(&["check"]);
    assert!(output.status.success());
    assert_eq!(result, expected);
    let records = super::logging::records(&output);
    assert_eq!(bootstrap(&records)["destination"], "stderr");
    assert_eq!(bootstrap(&records)["fallback"], true);
    assert_eq!(bootstrap(&records)["configuration"], "configured");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains(fixture.root.path().to_str().unwrap())
    );
}

#[cfg(unix)]
#[test]
fn final_symlink_and_nonregular_log_file_fall_back_without_touching_the_target() {
    for symlink in [false, true] {
        let fixture = Fixture::new();
        fs::create_dir_all(log_path(&fixture).parent().unwrap()).unwrap();
        let target = fixture.root.path().join("target");
        fs::write(&target, "KEEP TARGET").unwrap();
        if symlink {
            std::os::unix::fs::symlink(&target, log_path(&fixture)).unwrap();
        } else {
            fs::create_dir(log_path(&fixture)).unwrap();
        }
        let output = command(&fixture).arg("--version").output().unwrap();
        assert!(output.status.success());
        assert_eq!(fs::read_to_string(&target).unwrap(), "KEEP TARGET");
        let records = super::logging::records(&output);
        assert_eq!(bootstrap(&records)["destination"], "stderr");
        assert_eq!(bootstrap(&records)["fallback"], true);
    }
}

#[cfg(unix)]
#[test]
fn new_home_log_directories_and_file_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let output = command(&fixture).arg("--version").output().unwrap();
    assert!(output.status.success());
    for directory in [
        fixture.root.path().join(".review-kit"),
        fixture.root.path().join(".review-kit/logs"),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::metadata(log_path(&fixture))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn concurrent_process_appends_keep_complete_records_and_prior_history() {
    let fixture = Fixture::new();
    let initial = command(&fixture).arg("--version").output().unwrap();
    assert!(initial.status.success());
    let before = fs::read(log_path(&fixture)).unwrap();
    let barrier = std::sync::Barrier::new(12);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..12)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let output = command(&fixture).arg("--help").output().unwrap();
                    assert!(output.status.success());
                    assert!(output.stderr.is_empty());
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    assert!(fs::read(log_path(&fixture)).unwrap().starts_with(&before));
    let records = read_records(&log_path(&fixture));
    let starts: std::collections::HashSet<_> = records
        .iter()
        .filter(|r| r["operation"] == "cli.run" && r["phase"] == "started")
        .map(|r| r["execution_id"].as_str().unwrap())
        .collect();
    assert_eq!(starts.len(), 13);
    let completions: Vec<_> = records
        .iter()
        .filter(|r| r["operation"] == "cli.run" && r["phase"] == "completed")
        .collect();
    assert_eq!(completions.len(), 13);
    assert!(
        completions
            .iter()
            .all(|r| starts.contains(r["execution_id"].as_str().unwrap())
                && r["output"]["exit"] == 0)
    );
}
