use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::campaign::{
    self, Campaign, Role, Target,
    container::{self, ContainedRun, IMAGE, Program, WithholdSpec},
    digest, family,
    mutation::InlinePosition,
};
use crate::fixture::{root, write_json};
use crate::harness::{
    files::FileServer,
    proxy::{Proxy, ProxyConfig},
};

#[derive(Args)]
pub struct Selection {
    /// Campaign manifest printed by `setup-families`.
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    family: String,
    /// Merge request label; defaults to the family's first merge request.
    #[arg(long)]
    mr: Option<String>,
}

impl Selection {
    fn open(&self) -> Result<(Campaign, Target)> {
        let campaign = Campaign::load(&self.manifest)?;
        let target = campaign.target(&self.family, self.mr.as_deref())?;
        Ok((campaign, target))
    }
}

#[derive(Args)]
pub struct WithholdArgs {
    /// Method of the mutation whose response is withheld, or `*`.
    #[arg(long, requires = "withhold_path")]
    withhold_method: Option<String>,
    /// Request path pattern; `*` matches one segment, `**` several.
    #[arg(long, requires = "withhold_method")]
    withhold_path: Option<String>,
    /// One-based count among matching mutations.
    #[arg(long, default_value_t = 1)]
    withhold_occurrence: u64,
    /// `close`, `hold:SECONDS` or `delay:SECONDS`.
    #[arg(long, default_value = "close")]
    withhold_response: String,
}

#[derive(Args)]
pub struct PlacementArgs {
    /// Checkout below the campaign directory to run in instead of the merge
    /// request's current one.
    #[arg(long)]
    checkout: Option<PathBuf>,
    /// Publication-state directory below the campaign directory to use
    /// instead of the campaign's.
    #[arg(long)]
    state: Option<PathBuf>,
    /// Crash point for a fault-injection `rkc`, passed as `RKC_FAULT`.
    #[arg(long, conflicts_with = "probe_note")]
    fault: Option<String>,
}

