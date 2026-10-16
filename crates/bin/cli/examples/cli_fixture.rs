#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(
    dead_code,
    reason = "the fixture shares deterministic inputs with live tests"
)]
#[path = "../tests/e2e/assessments.rs"]
mod assessments;
#[path = "../tests/e2e/campaign.rs"]
mod campaign;
#[path = "cli_fixture/campaign_commands.rs"]
mod campaign_commands;
#[path = "../tests/e2e/controls.rs"]
mod controls;
#[path = "../tests/e2e/fixture.rs"]
mod fixture;
#[path = "../tests/e2e/harness.rs"]
mod harness;
#[path = "../tests/e2e/source.rs"]
mod source;
#[allow(dead_code, reason = "the fixture shares run steps with live tests")]
#[path = "../tests/e2e/workflow.rs"]
mod workflow;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fixture::{Fixture, write_json};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
struct Arguments {
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    Setup,
    Fix {
        #[arg(long)]
        manifest: PathBuf,
    },
    Recur {
        #[arg(long)]
        manifest: PathBuf,
    },
    Inspect {
        #[arg(long)]
        manifest: PathBuf,
    },
    Cleanup {
        #[arg(long)]
        manifest: PathBuf,
    },
    Controls {
        #[arg(long)]
        manifest: PathBuf,
        /// Run directory of the completed clean review: `<worktree>/.review-kit/review-runs/<id>`.
        #[arg(long)]
        run: PathBuf,
    },
    Cli {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        rkc: Option<PathBuf>,
        /// Merge request whose current source checkout is the working directory.
        #[arg(long, default_value = "review")]
        mr: String,
        #[arg(last = true, required = true)]
        arguments: Vec<String>,
    },
    #[command(flatten)]
    Campaign(Box<campaign_commands::Command>),
}

fn main() -> Result<()> {
    match Arguments::parse().command {
        Operation::Setup => {
            let fixture = Fixture::setup()?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"manifest": fixture.directory.join("manifest.json"), "review_mr": fixture.manifest["mrs"]["review"]["url"], "clean_mr": fixture.manifest["mrs"]["clean"]["url"]})
                )?
            );
        }
        Operation::Fix { manifest } => evolve(&manifest, false)?,
        Operation::Recur { manifest } => evolve(&manifest, true)?,
        Operation::Inspect { manifest } if campaign_commands::is_campaign(&manifest)? => {
            campaign_commands::inspect(&manifest)?
        }
        Operation::Inspect { manifest } => {
            let fixture = Fixture::load(&manifest)?;
            let mut output = json!({"phase": fixture.manifest["phase"], "project": fixture.manifest["project_path"], "mrs": {}});
            for (label, mr) in fixture.manifest["mrs"].as_object().context("fixture MRs")? {
                let observed = fixture.observe(label, mr)?;
                output["mrs"][label] = json!({"url": mr["url"], "head": observed["mr"]["diff_refs"]["head_sha"], "notes": observed["notes"].as_array().context("notes")?.len(), "discussions": observed["discussions"].as_array().context("discussions")?.len(), "resolved_threads": observed["discussions"].as_array().unwrap().iter().filter(|discussion| discussion["notes"][0]["resolved"] == true).count()});
            }
            write_json(fixture.directory.join("evidence/inspection.json"), &output)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Operation::Cleanup { manifest } if campaign_commands::is_campaign(&manifest)? => {
            campaign_commands::cleanup(&manifest)?
        }
        Operation::Cleanup { manifest } => Fixture::load(&manifest)?.cleanup()?,
        Operation::Campaign(command) => campaign_commands::run(*command)?,
        Operation::Controls { manifest, run } => {
            let mut fixture = Fixture::load(&manifest)?;
            controls::authority_and_abandonment(&mut fixture, &run)?;
            println!(
                "{}",
                json!({"status": "passed", "evidence": fixture.directory.join("authority")})
            );
        }
        Operation::Cli {
            manifest,
            rkc,
            mr,
            arguments,
        } => {
            let mut fixture = Fixture::load(&manifest)?;
            if let Some(binary) = rkc {
                fixture.binary = binary.canonicalize()?;
            }
            let worktree = PathBuf::from(
                fixture.manifest["mrs"][&mr]["source"]
                    .as_str()
                    .with_context(|| format!("fixture has no merge request {mr}"))?,
            );
            let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
            let output = fixture.agent_cli(&worktree, &arguments)?;
            std::io::Write::write_all(&mut std::io::stdout(), &output.stdout)?;
            std::io::Write::write_all(&mut std::io::stderr(), &output.stderr)?;
            std::process::exit(output.status.code().unwrap_or(1));
        }
    }
    Ok(())
}

fn evolve(manifest: &std::path::Path, recurrence: bool) -> Result<()> {
    let mut fixture = Fixture::load(manifest)?;
    fixture.evolve(recurrence)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"phase": fixture.manifest["phase"], "review_mr": fixture.manifest["mrs"]["review"]["url"], "head": fixture.manifest["mrs"]["review"]["head"]})
        )?
    );
    Ok(())
}
