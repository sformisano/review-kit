use super::*;

pub(super) fn records(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stderr.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
        .collect()
}

#[test]
fn cli_logs_cover_internal_offline_work_without_changing_stdout() {
    let fixture = Fixture::stderr();
    fixture.prepare();
    fixture.fill_ready();
    let (output, result) = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(result["status"], "valid");
    let logs = records(&output);
    for operation in [
        "cli.run",
        "cli.parse",
        "cli.check",
        "config.load",
        "filesystem.runs.read_file",
        "filesystem.runs.read_json",
        "json.decode",
        "application.publication.check_review_with_progress",
        "application.validation.snapshot",
        "application.publication.build_plan",
        "application.render.summary",
        "cli.output",
    ] {
        assert!(
            logs.iter()
                .any(|r| r["operation"] == operation && r["phase"] == "started"),
            "missing {operation}"
        );
    }
    let starts: std::collections::HashMap<_, _> = logs
        .iter()
        .filter(|r| r["phase"] == "started")
        .map(|r| (r["execution_id"].as_str().unwrap(), r))
        .collect();
    for completed in logs.iter().filter(|r| r["phase"] == "completed") {
        assert!(starts.contains_key(completed["execution_id"].as_str().unwrap()));
        assert!(completed["duration_us"].is_u64());
        if let Some(parent) = completed["parent_execution_id"].as_str() {
            assert!(starts.contains_key(parent));
        }
    }
    let text = String::from_utf8(output.stderr).unwrap();
    for secret in [
        "PRIVATE TEST CONTEXT",
        "file:///policies/",
        "review-policy-list.md",
        "Focused review and proof.",
        "Changed Rust library.",
        "group/sub/repo",
        "127.0.0.1",
        fixture.root.path().to_str().unwrap(),
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
    assert_eq!(logs.last().unwrap()["output"]["exit"], 0);
}

#[test]
fn argument_and_configuration_errors_log_safe_categories() {
    let fixture = Fixture::stderr();
    let (output, result) = fixture.run(&["check", "--SECRET_ARGUMENT_VALUE"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(result["diagnostics"][0]["code"], "arguments");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_ARGUMENT_VALUE"));
    assert!(
        records(&output)
            .iter()
            .any(|r| r["error"]["variant"] == "invalid")
    );
    fs::write(&fixture.config, br#"{"version":"SECRET_CONFIG_VALUE"}"#).unwrap();
    let (output, result) = fixture.run(&[
        "run",
        "start",
        "--mr",
        "https://gitlab.example.test/SECRET_POLICY_VALUE/-/merge_requests/1",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(result["diagnostics"][0]["code"], "configuration");
    let logs = records(&output);
    assert!(
        logs.iter()
            .any(|r| r["operation"] == "config.load"
                && r["error"]["variant"] == "configuration_json")
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("SECRET_CONFIG_VALUE"));
    assert!(!stderr.contains("SECRET_POLICY_VALUE"));
}

#[test]
fn command_boundaries_include_configuration_failures() {
    let fixture = Fixture::stderr();
    fs::write(&fixture.config, br#"{"version":"PRIVATE_INVALID_CONFIG"}"#).unwrap();
    for (operation, args) in [
        ("cli.run.start", vec!["run", "start", "--mr", "PRIVATE_MR"]),
        ("cli.publish.run", vec!["publish", "--plan"]),
        ("cli.check", vec!["check"]),
        (
            "cli.publish.run",
            vec!["publish", "--abandon", "--reason", "PRIVATE_REASON"],
        ),
    ] {
        let (output, result) = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(result["diagnostics"][0]["code"], "configuration");
        let logs = records(&output);
        let command = logs
            .iter()
            .find(|r| r["operation"] == operation && r["phase"] == "started")
            .unwrap();
        let config = logs
            .iter()
            .find(|r| r["operation"] == "config.load" && r["phase"] == "started")
            .unwrap();
        assert_eq!(config["parent_execution_id"], command["execution_id"]);
        assert_eq!(
            logs.iter().filter(|r| r["operation"] == operation).count(),
            2
        );
        assert!(
            logs.iter()
                .any(|r| r["operation"] == operation && r["outcome"] == "rejected")
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_"));
    }
    let (output, result) = fixture.run(&["render-profile", "export", "--dir", "exported"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(result["status"], "exported");
    let logs = records(&output);
    assert!(
        logs.iter()
            .any(|r| r["operation"] == "cli.render_profile.export")
    );
    assert!(!logs.iter().any(|r| r["operation"] == "config.load"));
}

#[test]
#[cfg(target_os = "linux")]
fn failed_stderr_sink_preserves_cli_result_and_exit() {
    let fixture = Fixture::stderr();
    fixture.prepare();
    fixture.fill_ready();
    let (healthy, expected) = fixture.run(&["check"]);
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rkc"))
        .current_dir(&fixture.repo)
        .arg("--progress=none")
        .env("HOME", fixture.root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .arg("--config")
        .arg(&fixture.config)
        .args(["check"])
        .stderr(full)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), healthy.status.code());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
}

#[test]
#[cfg(target_os = "linux")]
fn failed_stdout_has_a_structured_error_and_existing_exit() {
    let fixture = Fixture::stderr();
    fixture.prepare();
    fixture.fill_ready();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rkc"))
        .current_dir(&fixture.repo)
        .env("HOME", fixture.root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .arg("--config")
        .arg(&fixture.config)
        .args(["check"])
        .stdout(full)
        .arg("--progress=none")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(
        records(&output)
            .iter()
            .any(|r| r["operation"] == "cli.output" && r["outcome"] == "failure")
    );
}

#[test]
fn help_and_version_keep_their_text_contract() {
    for flag in ["--help", "--version"] {
        let fixture = Fixture::stderr();
        let output = Command::new(env!("CARGO_BIN_EXE_rkc"))
            .env("HOME", fixture.root.path())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("RKC_CONFIG")
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("rkc"));
        assert_eq!(records(&output).last().unwrap()["output"]["exit"], 0);
    }
}
