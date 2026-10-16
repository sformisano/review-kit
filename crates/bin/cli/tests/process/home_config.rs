use super::*;

fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rkc"));
    command
        .timeout(Duration::from_secs(10))
        .arg("--progress=none")
        .current_dir(&fixture.repo)
        .env("HOME", fixture.root.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .env_remove("GITLAB_TOKEN")
        .env_remove("REVIEW_KIT_REVIEWER_TOKEN");
    command
}

fn home_config(fixture: &Fixture, text: &str) -> PathBuf {
    let path = fixture.root.path().join(".review-kit/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

fn json_config(_fixture: &Fixture, xdg: &Path) -> PathBuf {
    let path = xdg.join("review-kit/config.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write(&path, &json!({"version": 1}));
    path
}

fn result(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn canonical_home_toml_loads_review_settings_outside_the_invocation_directory() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    home_config(
        &fixture,
        "version = 1\nreview_workflow = \"codebase-policies-review\"\n",
    );
    fs::create_dir(fixture.repo.join("unrelated")).unwrap();
    let output = command(&fixture)
        .current_dir(fixture.repo.join("unrelated"))
        .args(["check"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", result(&output));
    assert_eq!(result(&output)["status"], "valid");
    assert!(output.stderr.is_empty());
    assert!(
        fixture
            .root
            .path()
            .join(".review-kit/logs/review-kit.jsonl")
            .is_file()
    );
}

#[test]
fn automatic_config_selection_supports_xdg_and_home_config_roots() {
    for explicit_xdg in [false, true] {
        let fixture = Fixture::new();
        fixture.ready();
        fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
        let xdg = fixture
            .root
            .path()
            .join(if explicit_xdg { "xdg" } else { ".config" });
        json_config(&fixture, &xdg);
        let mut command = command(&fixture);
        if explicit_xdg {
            command.env("XDG_CONFIG_HOME", &xdg);
        }
        let output = command.args(["check"]).output().unwrap();
        assert!(output.status.success(), "{}", result(&output));
        assert_eq!(result(&output)["status"], "valid");
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn both_automatic_configs_require_explicit_selection_and_neither_is_loaded() {
    for explicit_xdg in [false, true] {
        let fixture = Fixture::new();
        fixture.ready();
        fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
        home_config(
            &fixture,
            "version = 1\nreview_workflow = \"PRIVATE_WORKFLOW\"\n",
        );
        let xdg = fixture
            .root
            .path()
            .join(if explicit_xdg { "xdg" } else { ".config" });
        let selected_json = json_config(&fixture, &xdg);
        let mut ambiguous = command(&fixture);
        if explicit_xdg {
            ambiguous.env("XDG_CONFIG_HOME", &xdg);
        }
        let output = ambiguous.args(["check"]).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", result(&output));
        assert_eq!(result(&output)["diagnostics"][0]["code"], "configuration");
        let diagnostic = result(&output)["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(diagnostic.contains("--config"), "{diagnostic}");
        for environment in [false, true] {
            let mut selected = command(&fixture);
            if explicit_xdg {
                selected.env("XDG_CONFIG_HOME", &xdg);
            }
            if environment {
                selected.env("RKC_CONFIG", &selected_json);
            } else {
                selected.arg("--config").arg(&selected_json);
            }
            let output = selected.args(["check"]).output().unwrap();
            assert!(output.status.success(), "{}", result(&output));
        }
    }
}

#[test]
fn explicit_json_and_toml_do_not_select_logging() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    home_config(&fixture, "[logging]\ndestination = \"stderr\"\n");
    let directory = fixture.root.path().join("selected");
    fs::create_dir(&directory).unwrap();
    for extension in ["json", "toml"] {
        let path = directory.join(format!("config.{extension}"));
        if extension == "json" {
            write(&path, &json!({"version": 1}));
        } else {
            fs::write(
                &path,
                "version = 1\n[logging]\ndirectory = \"PRIVATE_UNUSED_LOG_DIRECTORY\"\n",
            )
            .unwrap();
        }
        for environment in [false, true] {
            let mut selected = command(&fixture);
            if environment {
                selected.env("RKC_CONFIG", &path);
            } else {
                selected.arg("--config").arg(&path);
            }
            let output = selected.args(["check"]).output().unwrap();
            assert!(output.status.success(), "{}", result(&output));
            let records = super::logging::records(&output);
            assert!(records.iter().any(|r| r["operation"] == "cli.run"));
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("PRIVATE_UNUSED_LOG_DIRECTORY")
            );
            assert!(!fixture.root.path().join(".review-kit/logs").exists());
        }
    }
}

#[test]
fn config_flag_takes_precedence_over_config_environment() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    let output = command(&fixture)
        .env("RKC_CONFIG", fixture.root.path().join("absent.toml"))
        .arg("--config")
        .arg(&fixture.config)
        .args(["check"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", result(&output));
}

#[test]
fn no_home_can_use_xdg_config_with_stderr_logging() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    let xdg = fixture.root.path().join("xdg");
    json_config(&fixture, &xdg);
    let output = command(&fixture)
        .env_remove("HOME")
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["check"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", result(&output));
    assert!(
        super::logging::records(&output)
            .iter()
            .any(|r| r["operation"] == "cli.check")
    );
    assert!(!fixture.root.path().join(".review-kit").exists());
}

#[test]
fn neither_candidate_attempts_the_canonical_home_toml() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    let output = command(&fixture).args(["check"]).output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", result(&output));
    assert_eq!(result(&output)["diagnostics"][0]["code"], "filesystem");
    assert!(
        result(&output)["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains(".review-kit/config.toml")
    );
    assert!(
        fixture
            .root
            .path()
            .join(".review-kit/logs/review-kit.jsonl")
            .is_file()
    );
}

#[test]
fn invalid_review_fields_fail_while_valid_logging_remains_available() {
    for review in [
        "review_workflow = \"codebase-policies-review\"\n",
        "version = 2\n",
        "version = 1\nprivate_unknown = true\n",
        "version = 1\nreview_workflow = \"\"\n",
    ] {
        let fixture = Fixture::new();
        fixture.ready();
        fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
        home_config(
            &fixture,
            &format!("{review}[logging]\ndestination = \"stderr\"\n"),
        );
        let output = command(&fixture).args(["check"]).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", result(&output));
        assert!(!super::logging::records(&output).is_empty());
    }
}

#[test]
fn oversized_and_nonregular_home_configuration_do_not_block_help() {
    for directory in [false, true] {
        let fixture = Fixture::new();
        fixture.ready();
        fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
        let path = home_config(&fixture, "");
        if directory {
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        }
        let output = command(&fixture).arg("--help").output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(
            fixture
                .root
                .path()
                .join(".review-kit/logs/review-kit.jsonl")
                .is_file()
        );
    }
}

#[cfg(unix)]
#[test]
fn fifo_configuration_is_rejected_without_waiting_for_a_writer() {
    let fixture = Fixture::new();
    fixture.ready();
    fs::remove_dir_all(fixture.root.path().join(".review-kit")).unwrap();
    let path = home_config(&fixture, "");
    fs::remove_file(&path).unwrap();
    let created = std::process::Command::new("mkfifo")
        .arg(&path)
        .output()
        .unwrap();
    assert!(created.status.success());
    let output = command(&fixture).arg("--help").output().unwrap();
    assert!(output.status.success());
    let output = command(&fixture).args(["check"]).output().unwrap();
    assert!(!output.status.success());
    assert!(
        !result(&output)["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
