use anyhow::{Context, Result, ensure};
use assert_cmd::Command;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;

use super::{assessments::PRIVATE_CONTEXT, source};

#[path = "fixture/api.rs"]
mod api;
#[path = "fixture/observation.rs"]
mod observation;
#[path = "fixture/process.rs"]
pub mod process;
#[path = "fixture/provision.rs"]
pub mod provision;
pub use api::Api;
pub use observation::{authored_discussions, authored_notes};
use provision::wait_for;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root")
}
pub fn read_json(path: impl AsRef<Path>) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path.as_ref())?)?)
}
pub fn private_file(path: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Result<()> {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().context("private file parent")?)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    std::io::Write::write_all(&mut file, content.as_ref())?;
    Ok(())
}
pub fn write_json(path: impl AsRef<Path>, value: &Value) -> Result<()> {
    private_file(path, serde_json::to_vec_pretty(value)?)
}
pub fn marker(body: &str) -> Option<Value> {
    let text = body
        .strip_suffix(" -->")?
        .rsplit_once("\n\n<!-- review-kit:v1 ")?
        .1;
    if text.contains('\n') {
        return None;
    }
    serde_json::from_str(text).ok()
}
pub fn summary_state(note: &Value) -> Value {
    let body = note["body"].as_str().expect("summary body");
    let body = body
        .split_once("\n\n<!-- review-kit:state:v3 ")
        .expect("summary data")
        .1;
    let text = body
        .split_once(" -->\n\n<!-- review-kit:v1 ")
        .expect("summary JSON end")
        .0;
    serde_json::from_str(text).expect("summary state")
}

#[derive(Default, Clone, Copy)]
pub struct CliOptions<'a> {
    pub config: Option<&'a Path>,
    pub token: Option<&'a str>,
    pub offline: bool,
    pub expected: i32,
}

pub struct Fixture {
    directory_owner: Option<TempDir>,
    pub directory: PathBuf,
    pub manifest: Value,
    pub binary: PathBuf,
    pub tokens: Vec<String>,
    pub author: Api,
    admin: Api,
}