impl WithholdArgs {
    fn spec(&self) -> Option<WithholdSpec> {
        Some(WithholdSpec {
            method: self.withhold_method.clone()?,
            path: self.withhold_path.clone()?,
            occurrence: self.withhold_occurrence,
            response: self.withhold_response.clone(),
        })
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Create one campaign namespace with a project per family.
    SetupFamilies {
        /// Comma-separated family names; all families when omitted.
        #[arg(long, value_delimiter = ',')]
        families: Vec<String>,
    },
    /// List families, their merge requests and overlays.
    Families,
    /// Write a family's files after its base and the given overlays.
    Materialize {
        #[arg(long)]
        family: String,
        /// Start from this merge request's overlay.
        #[arg(long)]
        mr: Option<String>,
        /// Further overlays, applied in order.
        #[arg(long = "overlay")]
        overlays: Vec<String>,
        #[arg(long)]
        dir: PathBuf,
    },
    /// Commit a named overlay to the merge request branch.
    Apply {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        overlay: String,
        /// Commit to the merge request's target branch instead.
        #[arg(long)]
        to_target: bool,
    },
    /// Rewrite the merge request history and force-push it.
    ForcePush {
        #[command(flatten)]
        selection: Selection,
        /// Branch, commit or `initial`; defaults to the target branch head.
        #[arg(long)]
        onto: Option<String>,
        #[arg(long)]
        squash: bool,
        #[arg(long)]
        message: Option<String>,
    },
    /// Change the target branch while keeping the source head.
    Retarget {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        branch: String,
        /// Branch, commit or `initial` to create the target branch from.
        #[arg(long)]
        from: String,
        /// Overlay committed to the new target branch.
        #[arg(long)]
        overlay: Option<String>,
    },
    EditDescription {
        #[command(flatten)]
        selection: Selection,
        #[arg(long, conflicts_with = "file", required_unless_present = "file")]
        text: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Add a general note as the author or commenter.
    Note {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long)]
        body: String,
    },
    /// Start an unmanaged diff thread as the author or commenter.
    InlineNote {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        old_path: Option<String>,
        /// New-file line.
        #[arg(long)]
        line: u64,
        /// Old-file line, required for an unchanged context line.
        #[arg(long)]
        old_line: Option<u64>,
        #[arg(long)]
        body: String,
    },
    /// Reply in a discussion, selected directly or through one of its notes.
    Reply {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long, conflicts_with = "note", required_unless_present = "note")]
        discussion: Option<String>,
        #[arg(long)]
        note: Option<u64>,
        #[arg(long)]
        body: String,
    },
    EditNote {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long)]
        note: u64,
        #[arg(long)]
        body: String,
    },
    DeleteNote {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long)]
        note: u64,
    },
    /// Resolve or reopen a discussion as the author or commenter.
    Resolve {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long)]
        discussion: String,
        #[arg(long)]
        unresolve: bool,
    },
    /// Create many general notes so discussions span several pages.
    BulkNotes {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as")]
        role: String,
        #[arg(long, default_value_t = 105)]
        count: u32,
        #[arg(long, default_value = "Status check")]
        prefix: String,
    },
    /// Delete the root note of the reviewer's managed thread for a finding.
    DeleteRoot {
        #[command(flatten)]
        selection: Selection,
        #[arg(long = "as", default_value = "author")]
        role: String,
        #[arg(long)]
        finding: u64,
    },
    /// Record an independent REST observation under a unique name.
    Observe {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        name: String,
    },
    /// Compare two named observations.
    Compare {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        before: String,
        #[arg(long)]
        after: String,
    },
    /// Run rkc in the merge request's current checkout.
    Rkc {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        rkc: Option<PathBuf>,
        /// Token role: reviewer, read-only, author or commenter.
        #[arg(long, default_value = "reviewer")]
        token: String,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Run rkc, or a probe write, in a container behind the proxy.
    ContainedRun {
        #[command(flatten)]
        selection: Selection,
        /// Docker network of the GitLab service.
        #[arg(long)]
        network: String,
        /// GitLab `host:port` on that network; defaults to `gitlab:<server port>`.
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long, default_value = IMAGE)]
        image: String,
        #[arg(long, default_value = "rk-fixture-")]
        container_prefix: String,
        #[arg(long, conflicts_with = "probe_note")]
        rkc: Option<PathBuf>,
        /// Write this general note with the fixture client instead of running rkc.
        #[arg(long)]
        probe_note: Option<String>,
        #[arg(long, default_value = "reviewer")]
        token: String,
        #[arg(long)]
        config: Option<PathBuf>,
        #[command(flatten)]
        withhold: WithholdArgs,
        #[arg(long)]
        logs: Option<PathBuf>,
        #[arg(long, default_value_t = 600)]
        timeout_seconds: u64,
        #[command(flatten)]
        placement: Box<PlacementArgs>,
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Run the recording proxy in the foreground until terminated.
    Proxy {
        #[arg(long)]
        listen: SocketAddr,
        #[arg(long)]
        upstream: String,
        #[arg(long)]
        log: PathBuf,
        #[command(flatten)]
        withhold: WithholdArgs,
        /// Stop after this many seconds instead of running until terminated.
        #[arg(long)]
        seconds: Option<u64>,
    },
    /// Count exchanges and mutation requests in a proxy log.
    ProxySummary {
        #[arg(long)]
        log: PathBuf,
    },
    /// Serve a directory of trusted skills on loopback until terminated.
    ServeFiles {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        listen: SocketAddr,
        #[arg(long)]
        log: PathBuf,
        /// Stop after this many seconds instead of running until terminated.
        #[arg(long)]
        seconds: Option<u64>,
    },
    #[command(hide = true)]
    ProxyExec {
        #[arg(long)]
        listen: SocketAddr,
        #[arg(long)]
        upstream: String,
        #[arg(long)]
        log: PathBuf,
        #[command(flatten)]
        withhold: WithholdArgs,
        #[arg(long)]
        stdout: PathBuf,
        #[arg(long)]
        stderr: PathBuf,
        #[arg(last = true, required = true)]
        program: Vec<String>,
    },
    #[command(hide = true)]
    ProbeWrite {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token_env: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        mr: String,
        #[arg(long)]
        body: String,
        #[arg(long)]
        output: PathBuf,
    },
}

