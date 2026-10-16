//! Provider provisioning steps shared by fixture owners: users, tokens,
//! diff collection and authenticated source checkouts.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::Path,
    time::{Duration, Instant},
};
use time::OffsetDateTime;

use super::{Api, private_file, process};

/// Creates a confirmed user with an unrecoverable random password.
pub fn create_user(admin: &Api, username: &str) -> Result<Value> {
    let mut random = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let password = format!("Aa!9{:x}", Sha256::digest(random));
    admin.post(
        "users",
        &json!({"username": username, "name": username, "email": format!("{username}@example.test"), "password": password, "skip_confirmation": true}),
    )
}

/// Issues a seven-day personal access token with one scope.
pub fn issue_token(admin: &Api, user: &Value, scope: &str) -> Result<String> {
    let date = (OffsetDateTime::now_utc() + time::Duration::days(7))
        .date()
        .to_string();
    let token = admin.post(
        &format!("users/{user}/personal_access_tokens"),
        &json!({"name": "cli-acceptance", "scopes": [scope], "expires_at": date}),
    )?;
    Ok(token["token"]
        .as_str()
        .context("issued fixture token")?
        .to_owned())
}

/// Waits until the merge request reports `head` and GitLab has collected a
/// diff version for it. With `start`, the version must also compare against
/// that target commit, which distinguishes a retargeted comparison from an
/// earlier version with the same head.
pub fn wait_diff(
    api: &Api,
    project: &Value,
    iid: &Value,
    head: &str,
    start: Option<&str>,
) -> Result<()> {
    let prefix = format!("projects/{project}/merge_requests/{iid}");
    wait_for(
        || {
            if api.get(&prefix)?["diff_refs"]["head_sha"] != head {
                return Ok(false);
            }
            for version in api
                .get(&format!("{prefix}/versions"))?
                .as_array()
                .context("MR versions")?
            {
                if version["head_commit_sha"] == head
                    && start.is_none_or(|start| version["start_commit_sha"] == start)
                {
                    return Ok(
                        api.get(&format!("{prefix}/versions/{}", version["id"]))?["state"]
                            == "collected",
                    );
                }
            }
            Ok(false)
        },
        "expected MR diff",
    )
}

/// Clones one branch over HTTP with credentials supplied by an askpass helper,
/// so tokens never appear in URLs, arguments or Git configuration.
pub struct Checkout<'a> {
    pub directory: &'a Path,
    pub tokens: &'a [String],
    pub server: &'a str,
    pub project_path: &'a str,
    pub branch: &'a str,
    pub checkout: &'a Path,
    pub token_file: &'a Path,
}

impl Checkout<'_> {
    pub fn run(&self) -> Result<()> {
        let askpass = self.directory.join("git-askpass.sh");
        private_file(
            &askpass,
            "#!/bin/sh\ncase \"$1\" in *sername*) printf '%s\\n' oauth2 ;; *) cat \"$RKC_FIXTURE_TOKEN_FILE\" ;; esac\n",
        )?;
        fs::set_permissions(&askpass, fs::Permissions::from_mode(0o700))?;
        let url = format!(
            "{}/{}.git",
            self.server.trim_end_matches('/'),
            self.project_path
        );
        let mut command = std::process::Command::new("git");
        command
            .args([
                "-c",
                "http.proxy=",
                "clone",
                "--quiet",
                "--single-branch",
                "--no-tags",
                "--branch",
                self.branch,
                &url,
            ])
            .arg(self.checkout)
            .env("GIT_ASKPASS", askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("RKC_FIXTURE_TOKEN_FILE", self.token_file);
        process::execute_git(
            self.directory,
            self.tokens,
            &format!("clone-{}", self.branch),
            command,
            Duration::from_secs(180),
        )?;
        Ok(())
    }
}

pub fn wait_for(mut probe: impl FnMut() -> Result<bool>, description: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if probe()? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("timed out waiting for {description}")
}
