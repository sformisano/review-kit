//! Runs a program inside a disposable container attached to the GitLab
//! service network, behind the recording proxy.
//!
//! The proxy listens on the container's own `127.0.0.1` at the port of the
//! campaign server and forwards to the GitLab service on the network, so the
//! canonical server URL, and therefore the merge request and reviewer
//! identity, is the same inside and outside the container. The container
//! receives only bind mounts for the fixture binary, the candidate `rkc`, the
//! host `git` executable that `rkc` uses to find its worktree, the host CA
//! bundle that the HTTP clients load at start even for plain HTTP, a
//! generated configuration, one checkout, the publication-state directory
//! and a log directory. A run can name another checkout or state directory
//! below the campaign directory, and a crash point for a fault-injection
//! `rkc`. The reviewer token travels in a mode 0600 environment file that
//! is removed as soon as the container is created. The container is removed
//! after it exits, also when the run fails or times out.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::{Campaign, Role, Target};
use crate::fixture::{Api, private_file, read_json, write_json};
use crate::harness::proxy::{self, PathPattern, Proxy, ProxyConfig, WithheldResponse, Withhold};

pub const IMAGE: &str = "ubuntu:26.04";
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

/// The withheld exchange, as forwarded to the in-container proxy.
#[derive(Debug, Clone)]
pub struct WithholdSpec {
    pub method: String,
    pub path: String,
    pub occurrence: u64,
    /// `close` or `hold:SECONDS`.
    pub response: String,
}

impl WithholdSpec {
    pub fn rule(&self) -> Result<Withhold> {
        ensure!(self.occurrence >= 1, "withheld occurrence starts at 1");
        Ok(Withhold {
            method: self.method.clone(),
            path: PathPattern::parse(&self.path)?,
            occurrence: self.occurrence,
            response: proxy::parse_withheld_response(&self.response)?,
        })
    }

    fn arguments(&self) -> Vec<String> {
        vec![
            "--withhold-method".to_owned(),
            self.method.clone(),
            "--withhold-path".to_owned(),
            self.path.clone(),
            "--withhold-occurrence".to_owned(),
            self.occurrence.to_string(),
            "--withhold-response".to_owned(),
            self.response.clone(),
        ]
    }
}

pub enum Program {
    /// The candidate `rkc` with arguments after `--config`.
    Rkc {
        binary: PathBuf,
        arguments: Vec<String>,
    },
    /// One general note written by the fixture client as the selected role.
    Probe { body: String },
}

pub struct ContainedRun {
    pub image: String,
    pub network: String,
    /// GitLab `host:port` on the container network.
    pub upstream: Option<String>,
    /// Container name prefix; names end with the namespace and a sequence.
    pub prefix: String,
    pub program: Program,
    pub role: Role,
    pub config: Option<PathBuf>,
    pub withhold: Option<WithholdSpec>,
    pub logs: Option<PathBuf>,
    pub timeout: Duration,
    /// Checkout mounted as the worktree instead of the merge request's
    /// current one; it must lie below the campaign directory.
    pub checkout: Option<PathBuf>,
    /// Publication-state directory mounted instead of the campaign's; it must
    /// lie below the campaign directory.
    pub state: Option<PathBuf>,
    /// Crash point that a fault-injection `rkc` receives as `RKC_FAULT`.
    pub fault: Option<String>,
}

struct Mount {
    source: PathBuf,
    target: &'static str,
    readonly: bool,
}

/// Removes the environment file when dropped, including on early returns.
struct EnvironmentFile(PathBuf);

impl Drop for EnvironmentFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Removes the named container when dropped unless removal already succeeded.
struct ContainerGuard {
    name: String,
    removed: bool,
}

impl ContainerGuard {
    fn remove(&mut self) -> Result<bool> {
        let output = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .stdin(Stdio::null())
            .output()?;
        let listed = Command::new("docker")
            .args([
                "ps",
                "--all",
                "--quiet",
                "--filter",
                &format!("name=^/{}$", self.name),
            ])
            .stdin(Stdio::null())
            .output()?;
        self.removed =
            output.status.success() && listed.status.success() && listed.stdout.is_empty();
        Ok(self.removed)
    }
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        if !self.removed {
            let _ = self.remove();
        }
    }
}

fn executable_on_path(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH is unset")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .with_context(|| format!("{name} is not on PATH"))?
        .canonicalize()
        .map_err(Into::into)
}

fn docker(arguments: &[String], log: &Path) -> Result<std::process::Output> {
    let output = Command::new("docker")
        .args(arguments)
        .stdin(Stdio::null())
        .output()?;
    private_file(
        log,
        [
            output.stdout.as_slice(),
            &b"\n--- stderr ---\n"[..],
            output.stderr.as_slice(),
        ]
        .concat(),
    )?;
    Ok(output)
}