fn print(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn rkc_binary(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("RKC_E2E_BINARY").map(PathBuf::from))
        .unwrap_or_else(|| root().join("target/debug/rkc"))
}

fn proxy_config(
    listen: SocketAddr,
    upstream: String,
    log: PathBuf,
    withhold: &WithholdArgs,
) -> Result<ProxyConfig> {
    Ok(ProxyConfig {
        listen,
        upstream,
        log,
        withhold: withhold.spec().map(|spec| spec.rule()).transpose()?,
    })
}

/// Blocks for `seconds`, or until the process is terminated.
fn serve_for(seconds: Option<u64>) {
    match seconds {
        Some(seconds) => std::thread::sleep(Duration::from_secs(seconds)),
        None => loop {
            std::thread::park();
        },
    }
}

fn materialize(
    name: &str,
    mr: Option<&str>,
    overlays: &[String],
    directory: &Path,
) -> Result<Value> {
    let family = family::load(&family::root(), name)?;
    let mut sequence: Vec<&str> = Vec::new();
    if let Some(label) = mr {
        sequence.push(&family.merge_request(label)?.overlay);
    }
    sequence.extend(overlays.iter().map(String::as_str));
    let files = family.materialize(&sequence)?;
    ensure!(
        !directory.exists() || fs::read_dir(directory)?.next().is_none(),
        "materialize needs a new or empty directory"
    );
    for (path, text) in &files {
        let destination = directory.join(path);
        fs::create_dir_all(destination.parent().context("file parent")?)?;
        fs::write(&destination, text)?;
        if family.descriptor.executable.contains(path) {
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(
        json!({"family": name, "overlays": sequence, "files": files.keys().collect::<Vec<_>>(), "directory": directory}),
    )
}

/// Observes every family merge request and prints counts.
pub fn inspect(manifest: &Path) -> Result<()> {
    let campaign = Campaign::load(manifest)?;
    let run = fs::read_dir(campaign.directory.join("evidence/inspections"))
        .map_or(0, Iterator::count)
        + 1;
    let mut rows = Vec::new();
    for (name, record) in campaign.manifest["families"]
        .as_object()
        .context("families")?
    {
        for label in record["merge_requests"]
            .as_object()
            .context("merge requests")?
            .keys()
        {
            let target = campaign.target(name, Some(label))?;
            let (path, summary) =
                campaign.observe(&target, &format!("inspect-{run}-{name}-{label}"))?;
            rows.push(json!({"family": name, "merge_request": label, "url": summary["url"], "head": summary["diff_refs"]["head_sha"], "target": summary["target_branch"], "versions": summary["versions"]["count"], "notes": summary["notes"]["total"], "system_notes": summary["notes"]["system"], "discussions": summary["discussions"]["total"], "resolvable": summary["discussions"]["resolvable"], "resolved": summary["discussions"]["resolved"], "observation": path}));
        }
    }
    let output = json!({"namespace": campaign.manifest["namespace"], "merge_requests": rows});
    campaign.evidence("inspections", "inspect", &output)?;
    print(&output)
}

pub fn cleanup(manifest: &Path) -> Result<()> {
    print(&Campaign::load(manifest)?.cleanup()?)
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::SetupFamilies { families } => {
            let campaign = Campaign::setup(&families)?;
            let mut output = json!({"manifest": campaign.manifest_path(), "namespace": campaign.manifest["namespace"], "merge_requests": {}});
            for (name, record) in campaign.manifest["families"]
                .as_object()
                .context("families")?
            {
                for (label, mr) in record["merge_requests"]
                    .as_object()
                    .context("merge requests")?
                {
                    output["merge_requests"][format!("{name}/{label}")] = mr["url"].clone();
                }
            }
            print(&output)
        }
        Command::Families => {
            let mut output = Vec::new();
            for name in family::names(&family::root())? {
                let family = family::load(&family::root(), &name)?;
                output.push(json!({"family": name, "project": family.descriptor.project, "merge_requests": family.descriptor.merge_requests.iter().map(|request| json!({"label": request.label, "branch": request.branch, "overlay": request.overlay})).collect::<Vec<_>>(), "overlays": family.descriptor.overlays.keys().collect::<Vec<_>>()}));
            }
            print(&json!(output))
        }
        Command::Materialize {
            family,
            mr,
            overlays,
            dir,
        } => print(&materialize(&family, mr.as_deref(), &overlays, &dir)?),
        Command::Apply {
            selection,
            overlay,
            to_target,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.apply(&target, &overlay, to_target)?)
        }
        Command::ForcePush {
            selection,
            onto,
            squash,
            message,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.force_push(&target, onto.as_deref(), squash, message.as_deref())?)
        }
        Command::Retarget {
            selection,
            branch,
            from,
            overlay,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.retarget(&target, &branch, &from, overlay.as_deref())?)
        }
        Command::EditDescription {
            selection,
            text,
            file,
        } => {
            let (mut campaign, target) = selection.open()?;
            let description = match (text, file) {
                (Some(text), _) => text,
                (None, Some(file)) => fs::read_to_string(file)?,
                (None, None) => anyhow::bail!("give --text or --file"),
            };
            print(&campaign.edit_description(&target, &description)?)
        }
        Command::Note {
            selection,
            role,
            body,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.note(&target, Role::parse(&role)?, &body)?)
        }
        Command::InlineNote {
            selection,
            role,
            path,
            old_path,
            line,
            old_line,
            body,
        } => {
            let (mut campaign, target) = selection.open()?;
            let position = InlinePosition {
                path: &path,
                old_path: old_path.as_deref(),
                line,
                old_line,
            };
            print(&campaign.inline_note(&target, Role::parse(&role)?, position, &body)?)
        }
        Command::Reply {
            selection,
            role,
            discussion,
            note,
            body,
        } => {
            let (mut campaign, target) = selection.open()?;
            let discussion = match (discussion, note) {
                (Some(discussion), _) => discussion,
                (None, Some(note)) => campaign.discussion_of(&target, note)?,
                (None, None) => anyhow::bail!("give --discussion or --note"),
            };
            print(&campaign.reply(&target, Role::parse(&role)?, &discussion, &body)?)
        }
        Command::EditNote {
            selection,
            role,
            note,
            body,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.edit_note(&target, Role::parse(&role)?, note, &body)?)
        }
        Command::DeleteNote {
            selection,
            role,
            note,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.delete_note(&target, Role::parse(&role)?, note)?)
        }
        Command::Resolve {
            selection,
            role,
            discussion,
            unresolve,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.resolve(&target, Role::parse(&role)?, &discussion, !unresolve)?)
        }
        Command::BulkNotes {
            selection,
            role,
            count,
            prefix,
        } => {
            let (mut campaign, target) = selection.open()?;
            let mut result = campaign.bulk_notes(&target, Role::parse(&role)?, count, &prefix)?;
            if let Some(object) = result.as_object_mut() {
                object.remove("notes");
            }
            print(&result)
        }
        Command::DeleteRoot {
            selection,
            role,
            finding,
        } => {
            let (mut campaign, target) = selection.open()?;
            print(&campaign.delete_root(&target, Role::parse(&role)?, finding)?)
        }
        Command::Observe { selection, name } => {
            let (campaign, target) = selection.open()?;
            let (path, summary) = campaign.observe(&target, &name)?;
            print(&json!({"observation": path, "summary": digest::compact(&summary)}))
        }
        Command::Compare {
            manifest,
            before,
            after,
        } => {
            let campaign = Campaign::load(&manifest)?;
            let (old, new) = (
                campaign.load_observation(&before)?,
                campaign.load_observation(&after)?,
            );
            ensure!(
                old["family"] == new["family"] && old["merge_request"] == new["merge_request"],
                "observations describe different merge requests"
            );
            let changes = digest::compare(&old["summary"], &new["summary"]);
            campaign.evidence("comparisons", &format!("{before}-to-{after}"), &changes)?;
            print(&json!({"before": before, "after": after, "changes": changes}))
        }
        Command::Rkc {
            selection,
            rkc,
            token,
            config,
            arguments,
        } => {
            let (mut campaign, target) = selection.open()?;
            let output = campaign.rkc(
                &target,
                &rkc_binary(rkc),
                Role::parse(&token)?,
                config.as_deref(),
                &arguments,
            )?;
            std::io::Write::write_all(&mut std::io::stdout(), &output.stdout)?;
            std::io::Write::write_all(&mut std::io::stderr(), &output.stderr)?;
            std::process::exit(output.status.code().unwrap_or(1));
        }
        Command::ContainedRun {
            selection,
            network,
            upstream,
            image,
            container_prefix,
            rkc,
            probe_note,
            token,
            config,
            withhold,
            logs,
            timeout_seconds,
            placement,
            arguments,
        } => {
            let (mut campaign, target) = selection.open()?;
            let program = match probe_note {
                Some(body) => {
                    ensure!(arguments.is_empty(), "a probe write takes no rkc arguments");
                    Program::Probe { body }
                }
                None => Program::Rkc {
                    binary: rkc_binary(rkc),
                    arguments,
                },
            };
            let result = campaign.contained_run(
                &target,
                ContainedRun {
                    image,
                    network,
                    upstream,
                    prefix: container_prefix,
                    program,
                    role: Role::parse(&token)?,
                    config,
                    withhold: withhold.spec(),
                    logs,
                    timeout: Duration::from_secs(timeout_seconds),
                    checkout: placement.checkout,
                    state: placement.state,
                    fault: placement.fault,
                },
            )?;
            print(&result)?;
            std::process::exit(
                result["container_exit"]
                    .as_i64()
                    .map_or(1, |code| code as i32),
            );
        }
        Command::Proxy {
            listen,
            upstream,
            log,
            withhold,
            seconds,
        } => {
            let proxy = Proxy::start(proxy_config(listen, upstream, log.clone(), &withhold)?)?;
            print(&json!({"listening": proxy.address(), "log": log}))?;
            serve_for(seconds);
            proxy.stop(Duration::from_secs(30))?;
            print(&crate::harness::proxy::summarize(&log)?)
        }
        Command::ProxySummary { log } => print(&crate::harness::proxy::summarize(&log)?),
        Command::ServeFiles {
            root,
            listen,
            log,
            seconds,
        } => {
            let server = FileServer::start(&root, listen, &log)?;
            print(&json!({"listening": server.address(), "root": root, "log": log}))?;
            serve_for(seconds);
            server.stop()
        }
        Command::ProxyExec {
            listen,
            upstream,
            log,
            withhold,
            stdout,
            stderr,
            program,
        } => {
            let code = container::proxy_exec(
                proxy_config(listen, upstream, log, &withhold)?,
                &stdout,
                &stderr,
                &program,
            )?;
            print(&json!({"child_exit": code}))?;
            std::process::exit(code);
        }
        Command::ProbeWrite {
            server,
            token_env,
            project,
            mr,
            body,
            output,
        } => {
            let result = container::probe_write(&server, &token_env, &project, &mr, &body)?;
            write_json(&output, &result)?;
            print(&result)
        }
    }
}

/// Whether a manifest belongs to the campaign owner rather than the
/// single-project fixture.
pub fn is_campaign(manifest: &Path) -> Result<bool> {
    Ok(crate::fixture::read_json(manifest)?["schema_version"] == campaign::SCHEMA)
}
