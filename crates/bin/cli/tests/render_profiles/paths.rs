use std::{fs, time::Duration};

use assert_cmd::Command;
use serde_json::json;

use super::fixture::{Fixture, assert_no_writes, read_json, text, write_json};

#[test]
fn profile_export_needs_no_configuration_credentials_or_source_checkout() {
    let root = tempfile::Builder::new()
        .prefix("rkc-offline-export-")
        .tempdir()
        .unwrap();
    let destination = root.path().join("exported");
    let output = Command::new(env!("CARGO_BIN_EXE_rkc"))
        .timeout(Duration::from_secs(60))
        .current_dir(root.path())
        .args([
            "--config",
            text(&root.path().join("absent-config.json")),
            "render-profile",
            "export",
            "--dir",
            text(&destination),
        ])
        .env_remove("RKC_CONFIG")
        .env_remove("GITLAB_TOKEN")
        .env_remove("RKC_PROFILE_TEST_TOKEN")
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let manifest = read_json(&destination.join("profile.json"));
    assert_eq!(manifest["version"], "review-kit-render-profile/v1");
    assert_eq!(manifest["input_contract"], "review-kit-render-input/v2");
    for role in ["finding", "summary", "transition", "answer", "suggestion"] {
        for asset in ["schema", "template"] {
            assert!(
                destination
                    .join(manifest["artifacts"][role][asset].as_str().unwrap())
                    .is_file()
            );
        }
    }
}

#[test]
fn configuration_cli_and_asset_paths_resolve_from_their_declared_directories() {
    let fixture = Fixture::new();
    let configured = fixture.export("config/configured-profile");
    configured.prefix("summary", "CONFIGURED PRESENTATION");
    let explicit = fixture.export("invocation/explicit-profile");
    explicit.prefix("summary", "EXPLICIT PRESENTATION");
    let mut config = read_json(&fixture.config);
    config["render_profile"] = json!("configured-profile/profile.json");
    write_json(&fixture.config, &config);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    let invocation = fixture.root.path().join("invocation");
    let (output, configured_result) = fixture.command(&fixture.config, &invocation, &["check"]);
    assert!(output.status.success(), "{configured_result}");
    assert!(
        configured_result["check"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("CONFIGURED PRESENTATION\n")
    );
    let (output, override_result) = fixture.command(
        &fixture.config,
        &invocation,
        &["--render-profile", "explicit-profile/profile.json", "check"],
    );
    assert!(output.status.success(), "{override_result}");
    assert!(
        override_result["check"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("EXPLICIT PRESENTATION\n")
    );
    assert_ne!(
        configured_result["render_profile"]["fingerprint"],
        override_result["render_profile"]["fingerprint"]
    );
    let (output, invalid) = fixture.command(
        &fixture.config,
        &invocation,
        &[
            "--render-profile",
            "missing/profile.json",
            "publish",
            "--effects",
            "review",
        ],
    );
    assert_no_writes(&fixture, 0, &output, &invalid);
    let asset = explicit.asset("summary", "template");
    let moved = asset.with_file_name("team-summary.j2");
    fs::rename(&asset, &moved).unwrap();
    let mut manifest = read_json(&explicit.manifest);
    manifest["artifacts"]["summary"]["template"] = json!("team-summary.j2");
    write_json(&explicit.manifest, &manifest);
    let (output, relocated) = fixture.command(
        &fixture.config,
        &invocation,
        &["--render-profile", "explicit-profile/profile.json", "check"],
    );
    assert!(output.status.success(), "{relocated}");
    assert!(
        relocated["check"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("EXPLICIT PRESENTATION\n")
    );
}