impl Campaign {
    pub fn contained_run(&mut self, target: &Target, run: ContainedRun) -> Result<Value> {
        let mr = self.verify_merge_request(target)?;
        let rule = run.withhold.as_ref().map(WithholdSpec::rule).transpose()?;
        let server = url::Url::parse(self.server()?)?;
        ensure!(
            server.scheme() == "http" && server.host_str() == Some("127.0.0.1"),
            "the contained run needs an http://127.0.0.1 campaign server"
        );
        let port = server.port_or_known_default().context("server port")?;
        let upstream = run
            .upstream
            .clone()
            .unwrap_or_else(|| format!("gitlab:{port}"));
        let namespace = self.namespace()?.to_owned();
        let contained = self.directory.join("evidence/contained");
        fs::create_dir_all(&contained)?;
        let sequence = fs::read_dir(&contained)?.count() + 1;
        ensure!(
            Path::new(CA_BUNDLE).is_file(),
            "the host CA bundle {CA_BUNDLE} is missing"
        );
        let name = format!("{}{namespace}-{sequence}", run.prefix);
        ensure!(
            name.chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.')),
            "container name {name} contains unsupported characters"
        );
        let logs = run.logs.clone().unwrap_or_else(|| {
            contained.join(format!("{sequence:03}-{}-{}", target.family, target.label))
        });
        ensure!(
            !logs.exists(),
            "log directory {} already exists",
            logs.display()
        );
        for directory in [logs.clone(), logs.join("home"), logs.join("tmp")] {
            fs::create_dir_all(&directory)?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let logs = logs.canonicalize()?;
        let mut config = read_json(match &run.config {
            Some(config) => config.clone(),
            None => PathBuf::from(
                self.manifest["config"]
                    .as_str()
                    .context("campaign config")?,
            ),
        })?;
        ensure!(
            config["gitlab"]["server"].as_str() == Some(self.server()?),
            "the contained configuration must use the campaign server"
        );
        config["publication_state"] = json!("/rk/state");
        let token_env = config["gitlab"]["token_env"]
            .as_str()
            .unwrap_or("GITLAB_TOKEN")
            .to_owned();
        write_json(logs.join("config.json"), &config)?;
        let state = match &run.state {
            Some(state) => state.clone(),
            None => PathBuf::from(
                self.manifest["publication_state"]
                    .as_str()
                    .context("campaign publication state")?,
            ),
        };
        fs::create_dir_all(&state)?;
        let state = self.inside_campaign(&state)?;
        if let Some(fault) = &run.fault {
            ensure!(
                !fault.is_empty()
                    && fault.chars().all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '_' | '.' | ':')),
                "fault point {fault} contains unsupported characters"
            );
        }
        let fixture = std::env::current_exe()?.canonicalize()?;
        let mut mounts = vec![
            Mount {
                source: fixture,
                target: "/rk/bin/cli_fixture",
                readonly: true,
            },
            Mount {
                source: logs.join("config.json"),
                target: "/rk/config.json",
                readonly: true,
            },
            Mount {
                source: logs.clone(),
                target: "/rk/logs",
                readonly: false,
            },
            Mount {
                source: PathBuf::from(CA_BUNDLE),
                target: CA_BUNDLE,
                readonly: true,
            },
        ];
        let listen = format!("127.0.0.1:{port}");
        let (workdir, program) = match &run.program {
            Program::Rkc { binary, arguments } => {
                let checkout = match &run.checkout {
                    Some(checkout) => self.inside_campaign(checkout)?,
                    None => PathBuf::from(
                        self.record(target)?["source"]
                            .as_str()
                            .context("checkout")?,
                    ),
                };
                mounts.extend([
                    Mount {
                        source: binary.canonicalize().context("resolve rkc executable")?,
                        target: "/rk/bin/rkc",
                        readonly: true,
                    },
                    Mount {
                        source: executable_on_path("git")?,
                        target: "/usr/bin/git",
                        readonly: true,
                    },
                    Mount {
                        source: checkout.canonicalize()?,
                        target: "/rk/worktree",
                        readonly: false,
                    },
                    Mount {
                        source: state.clone(),
                        target: "/rk/state",
                        readonly: false,
                    },
                ]);
                let mut program = vec![
                    "/rk/bin/rkc".to_owned(),
                    "--config".to_owned(),
                    "/rk/config.json".to_owned(),
                ];
                program.extend(arguments.iter().cloned());
                ("/rk/worktree", program)
            }
            Program::Probe { body } => (
                "/rk/logs",
                vec![
                    "/rk/bin/cli_fixture".to_owned(),
                    "probe-write".to_owned(),
                    "--server".to_owned(),
                    format!("http://{listen}"),
                    "--token-env".to_owned(),
                    token_env.clone(),
                    "--project".to_owned(),
                    self.project_id(&target.family)?.to_string(),
                    "--mr".to_owned(),
                    mr["iid"].to_string(),
                    "--body".to_owned(),
                    body.clone(),
                    "--output".to_owned(),
                    "/rk/logs/probe.json".to_owned(),
                ],
            ),
        };
        let owner = fs::metadata(&self.directory)?;
        let environment = EnvironmentFile(
            self.directory
                .join("tmp")
                .join(format!("container-env-{sequence}")),
        );
        private_file(
            &environment.0,
            format!("{token_env}={}\n", self.token(run.role)?),
        )?;
        let mut create = vec![
            "create".to_owned(),
            "--name".to_owned(),
            name.clone(),
            "--label".to_owned(),
            format!("review-kit-fixture={namespace}"),
            "--network".to_owned(),
            run.network.clone(),
            "--user".to_owned(),
            format!("{}:{}", owner.uid(), owner.gid()),
            "--workdir".to_owned(),
            workdir.to_owned(),
            "--entrypoint".to_owned(),
            "/rk/bin/cli_fixture".to_owned(),
            "--env-file".to_owned(),
            environment.0.to_string_lossy().into_owned(),
            "--env".to_owned(),
            "HOME=/rk/logs/home".to_owned(),
            "--env".to_owned(),
            "TMPDIR=/rk/logs/tmp".to_owned(),
            "--cap-drop".to_owned(),
            "ALL".to_owned(),
            "--security-opt".to_owned(),
            "no-new-privileges".to_owned(),
        ];
        if let Some(fault) = &run.fault {
            create.push("--env".to_owned());
            create.push(format!("RKC_FAULT={fault}"));
        }
        for mount in &mounts {
            let source = mount.source.to_str().context("mount source is not UTF-8")?;
            ensure!(
                !source.contains(','),
                "mount source {source} contains a comma"
            );
            create.push("--mount".to_owned());
            create.push(format!(
                "type=bind,source={source},target={}{}",
                mount.target,
                if mount.readonly { ",readonly" } else { "" }
            ));
        }
        create.push(run.image.clone());
        let mut inner = vec![
            "proxy-exec".to_owned(),
            "--listen".to_owned(),
            listen.clone(),
            "--upstream".to_owned(),
            upstream.clone(),
            "--log".to_owned(),
            "/rk/logs/proxy.jsonl".to_owned(),
            "--stdout".to_owned(),
            "/rk/logs/child.stdout".to_owned(),
            "--stderr".to_owned(),
            "/rk/logs/child.stderr".to_owned(),
        ];
        if let Some(withhold) = &run.withhold {
            inner.extend(withhold.arguments());
        }
        inner.push("--".to_owned());
        inner.extend(program.iter().cloned());
        create.extend(inner.iter().cloned());
        let mut guard = ContainerGuard {
            name: name.clone(),
            removed: false,
        };
        let created = docker(&create, &logs.join("docker-create.log"));
        drop(environment);
        let created = created?;
        ensure!(
            created.status.success(),
            "docker create failed; see {}",
            logs.join("docker-create.log").display()
        );
        let started = Instant::now();
        let mut child = Command::new("docker")
            .args(["start", "--attach", &name])
            .stdin(Stdio::null())
            .stdout(fs::File::create(logs.join("container.stdout"))?)
            .stderr(fs::File::create(logs.join("container.stderr"))?)
            .spawn()?;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() > run.timeout {
                timed_out = true;
                guard.remove()?;
                break child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let inspected = docker(
            &[
                "inspect".to_owned(),
                "--format".to_owned(),
                "{{.State.ExitCode}}".to_owned(),
                name.clone(),
            ],
            &logs.join("docker-inspect.log"),
        )?;
        let exit_code = String::from_utf8_lossy(&inspected.stdout)
            .trim()
            .parse::<i32>()
            .ok();
        let removed = guard.remove()?;
        let summary = if logs.join("proxy.jsonl").exists() {
            proxy::summarize(&logs.join("proxy.jsonl"))?
        } else {
            Value::Null
        };
        for file in [
            "child.stdout",
            "child.stderr",
            "container.stdout",
            "container.stderr",
            "proxy.jsonl",
            "probe.json",
        ] {
            let path = logs.join(file);
            if path.exists() {
                let text = String::from_utf8_lossy(&fs::read(&path)?).into_owned();
                ensure!(
                    !self.tokens.iter().any(|token| text.contains(token)),
                    "{} contains a credential",
                    path.display()
                );
            }
        }
        let mut result = json!({
            "container": name,
            "image": run.image,
            "network": run.network,
            "listen": listen,
            "upstream": upstream,
            "role": run.role.key(),
            "fault": run.fault,
            "mounts": mounts.iter().map(|mount| json!({"source": mount.source, "target": mount.target, "readonly": mount.readonly})).collect::<Vec<_>>(),
            "command": inner,
            "withhold": rule.as_ref().map(|rule| json!({"method": rule.method, "path": run.withhold.as_ref().map(|spec| spec.path.clone()), "occurrence": rule.occurrence, "response": run.withhold.as_ref().map(|spec| spec.response.clone())})),
            "docker_start_status": status.code(),
            "container_exit": exit_code,
            "timed_out": timed_out,
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "container_removed": removed,
            "environment_file_removed": !self.directory.join("tmp").join(format!("container-env-{sequence}")).exists(),
            "proxy": summary,
            "logs": logs,
        });
        if let Program::Probe { body } = &run.program {
            result["probe"] = read_json(logs.join("probe.json")).unwrap_or(Value::Null);
            let notes = self
                .api(Role::Author)?
                .pages(&format!("{}/notes", self.merge_request_path(target)?))?;
            let author = self.user_id(run.role)?.clone();
            let stored: Vec<_> = notes
                .as_array()
                .context("notes")?
                .iter()
                .filter(|note| note["body"] == *body && note["author"]["id"] == author)
                .map(|note| note["id"].clone())
                .collect();
            result["independent_check"] = json!({"stored_note_ids": stored, "method": "fixture REST read of every note page after the container exited"});
        }
        write_json(logs.join("run.json"), &result)?;
        let evidence = self.evidence("contained-runs", "run", &result)?;
        result["evidence"] = json!(evidence);
        ensure!(removed, "container {name} was not removed");
        if timed_out {
            bail!(
                "contained run exceeded {} seconds; see {}",
                run.timeout.as_secs(),
                logs.display()
            );
        }
        Ok(result)
    }
}

