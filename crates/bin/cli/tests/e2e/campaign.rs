//! Live campaign fixture: one private GitLab namespace that owns separate
//! author, reviewer and commenter users and one project per fixture family.
//!
//! The manifest records each identity as soon as GitLab creates it. Every
//! later operation verifies that the project and merge request it touches
//! belong to this namespace and were opened by its author, and cleanup
//! deletes only recorded resources.

#[path = "campaign/container.rs"]
pub mod container;
#[path = "campaign/digest.rs"]
pub mod digest;
#[path = "campaign/family.rs"]
pub mod family;
#[path = "campaign/mutation.rs"]
pub mod mutation;
#[path = "campaign/oracle.rs"]
pub mod oracle;

use anyhow::{Context, Result, bail, ensure};
use assert_cmd::Command;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use super::fixture::{Api, private_file, process, provision, read_json, root, write_json};
use family::Family;

pub const SCHEMA: &str = "cli-campaign/v1";
pub const PREFIX: &str = "rk-camp-";
pub const AUTHORITY: [&str; 6] = [
    "create_summary",
    "update_summary",
    "finding",
    "reply",
    "resolve",
    "reopen",
];
pub const TOKEN_ENV: &str = "REVIEW_KIT_REVIEWER_TOKEN";

/// Fixture identities. `ReadOnly` is the reviewer user with a `read_api` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Author,
    Reviewer,
    ReadOnly,
    Commenter,
}

impl Role {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "author" => Self::Author,
            "reviewer" => Self::Reviewer,
            "read-only" => Self::ReadOnly,
            "commenter" => Self::Commenter,
            _ => bail!("role must be author, reviewer, read-only or commenter"),
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Reviewer => "reviewer",
            Self::ReadOnly => "read-only",
            Self::Commenter => "commenter",
        }
    }

    fn user(self) -> &'static str {
        match self {
            Self::ReadOnly => "reviewer",
            other => other.key(),
        }
    }
}

/// One merge request of one family in the manifest.
#[derive(Debug, Clone)]
pub struct Target {
    pub family: String,
    pub label: String,
}

pub struct Campaign {
    pub directory: PathBuf,
    pub manifest: Value,
    pub tokens: Vec<String>,
    admin: Api,
}

pub fn server() -> String {
    std::env::var("REVIEW_KIT_GITLAB_URL").unwrap_or_else(|_| "http://127.0.0.1:8929".to_owned())
}