impl Fixture {
    pub fn setup() -> Result<Self> {
        let server = std::env::var("REVIEW_KIT_GITLAB_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8929".to_owned());
        let credential = std::env::var_os("REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| root().join("tests/gitlab/.local/admin-token"));
        Self::setup_with(
            &server,
            &fs::read_to_string(credential).context("read GitLab administrator credential")?,
        )
    }
    pub fn setup_with(server: &str, admin_token: &str) -> Result<Self> {
        let binary = std::env::var_os("RKC_E2E_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                option_env!("CARGO_BIN_EXE_rkc")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root().join("target/debug/rkc"))
            });
        let binary = binary.canonicalize().context("resolve CLI executable")?;
        ensure!(
            binary.is_file() && fs::metadata(&binary)?.permissions().mode() & 0o111 != 0,
            "RKC_E2E_BINARY must identify an executable file"
        );
        let local = root().join("tests/cli/.local");
        fs::create_dir_all(&local)?;
        let owner = tempfile::Builder::new()
            .prefix("rk-cli-")
            .tempdir_in(local)?;
        let directory = owner.path().to_owned();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let namespace = directory
            .file_name()
            .context("fixture namespace")?
            .to_string_lossy()
            .into_owned();
        let admin = Api::new(server, admin_token)?;
        let mut fixture = Self {
            directory_owner: Some(owner),
            directory: directory.clone(),
            binary,
            manifest: json!({"schema_version": "cli-acceptance/v1", "server": server, "namespace": namespace, "directory": directory, "phase": "setup"}),
            tokens: vec![admin_token.trim().to_owned()],
            author: Api::new(server, admin_token)?,
            admin,
        };
        fixture.save()?;
        fixture.initialize()?;
        fixture.record_run()?;
        Ok(fixture)
    }
    fn save(&self) -> Result<()> {
        write_json(self.directory.join("manifest.json"), &self.manifest)
    }
    #[allow(dead_code)]
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        ensure!(
            path.starts_with(root().join("tests/cli/.local").canonicalize()?),
            "fixture manifest must be under tests/cli/.local"
        );
        let manifest = read_json(&path)?;
        let directory = PathBuf::from(
            manifest["directory"]
                .as_str()
                .context("fixture directory")?,
        )
        .canonicalize()?;
        ensure!(
            path == directory.join("manifest.json")
                && manifest["schema_version"] == "cli-acceptance/v1",
            "unsupported fixture manifest"
        );
        let server = manifest["server"].as_str().context("fixture server")?;
        let credential = std::env::var_os("REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| root().join("tests/gitlab/.local/admin-token"));
        let admin_token = fs::read_to_string(credential)?;
        let author_token = fs::read_to_string(
            manifest["author"]["token_file"]
                .as_str()
                .context("author credential file")?,
        )?;
        let reviewer_token = fs::read_to_string(
            manifest["reviewer"]["token_file"]
                .as_str()
                .context("reviewer credential file")?,
        )?;
        let author = Api::new(server, &author_token)?;
        let admin = Api::new(server, &admin_token)?;
        let binary = std::env::var_os("RKC_E2E_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| root().join("target/debug/rkc"));
        let fixture = Self {
            directory_owner: None,
            directory,
            manifest,
            binary,
            tokens: vec![
                admin_token.trim().to_owned(),
                author_token.trim().to_owned(),
                reviewer_token.trim().to_owned(),
            ],
            author,
            admin,
        };
        fixture.verify_project()?;
        Ok(fixture)
    }
    fn verify_project(&self) -> Result<()> {
        let project = self
            .author
            .get(&format!("projects/{}", self.manifest["project_id"]))?;
        ensure!(
            project["path_with_namespace"] == self.manifest["project_path"]
                && project["namespace"]["full_path"] == self.manifest["namespace"],
            "fixture project does not match retained namespace"
        );
        Ok(())
    }
    fn token(&self, role: &str) -> Result<String> {
        Ok(fs::read_to_string(
            self.manifest[role]["token_file"]
                .as_str()
                .context("fixture token file")?,
        )?
        .trim()
        .to_owned())
    }
    fn initialize(&mut self) -> Result<()> {
        let namespace = self.manifest["namespace"]
            .as_str()
            .context("namespace")?
            .to_owned();
        let group = self.admin.post(
            "groups",
            &json!({"name": namespace, "path": namespace, "visibility": "private"}),
        )?;
        self.manifest["group_id"] = group["id"].clone();
        self.save()?;
        for (role, access) in [("author", 40), ("reviewer", 30)] {
            let name = format!("{namespace}-{role}");
            let user = provision::create_user(&self.admin, &name)?;
            let token_file = self.directory.join(format!("{role}-token"));
            self.manifest[role] =
                json!({"id": user["id"], "username": name, "token_file": token_file});
            self.save()?;
            let token = self.issue_token(&user["id"], "api")?;
            private_file(&token_file, &token)?;
            self.tokens.push(token);
            self.admin.post(
                &format!("groups/{}/members", group["id"]),
                &json!({"user_id": user["id"], "access_level": access}),
            )?;
        }
        self.author = Api::new(
            self.manifest["server"].as_str().unwrap(),
            &self.token("author")?,
        )?;
        let project = self.admin.post("projects", &json!({"name": "quantity-store", "path": "quantity-store", "namespace_id": group["id"], "visibility": "private", "initialize_with_readme": true, "default_branch": "main"}))?;
        self.manifest["project_id"] = project["id"].clone();
        self.manifest["project_path"] = project["path_with_namespace"].clone();
        self.save()?;
        wait_for(
            || {
                // Project creation can precede inherited member visibility.
                Ok(self
                    .author
                    .get_if_exists(&format!("projects/{}", project["id"]))?
                    .is_some_and(|project| project["empty_repo"] == false))
            },
            "initialized repository visible to its author",
        )?;
        self.manifest["base_head"] = json!(self.commit(
            "main",
            source::base_files(),
            "Initialize quantity storage library",
            None,
            true
        )?);
        self.save()?;
        for (label, files, title) in [
            (
                "review",
                BTreeMap::from([
                    ("src/quantity.rs", source::quantity_input()),
                    ("src/storage.rs", source::storage_audit()),
                ]),
                "Add JSON quantity input and optional audit output",
            ),
            (
                "clean",
                BTreeMap::from([("src/quantity.rs", source::quantity_clean())]),
                "Add checked quantity doubling",
            ),
        ] {
            let head = self.commit(label, files, title, Some("main"), false)?;
            let mr = self.author.post(&format!("projects/{}/merge_requests", project["id"]), &json!({"source_branch": label, "target_branch": "main", "title": title, "description": "Review the changed behavior against the library README and run focused offline checks."}))?;
            self.wait_diff(&mr["iid"], &head)?;
            let checkout = self.directory.join(if label == "review" {
                "source"
            } else {
                "source-clean"
            });
            self.manifest["mrs"][label] = json!({"iid": mr["iid"], "url": mr["web_url"], "branch": label, "head": head, "source": checkout});
            self.save()?;
            self.clone_source(label, &checkout)?;
        }
        let config = self.directory.join("config.json");
        write_json(
            &config,
            &json!({"version": 1, "gitlab": {"server": self.manifest["server"], "token_env": "REVIEW_KIT_REVIEWER_TOKEN", "authority": ["create_summary", "update_summary", "finding", "reply", "resolve", "reopen"]}}),
        )?;
        fs::create_dir(self.directory.join("tmp"))?;
        self.manifest["config"] = json!(config);
        self.manifest["phase"] = json!("review");
        self.save()
    }
    fn issue_token(&self, user: &Value, scope: &str) -> Result<String> {
        provision::issue_token(&self.admin, user, scope)
    }
    #[allow(dead_code, reason = "used by live permission scenarios")]
    pub fn read_only_token(&mut self) -> Result<String> {
        let token = self.issue_token(&self.manifest["reviewer"]["id"], "read_api")?;
        private_file(self.directory.join("read-only-token"), &token)?;
        self.tokens.push(token.clone());
        Ok(token)
    }
    fn commit(
        &self,
        branch: &str,
        files: BTreeMap<&str, String>,
        message: &str,
        start: Option<&str>,
        initialize: bool,
    ) -> Result<String> {
        let actions: Vec<_> = files.into_iter().map(|(path, content)| json!({"action": if path == "README.md" || !initialize { "update" } else { "create" }, "file_path": path, "content": content})).collect();
        let mut body = json!({"branch": branch, "commit_message": message, "actions": actions});
        if let Some(start) = start {
            body["start_branch"] = json!(start);
        }
        Ok(self.author.post(
            &format!(
                "projects/{}/repository/commits",
                self.manifest["project_id"]
            ),
            &body,
        )?["id"]
            .as_str()
            .context("commit id")?
            .to_owned())
    }
    fn clone_source(&mut self, branch: &str, checkout: &Path) -> Result<()> {
        provision::Checkout {
            directory: &self.directory,
            tokens: &self.tokens,
            server: self.manifest["server"].as_str().context("fixture server")?,
            project_path: self.manifest["project_path"]
                .as_str()
                .context("project path")?,
            branch,
            checkout,
            token_file: Path::new(
                self.manifest["reviewer"]["token_file"]
                    .as_str()
                    .context("reviewer credential file")?,
            ),
        }
        .run()
    }
    fn wait_diff(&self, iid: &Value, head: &str) -> Result<()> {
        provision::wait_diff(&self.author, &self.manifest["project_id"], iid, head, None)
    }
    pub fn evolve(&mut self, recurrence: bool) -> Result<()> {
        self.verify_project()?;
        ensure!(
            if recurrence {
                self.manifest["phase"] == "fixed"
            } else {
                matches!(
                    self.manifest["phase"].as_str(),
                    Some("review" | "fixed" | "recurrence")
                )
            },
            "fixture is in the wrong phase"
        );
        let mr = self.manifest["mrs"]["review"].clone();
        let branch = mr["branch"].as_str().context("review branch")?;
        let files = BTreeMap::from([
            (
                "src/quantity.rs",
                if recurrence {
                    source::quantity_input()
                } else {
                    source::quantity_fixed()
                },
            ),
            ("src/storage.rs", source::storage_fixed()),
        ]);
        let head = self.commit(
            branch,
            files,
            if recurrence {
                "Simplify JSON quantity decoding"
            } else {
                "Preserve quantity and save contracts"
            },
            None,
            false,
        )?;
        self.wait_diff(&mr["iid"], &head)?;
        if !recurrence {
            let prefix = format!(
                "projects/{}/merge_requests/{}",
                self.manifest["project_id"], mr["iid"]
            );
            for discussion in self
                .author
                .pages(&format!("{prefix}/discussions"))?
                .as_array()
                .context("discussions")?
            {
                let note = &discussion["notes"][0];
                if note["author"]["id"] == self.manifest["reviewer"]["id"]
                    && note["type"] == "DiffNote"
                {
                    self.author.post(&format!("{prefix}/discussions/{}/notes", discussion["id"].as_str().context("discussion id")?), &json!({"body": "The updated commit includes a regression for the reported case. Please check the revised behavior."}))?;
                }
            }
        }
        let phase = if recurrence { "recurrence" } else { "fixed" };
        let checkout = self.directory.join(format!("source-{phase}-{head}"));
        self.clone_source(branch, &checkout)?;
        self.manifest["mrs"]["review"]["source"] = json!(checkout);
        self.manifest["mrs"]["review"]["head"] = json!(head);
        self.manifest["phase"] = json!(phase);
        self.save()
    }
    pub fn extra_mr(&self, label: &str) -> Result<Value> {
        let branch = format!(
            "{label}-{}",
            self.directory.file_name().unwrap().to_string_lossy()
        );
        let head = &self.manifest["mrs"]["clean"]["head"];
        let project = &self.manifest["project_id"];
        self.author.post(
            &format!("projects/{project}/repository/branches"),
            &json!({"branch": branch, "ref": head}),
        )?;
        let mr = self.author.post(&format!("projects/{project}/merge_requests"), &json!({"source_branch": branch, "target_branch": "main", "title": format!("CLI {label} control")}))?;
        self.wait_diff(&mr["iid"], head.as_str().context("clean head")?)?;
        Ok(json!({"iid": mr["iid"], "url": mr["web_url"], "head": head}))
    }
    fn process(
        &mut self,
        label: &str,
        command: &mut Command,
        expected: Option<i32>,
    ) -> Result<std::process::Output> {
        process::execute(&self.directory, &self.tokens, label, command, expected)
    }
    pub fn cli_command(&self, arguments: &[&str], options: &CliOptions<'_>) -> Result<Command> {
        let config = options
            .config
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(self.manifest["config"].as_str().unwrap()));
        let token_env = read_json(&config)?["gitlab"]["token_env"]
            .as_str()
            .unwrap_or("REVIEW_KIT_REVIEWER_TOKEN")
            .to_owned();
        let mut command = Command::new(&self.binary);
        command
            .arg("--config")
            .arg(config)
            .args(arguments)
            .env_remove("GITLAB_TOKEN")
            .env_remove("REVIEW_KIT_REVIEWER_TOKEN")
            .env_remove(&token_env)
            .env("TMPDIR", self.directory.join("tmp"));
        if !options.offline {
            command.env(
                &token_env,
                options
                    .token
                    .map(str::to_owned)
                    .unwrap_or(self.token("reviewer")?),
            );
        }
        Ok(command)
    }
    /// Runs `rkc` from the repository root, for commands that need no review worktree.
    #[allow(dead_code, reason = "used by live profile scenarios")]
    pub fn cli(
        &mut self,
        label: &str,
        arguments: &[&str],
        options: CliOptions<'_>,
    ) -> Result<Value> {
        let mut command = self.cli_command(arguments, &options)?;
        let output = self.process(label, &mut command, Some(options.expected))?;
        let value = serde_json::from_slice(&output.stdout).context("CLI JSON output")?;
        Ok(value)
    }
    /// Runs agent-supplied `rkc` arguments in `worktree` and preserves their exit status.
    #[allow(dead_code)]
    pub fn agent_cli(
        &mut self,
        worktree: &Path,
        arguments: &[&str],
    ) -> Result<std::process::Output> {
        let mut command = self.cli_command(arguments, &CliOptions::default())?;
        process::execute_in(
            &self.directory,
            &self.tokens,
            "agent-command",
            &mut command,
            None,
            worktree,
        )
    }
    pub fn observe(&self, label: &str, mr: &Value) -> Result<Value> {
        let prefix = format!(
            "projects/{}/merge_requests/{}",
            self.manifest["project_id"], mr["iid"]
        );
        let observed = json!({"mr": self.author.get(&prefix)?, "notes": self.author.pages(&format!("{prefix}/notes"))?, "discussions": self.author.pages(&format!("{prefix}/discussions"))?});
        write_json(
            self.directory
                .join("evidence/e2e")
                .join(format!("{label}-provider.json")),
            &observed,
        )?;
        for note in observed["notes"].as_array().context("observed notes")? {
            let body = note["body"].as_str().context("note body")?;
            ensure!(
                !body.contains(PRIVATE_CONTEXT)
                    && !body.contains(self.directory.to_str().unwrap())
                    && !self.tokens.iter().any(|token| body.contains(token)),
                "private data appears in provider body"
            );
        }
        Ok(observed)
    }
    #[allow(dead_code, reason = "used by live provider assertions")]
    pub fn owned(&self, observed: &Value) -> (Vec<Value>, BTreeMap<u64, Value>) {
        let reviewer = &self.manifest["reviewer"]["id"];
        let summaries = observed["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|note| {
                note["author"]["id"] == *reviewer
                    && marker(note["body"].as_str().unwrap()) == Some(json!({"kind": "summary"}))
            })
            .cloned()
            .collect();
        let mut roots = BTreeMap::new();
        for discussion in observed["discussions"].as_array().unwrap() {
            let note = &discussion["notes"][0];
            if note["author"]["id"] != *reviewer {
                continue;
            }
            if let Some(record) = marker(note["body"].as_str().unwrap())
                && record["kind"] == "finding"
            {
                assert!(
                    roots
                        .insert(record["finding"].as_u64().unwrap(), discussion.clone())
                        .is_none(),
                    "duplicate owned root"
                );
            }
        }
        (summaries, roots)
    }
    #[allow(dead_code, reason = "used by live source assertions")]
    pub fn committed_file(&self, path: &str, head: &str) -> Result<Value> {
        let path: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
        self.author.get(&format!(
            "projects/{}/repository/files/{path}?ref={head}",
            self.manifest["project_id"]
        ))
    }
    fn record_run(&mut self) -> Result<()> {
        let mut command = Command::new(&self.binary);
        command.arg("--version");
        let version = self.process("version", &mut command, Some(0))?;
        let mut command = std::process::Command::new("git");
        command.args(["rev-parse", "HEAD"]);
        let revision = process::execute_git(
            &self.directory,
            &self.tokens,
            "source-revision",
            command,
            Duration::from_secs(180),
        )?;
        let mut command = std::process::Command::new("git");
        command.args(["status", "--porcelain"]);
        let status = process::execute_git(
            &self.directory,
            &self.tokens,
            "source-status",
            command,
            Duration::from_secs(180),
        )?;
        let digest = Sha256::digest(fs::read(&self.binary)?);
        write_json(
            self.directory.join("evidence/e2e/run.json"),
            &json!({"execution_mode": "sequential", "binary": self.binary, "version": String::from_utf8_lossy(&version.stdout).trim(), "binary_sha256": format!("{digest:x}"), "source_revision": String::from_utf8_lossy(&revision.stdout).trim(), "source_dirty": !status.stdout.is_empty(), "server": self.manifest["server"], "namespace": self.manifest["namespace"]}),
        )
    }
    #[allow(
        dead_code,
        reason = "live tests clean up only after all assertions pass"
    )]
    pub fn finish(self) -> Result<()> {
        self.cleanup()
    }
    pub fn cleanup(&self) -> Result<()> {
        self.verify_project()?;
        let namespace = self.manifest["namespace"].as_str().context("namespace")?;
        ensure!(
            namespace.starts_with("rk-cli-")
                && namespace
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
                && self.directory.canonicalize()?
                    == root()
                        .join("tests/cli/.local")
                        .join(namespace)
                        .canonicalize()?,
            "cleanup requires the exact owned fixture"
        );
        let group = self
            .admin
            .get(&format!("groups/{}", self.manifest["group_id"]))?;
        let project = self
            .admin
            .get(&format!("projects/{}", self.manifest["project_id"]))?;
        ensure!(
            group["full_path"] == namespace
                && project["path_with_namespace"] == self.manifest["project_path"]
                && project["namespace"]["id"] == self.manifest["group_id"],
            "cleanup resource identities differ"
        );
        let mut resources = vec![
            format!("projects/{}", self.manifest["project_id"]),
            format!("groups/{}", self.manifest["group_id"]),
        ];
        for role in ["author", "reviewer"] {
            let retained = &self.manifest[role];
            let user = self.admin.get(&format!("users/{}", retained["id"]))?;
            ensure!(
                user["username"] == retained["username"]
                    && user["username"] == format!("{namespace}-{role}"),
                "cleanup user identity differs"
            );
            resources.push(format!("users/{}?hard_delete=true", retained["id"]));
        }
        let path = self.directory.join("evidence/cleanup.json");
        let mut ledger = json!({"namespace": namespace, "resources": resources, "requested": [], "status": "started"});
        write_json(&path, &ledger)?;
        for resource in resources {
            if let Err(error) = self.admin.delete(&resource) {
                ledger["status"] = json!("failed");
                ledger["error"] = json!(error.to_string());
                write_json(&path, &ledger)?;
                return Err(error);
            }
            ledger["requested"]
                .as_array_mut()
                .unwrap()
                .push(json!(resource));
            write_json(&path, &ledger)?;
        }
        ledger["status"] = json!("deletion_requested");
        write_json(path, &ledger)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(owner) = self.directory_owner.take() {
            let _ = owner.keep();
        }
        eprintln!(
            "CLI fixture evidence: {}",
            self.directory.join("manifest.json").display()
        );
    }
}