impl Campaign {
    /// The canonical `path`, which must lie below the campaign directory.
    fn inside_campaign(&self, path: &Path) -> Result<PathBuf> {
        let path = path
            .canonicalize()
            .with_context(|| format!("resolve {}", path.display()))?;
        ensure!(
            path.starts_with(self.directory.canonicalize()?),
            "{} is outside the campaign directory",
            path.display()
        );
        Ok(path)
    }
}

/// Runs inside the container: starts the proxy, runs the program to
/// completion with its output in files, then stops the proxy after every
/// exchange, including a held response, has written its log entry.
pub fn proxy_exec(
    config: ProxyConfig,
    stdout: &Path,
    stderr: &Path,
    program: &[String],
) -> Result<i32> {
    let (executable, arguments) = program
        .split_first()
        .context("proxy-exec needs a program")?;
    let grace = match config.withhold.as_ref().map(|rule| rule.response) {
        Some(WithheldResponse::Hold(duration) | WithheldResponse::Delay(duration)) => {
            duration + Duration::from_secs(30)
        }
        _ => Duration::from_secs(30),
    };
    let proxy = Proxy::start(config)?;
    let status = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(fs::File::create(stdout)?)
        .stderr(fs::File::create(stderr)?)
        .status();
    let stopped = proxy.stop(grace);
    let status = status.with_context(|| format!("run {executable}"))?;
    stopped?;
    // A child ended by a signal reports 128 plus the signal number, as shells do.
    Ok(status.code().unwrap_or_else(|| {
        128 + std::os::unix::process::ExitStatusExt::signal(&status).unwrap_or(0)
    }))
}

/// Runs inside the container: reads the current user, then writes one
/// general note and reports whether its response arrived.
pub fn probe_write(
    server: &str,
    token_env: &str,
    project: &str,
    iid: &str,
    body: &str,
) -> Result<Value> {
    let token = std::env::var(token_env).with_context(|| format!("{token_env} is unset"))?;
    let api = Api::new(server, &token)?;
    let user = api.get("user")?;
    let write = match api.post(
        &format!("projects/{project}/merge_requests/{iid}/notes"),
        &json!({"body": body}),
    ) {
        Ok(note) => json!({"status": "response_received", "note": note["id"]}),
        Err(error) => json!({"status": "response_lost", "error": format!("{error:#}")}),
    };
    Ok(json!({"server": server, "user": user["id"], "write": write}))
}