pub fn admin_token() -> Result<String> {
    let credential = std::env::var_os("REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("tests/gitlab/.local/admin-token"));
    Ok(fs::read_to_string(credential)
        .context("read GitLab administrator credential")?
        .trim()
        .to_owned())
}

/// Directory that holds campaign directories: `RKC_FIXTURE_HOME` or the
/// ignored `tests/cli/.local/`.
pub fn home() -> PathBuf {
    std::env::var_os("RKC_FIXTURE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("tests/cli/.local"))
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

impl Campaign {
    /// Creates the namespace, users, tokens and one project per family, then
    /// opens each family's merge requests and clones their sources.
    pub fn setup(names: &[String]) -> Result<Self> {
        let available = family::names(&family::root())?;
        let names = if names.is_empty() {
            available.clone()
        } else {
            names.to_vec()
        };
        let families = names
            .iter()
            .map(|name| family::load(&family::root(), name))
            .collect::<Result<Vec<_>>>()?;
        let server = server();
        let admin_token = admin_token()?;
        let home = home();
        fs::create_dir_all(&home)?;
        let directory = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir_in(&home)?
            .keep();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let directory = directory.canonicalize()?;
        let namespace = directory
            .file_name()
            .context("campaign namespace")?
            .to_string_lossy()
            .into_owned();
        let mut campaign = Self {
            directory: directory.clone(),
            manifest: json!({"schema_version": SCHEMA, "server": server, "namespace": namespace, "directory": directory, "status": "setup", "users": {}, "families": {}}),
            tokens: vec![admin_token.clone()],
            admin: Api::new(&server, &admin_token)?,
        };
        campaign.save()?;
        campaign.provision_users()?;
        for family in &families {
            campaign.provision_family(family)?;
        }
        let config = directory.join("config.json");
        write_json(
            &config,
            &json!({"version": 1, "publication_state": "publication-state", "gitlab": {"server": server, "token_env": TOKEN_ENV, "authority": AUTHORITY}}),
        )?;
        fs::create_dir_all(directory.join("tmp"))?;
        campaign.manifest["config"] = json!(config);
        campaign.manifest["publication_state"] = json!(directory.join("publication-state"));
        campaign.manifest["status"] = json!("ready");
        campaign.save()?;
        Ok(campaign)
    }

    fn provision_users(&mut self) -> Result<()> {
        let namespace = self.namespace()?.to_owned();
        let group = self.admin.post(
            "groups",
            &json!({"name": namespace, "path": namespace, "visibility": "private"}),
        )?;
        self.manifest["group_id"] = group["id"].clone();
        self.save()?;
        let credentials = self.directory.join("credentials");
        fs::create_dir_all(&credentials)?;
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700))?;
        for (role, access) in [("author", 40), ("reviewer", 30), ("commenter", 30)] {
            let username = format!("{namespace}-{role}");
            let user = provision::create_user(&self.admin, &username)?;
            self.manifest["users"][role] = json!({"id": user["id"], "username": username});
            self.save()?;
            let token_file = credentials.join(format!("{role}-token"));
            let token = provision::issue_token(&self.admin, &user["id"], "api")?;
            private_file(&token_file, &token)?;
            self.tokens.push(token);
            self.manifest["users"][role]["token_file"] = json!(token_file);
            if role == "reviewer" {
                let read_only = credentials.join("read-only-token");
                let token = provision::issue_token(&self.admin, &user["id"], "read_api")?;
                private_file(&read_only, &token)?;
                self.tokens.push(token);
                self.manifest["users"][role]["read_only_token_file"] = json!(read_only);
            }
            self.save()?;
            self.admin.post(
                &format!("groups/{}/members", group["id"]),
                &json!({"user_id": user["id"], "access_level": access}),
            )?;
        }
        Ok(())
    }

    fn provision_family(&mut self, family: &Family) -> Result<()> {
        for (origin, text) in family.published_texts() {
            let hints = family::review_hints(text);
            ensure!(
                hints.is_empty(),
                "family {} {origin} would disclose {hints:?}",
                family.name
            );
        }
        let project = self.admin.post(
            "projects",
            &json!({"name": family.descriptor.project, "path": family.descriptor.project, "namespace_id": self.manifest["group_id"], "visibility": "private", "initialize_with_readme": true, "default_branch": family::DEFAULT_BRANCH}),
        )?;
        let key = &family.name;
        self.manifest["families"][key] = json!({"project_id": project["id"], "project_path": project["path_with_namespace"], "merge_requests": {}});
        self.save()?;
        let author = self.api(Role::Author)?;
        provision::wait_for(
            || {
                // Project creation can precede inherited member visibility.
                Ok(author
                    .get_if_exists(&format!("projects/{}", project["id"]))?
                    .is_some_and(|project| project["empty_repo"] == false))
            },
            "initialized repository visible to its author",
        )?;
        let initial = self.branch_head(&author, &project["id"], family::DEFAULT_BRANCH)?;
        let base = family::Change {
            write: family.base.clone(),
            delete: Vec::new(),
        };
        let base_head = commit_change(
            &author,
            &project["id"],
            family::DEFAULT_BRANCH,
            None,
            &base,
            &family.descriptor.base_message,
            &family.descriptor.executable,
        )?;
        self.manifest["families"][key]["initial_head"] = json!(initial);
        self.manifest["families"][key]["base_head"] = json!(base_head);
        self.save()?;
        for request in &family.descriptor.merge_requests {
            let (change, message) = family.change(&request.overlay)?;
            let head = commit_change(
                &author,
                &project["id"],
                &request.branch,
                Some(family::DEFAULT_BRANCH),
                &change,
                message,
                &family.descriptor.executable,
            )?;
            let mr = author.post(
                &format!("projects/{}/merge_requests", project["id"]),
                &json!({"source_branch": request.branch, "target_branch": family::DEFAULT_BRANCH, "title": request.title, "description": request.description}),
            )?;
            self.manifest["families"][key]["merge_requests"][&request.label] = json!({"iid": mr["iid"], "url": mr["web_url"], "branch": request.branch, "target": family::DEFAULT_BRANCH, "head": head, "applied": [request.overlay], "checkouts": []});
            self.save()?;
            provision::wait_diff(&author, &project["id"], &mr["iid"], &head, Some(&base_head))?;
            let target = Target {
                family: key.clone(),
                label: request.label.clone(),
            };
            self.settle(&target)?;
            self.refresh_record(&target)?;
            self.checkout(&target)?;
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let manifest = read_json(&path)?;
        ensure!(
            manifest["schema_version"] == SCHEMA,
            "unsupported campaign manifest"
        );
        let directory = path.parent().context("campaign directory")?.to_owned();
        let namespace = manifest["namespace"]
            .as_str()
            .context("campaign namespace")?;
        ensure!(
            manifest["directory"] == json!(directory)
                && directory.file_name().is_some_and(|name| name == namespace)
                && namespace.starts_with(PREFIX)
                && namespace
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "campaign manifest does not describe its own directory"
        );
        let server = manifest["server"].as_str().context("campaign server")?;
        let admin_token = admin_token()?;
        let mut tokens = vec![admin_token.clone()];
        for user in manifest["users"]
            .as_object()
            .context("campaign users")?
            .values()
        {
            for field in ["token_file", "read_only_token_file"] {
                if let Some(file) = user[field].as_str() {
                    tokens.push(fs::read_to_string(file)?.trim().to_owned());
                }
            }
        }
        Ok(Self {
            admin: Api::new(server, &admin_token)?,
            directory,
            manifest,
            tokens,
        })
    }

    pub fn save(&self) -> Result<()> {
        write_json(self.directory.join("manifest.json"), &self.manifest)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.directory.join("manifest.json")
    }

    pub fn namespace(&self) -> Result<&str> {
        self.manifest["namespace"]
            .as_str()
            .context("campaign namespace")
    }

    pub fn server(&self) -> Result<&str> {
        self.manifest["server"].as_str().context("campaign server")
    }

    fn token_file(&self, role: Role) -> Result<PathBuf> {
        let field = if role == Role::ReadOnly {
            "read_only_token_file"
        } else {
            "token_file"
        };
        Ok(PathBuf::from(
            self.manifest["users"][role.user()][field]
                .as_str()
                .with_context(|| format!("{} credential file", role.key()))?,
        ))
    }

    pub fn token(&self, role: Role) -> Result<String> {
        Ok(fs::read_to_string(self.token_file(role)?)?
            .trim()
            .to_owned())
    }

    pub fn api(&self, role: Role) -> Result<Api> {
        Api::new(self.server()?, &self.token(role)?)
    }

    pub fn user_id(&self, role: Role) -> Result<&Value> {
        let id = &self.manifest["users"][role.user()]["id"];
        ensure!(!id.is_null(), "campaign has no {} user", role.key());
        Ok(id)
    }

    pub fn family_record(&self, family: &str) -> Result<&Value> {
        let record = &self.manifest["families"][family];
        ensure!(record.is_object(), "campaign has no family {family}");
        Ok(record)
    }

    /// Resolves a merge request label, defaulting to the family's first one.
    pub fn target(&self, family: &str, label: Option<&str>) -> Result<Target> {
        let requests = self.family_record(family)?["merge_requests"]
            .as_object()
            .context("family merge requests")?;
        let label = match label {
            Some(label) => label.to_owned(),
            None => family::load(&family::root(), family)?
                .descriptor
                .merge_requests
                .first()
                .context("family merge request")?
                .label
                .clone(),
        };
        ensure!(
            requests.contains_key(&label),
            "family {family} has no merge request {label}"
        );
        Ok(Target {
            family: family.to_owned(),
            label,
        })
    }

    pub fn record(&self, target: &Target) -> Result<&Value> {
        Ok(&self.family_record(&target.family)?["merge_requests"][&target.label])
    }

    pub fn project_id(&self, family: &str) -> Result<Value> {
        Ok(self.family_record(family)?["project_id"].clone())
    }

    /// Confirms that the recorded project still belongs to this namespace.
    pub fn verify_project(&self, family: &str) -> Result<Value> {
        let record = self.family_record(family)?;
        let project = self
            .admin
            .get(&format!("projects/{}", record["project_id"]))?;
        ensure!(
            project["path_with_namespace"] == record["project_path"]
                && project["namespace"]["id"] == self.manifest["group_id"]
                && project["namespace"]["full_path"] == self.manifest["namespace"],
            "project {} is not owned by this campaign",
            record["project_path"]
        );
        Ok(project)
    }

    /// Confirms project ownership and that the merge request is the one this
    /// campaign's author opened from the recorded branch. Returns its state.
    pub fn verify_merge_request(&self, target: &Target) -> Result<Value> {
        let project = self.verify_project(&target.family)?;
        let record = self.record(target)?;
        let mr = self.admin.get(&format!(
            "projects/{}/merge_requests/{}",
            project["id"], record["iid"]
        ))?;
        ensure!(
            mr["project_id"] == project["id"]
                && mr["source_branch"] == record["branch"]
                && mr["author"]["id"] == *self.user_id(Role::Author)?,
            "merge request {} is not owned by this campaign",
            record["url"]
        );
        Ok(mr)
    }

    pub fn merge_request_path(&self, target: &Target) -> Result<String> {
        Ok(format!(
            "projects/{}/merge_requests/{}",
            self.project_id(&target.family)?,
            self.record(target)?["iid"]
        ))
    }

    fn branch_head(&self, api: &Api, project: &Value, branch: &str) -> Result<String> {
        Ok(api.get(&format!(
            "projects/{project}/repository/branches/{}",
            encode(branch)
        ))?["commit"]["id"]
            .as_str()
            .context("branch head")?
            .to_owned())
    }

    /// Waits until the merge request's diff versions and notes stay unchanged
    /// for three seconds. GitLab processes pushes after the API call that
    /// caused them returns, and that processing can add a diff version and a
    /// system note to an existing merge request.
    pub fn settle(&self, target: &Target) -> Result<()> {
        let api = self.api(Role::Author)?;
        let prefix = self.merge_request_path(target)?;
        let quiet = std::time::Duration::from_secs(3);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut last = None;
        let mut since = std::time::Instant::now();
        loop {
            let state = (
                api.pages(&format!("{prefix}/versions"))?,
                api.pages(&format!("{prefix}/notes"))?
                    .as_array()
                    .context("notes")?
                    .iter()
                    .map(|note| (note["id"].clone(), note["updated_at"].clone()))
                    .collect::<Vec<_>>(),
            );
            let versions = state.0.as_array().context("versions")?;
            let collected = versions
                .iter()
                .all(|version| version["state"] == "collected");
            if last.as_ref() != Some(&state) || !collected {
                last = Some(state);
                since = std::time::Instant::now();
            } else if since.elapsed() >= quiet {
                return Ok(());
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "merge request {} did not settle",
                target.label
            );
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    /// Records the merge request's current target, head and comparison.
    pub fn refresh_record(&mut self, target: &Target) -> Result<Value> {
        let mr = self.verify_merge_request(target)?;
        let record =
            &mut self.manifest["families"][&target.family]["merge_requests"][&target.label];
        record["target"] = mr["target_branch"].clone();
        record["head"] = mr["diff_refs"]["head_sha"].clone();
        record["base"] = mr["diff_refs"]["base_sha"].clone();
        record["start"] = mr["diff_refs"]["start_sha"].clone();
        self.save()?;
        Ok(mr)
    }

    /// Clones the merge request's current head for review and records the
    /// checkout as its current source. Earlier checkouts remain on disk.
    pub fn checkout(&mut self, target: &Target) -> Result<PathBuf> {
        let record = self.record(target)?.clone();
        let head = record["head"].as_str().context("merge request head")?;
        let sequence = record["checkouts"].as_array().map_or(0, Vec::len) + 1;
        let checkout = self
            .directory
            .join("checkouts")
            .join(&target.family)
            .join(format!("{}-{sequence}-{}", target.label, &head[..12]));
        fs::create_dir_all(checkout.parent().context("checkout parent")?)?;
        provision::Checkout {
            directory: &self.directory,
            tokens: &self.tokens,
            server: self.server()?,
            project_path: self.family_record(&target.family)?["project_path"]
                .as_str()
                .context("project path")?,
            branch: record["branch"].as_str().context("merge request branch")?,
            checkout: &checkout,
            token_file: &self.token_file(Role::Reviewer)?,
        }
        .run()?;
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(&checkout).args(["rev-parse", "HEAD"]);
        let cloned = process::execute_git(
            &self.directory,
            &self.tokens,
            "checkout-revision",
            command,
            std::time::Duration::from_secs(60),
        )?;
        ensure!(
            String::from_utf8_lossy(&cloned.stdout).trim() == head,
            "checkout of {} does not contain the recorded head",
            target.label
        );
        let record =
            &mut self.manifest["families"][&target.family]["merge_requests"][&target.label];
        record["source"] = json!(checkout);
        record["checkouts"]
            .as_array_mut()
            .context("checkout history")?
            .push(json!({"head": head, "path": checkout}));
        self.save()?;
        Ok(checkout)
    }

    /// Runs `rkc` in the merge request's current checkout with the campaign
    /// configuration and the selected role's token in the configured variable.
    pub fn rkc(
        &mut self,
        target: &Target,
        binary: &Path,
        role: Role,
        config: Option<&Path>,
        arguments: &[String],
    ) -> Result<std::process::Output> {
        self.verify_merge_request(target)?;
        let config = match config {
            Some(config) => config.canonicalize()?,
            None => PathBuf::from(
                self.manifest["config"]
                    .as_str()
                    .context("campaign config")?,
            ),
        };
        let token_env = read_json(&config)?["gitlab"]["token_env"]
            .as_str()
            .unwrap_or("GITLAB_TOKEN")
            .to_owned();
        let checkout = PathBuf::from(
            self.record(target)?["source"]
                .as_str()
                .context("checkout")?,
        );
        let mut command = Command::new(binary.canonicalize().context("resolve rkc executable")?);
        command
            .arg("--config")
            .arg(&config)
            .args(arguments)
            .env_remove("GITLAB_TOKEN")
            .env_remove(TOKEN_ENV)
            .env(&token_env, self.token(role)?)
            .env("TMPDIR", self.directory.join("tmp"));
        process::execute_in(
            &self.directory,
            &self.tokens,
            &format!("rkc-{}-{}", target.family, target.label),
            &mut command,
            None,
            &checkout,
        )
    }

    /// Deletes every recorded project, the group and the users after checking
    /// that each identity still matches the manifest. GitLab first schedules
    /// project and group deletion and renames them; cleanup accepts exactly
    /// those renamed paths for the recorded IDs and then removes scheduled
    /// projects permanently. GitLab permits permanent removal only for
    /// subgroups, so the top-level campaign group stays scheduled for deletion
    /// until the instance's retention period ends. Running cleanup again
    /// resumes an interrupted run.
    pub fn cleanup(&self) -> Result<Value> {
        let namespace = self.namespace()?.to_owned();
        let group = self.manifest["group_id"].clone();
        let scheduled_group = format!("{namespace}-deletion_scheduled-{group}");
        let path = self.directory.join("evidence/cleanup.json");
        let mut ledger = json!({"namespace": namespace, "steps": [], "status": "started"});
        let step = |ledger: &mut Value, entry: Value| -> Result<()> {
            ledger["steps"]
                .as_array_mut()
                .context("cleanup steps")?
                .push(entry);
            write_json(&path, ledger)
        };
        for (name, record) in self.manifest["families"].as_object().context("families")? {
            let id = &record["project_id"];
            if id.is_null() {
                continue;
            }
            let recorded = record["project_path"].as_str().context("project path")?;
            let project_name = recorded.rsplit('/').next().context("project name")?;
            let accepted = [
                recorded.to_owned(),
                format!("{namespace}/{project_name}-deletion_scheduled-{id}"),
                format!("{scheduled_group}/{project_name}-deletion_scheduled-{id}"),
            ];
            let outcome = self.remove(
                &format!("projects/{id}"),
                "path_with_namespace",
                &accepted,
                |project| project["namespace"]["id"] == group,
            )?;
            step(
                &mut ledger,
                json!({"resource": format!("projects/{id}"), "family": name, "outcome": outcome}),
            )?;
        }
        if !group.is_null() {
            let accepted = [namespace.clone(), scheduled_group.clone()];
            let outcome =
                self.remove(&format!("groups/{group}"), "full_path", &accepted, |_| true)?;
            step(
                &mut ledger,
                json!({"resource": format!("groups/{group}"), "outcome": outcome}),
            )?;
        }
        for (role, retained) in self.manifest["users"].as_object().context("users")? {
            let resource = format!("users/{}", retained["id"]);
            let outcome = match self.admin.get_if_exists(&resource)? {
                None => "absent",
                Some(user) => {
                    ensure!(
                        user["username"] == retained["username"]
                            && user["username"] == format!("{namespace}-{role}"),
                        "cleanup user identity differs"
                    );
                    self.admin.delete(&format!("{resource}?hard_delete=true"))?;
                    "deletion_requested"
                }
            };
            step(
                &mut ledger,
                json!({"resource": resource, "role": role, "outcome": outcome}),
            )?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let resources: Vec<String> = ledger["steps"]
            .as_array()
            .context("cleanup steps")?
            .iter()
            .filter_map(|entry| entry["resource"].as_str().map(str::to_owned))
            .collect();
        let (remaining, scheduled) = loop {
            let (mut remaining, mut scheduled) = (Vec::new(), Vec::new());
            for resource in &resources {
                match self.admin.get_if_exists(resource)? {
                    None => {}
                    Some(value)
                        if resource.starts_with("groups/")
                            && !value["marked_for_deletion_on"].is_null() =>
                    {
                        scheduled.push(json!({"resource": resource, "full_path": value["full_path"], "marked_for_deletion_on": value["marked_for_deletion_on"]}));
                    }
                    Some(_) => remaining.push(resource.clone()),
                }
            }
            if remaining.is_empty() || std::time::Instant::now() >= deadline {
                break (remaining, scheduled);
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        };
        ledger["status"] = json!(match (remaining.is_empty(), scheduled.is_empty()) {
            (true, true) => "deleted",
            (true, false) => "deleted_except_scheduled_group",
            (false, _) => "deletion_requested",
        });
        ledger["remaining"] = json!(remaining);
        ledger["scheduled_for_deletion"] = json!(scheduled);
        write_json(&path, &ledger)?;
        Ok(ledger)
    }

    /// Requests deletion of one project or group whose `field` equals one of
    /// the `accepted` paths, then permanently removes it once GitLab reports
    /// it as scheduled for deletion.
    fn remove(
        &self,
        resource: &str,
        field: &str,
        accepted: &[String],
        owned: impl Fn(&Value) -> bool,
    ) -> Result<&'static str> {
        let Some(mut current) = self.admin.get_if_exists(resource)? else {
            return Ok("absent");
        };
        let verify = |value: &Value| -> Result<()> {
            ensure!(
                value[field]
                    .as_str()
                    .is_some_and(|path| accepted.iter().any(|candidate| candidate == path))
                    && owned(value),
                "{resource} is not owned by this campaign"
            );
            Ok(())
        };
        verify(&current)?;
        if current["marked_for_deletion_on"].is_null() {
            self.admin.delete(resource)?;
            match self.admin.get_if_exists(resource)? {
                None => return Ok("deleted"),
                Some(value) => current = value,
            }
            verify(&current)?;
        }
        if current["marked_for_deletion_on"].is_null() {
            return Ok("deletion_requested");
        }
        if resource.starts_with("groups/") && current["parent_id"].is_null() {
            return Ok("scheduled_for_deletion");
        }
        let full_path = current[field].as_str().context("scheduled path")?;
        self.admin.delete(&format!(
            "{resource}?permanently_remove=true&full_path={}",
            encode(full_path)
        ))?;
        Ok("permanent_removal_requested")
    }

    /// Saves operation evidence under `evidence/<kind>/NNN-<label>.json`.
    pub fn evidence(&self, kind: &str, label: &str, value: &Value) -> Result<PathBuf> {
        let directory = self.directory.join("evidence").join(kind);
        fs::create_dir_all(&directory)?;
        let mut sequence = 0_u64;
        for entry in fs::read_dir(&directory)? {
            if let Some((prefix, _)) = entry?.file_name().to_string_lossy().split_once('-')
                && let Ok(previous) = prefix.parse::<u64>()
            {
                sequence = sequence.max(previous);
            }
        }
        let path = directory.join(format!("{:03}-{label}.json", sequence + 1));
        let text = serde_json::to_string(value)?;
        ensure!(
            !self.tokens.iter().any(|token| text.contains(token)),
            "evidence would contain a credential"
        );
        write_json(&path, value)?;
        Ok(path)
    }
}

/// Commits a family change to `branch`, creating the branch from `start`
/// when given. Each written path is created or updated according to whether
/// it exists at the commit's starting point.
pub fn commit_change(
    api: &Api,
    project: &Value,
    branch: &str,
    start: Option<&str>,
    change: &family::Change,
    message: &str,
    executable: &[String],
) -> Result<String> {
    let reference = start.unwrap_or(branch);
    let mut actions = Vec::new();
    for (path, content) in &change.write {
        let exists = api
            .get_if_exists(&format!(
                "projects/{project}/repository/files/{}?ref={}",
                encode(path),
                encode(reference)
            ))?
            .is_some();
        let mut action = json!({"action": if exists { "update" } else { "create" }, "file_path": path, "content": content});
        if executable.contains(path) {
            action["execute_filemode"] = json!(true);
        }
        actions.push(action);
    }
    actions.extend(
        change
            .delete
            .iter()
            .map(|path| json!({"action": "delete", "file_path": path})),
    );
    let mut body = json!({"branch": branch, "commit_message": message, "actions": actions});
    if let Some(start) = start {
        body["start_branch"] = json!(start);
    }
    Ok(
        api.post(&format!("projects/{project}/repository/commits"), &body)?["id"]
            .as_str()
            .context("commit id")?
            .to_owned(),
    )
}
