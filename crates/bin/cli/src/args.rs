use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::effects::EffectSelection;

/// Prepare, check and publish GitLab reviews from review runs in the current Git worktree.
#[derive(Debug, Parser)]
#[command(name = "rkc", version, about)]
pub struct Args {
    /// Trusted review configuration; defaults to ~/.review-kit/config.toml.
    #[arg(long, global = true, env = "RKC_CONFIG")]
    pub config: Option<PathBuf>,
    /// Rendering profile manifest; overrides render_profile in configuration.
    #[arg(long, global = true)]
    pub render_profile: Option<PathBuf>,
    /// Stderr progress: plain prints lines; auto uses a spinner in a terminal and lines otherwise; none disables progress.
    #[arg(long, global = true, value_enum, default_value = "plain")]
    pub progress: crate::progress::Mode,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Export editable rendering assets without GitLab configuration or credentials.
    RenderProfile {
        #[command(subcommand)]
        command: RenderProfileCommand,
    },
    /// Start, list, select, finish or stop review runs stored in this Git worktree.
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Report the selected review, retained files, publication state and next action.
    Status,
    /// Diagnose the selected review draft without accepting it.
    Check,
    /// Validate the complete review and freeze its exact bytes.
    Accept,
    /// Archive the review draft and prepare its successor.
    Revise {
        /// Explanation recorded in the successor draft.
        #[arg(long)]
        reason: String,
        /// Rebuild from accepted state after archiving unaccepted edits.
        #[arg(long)]
        regenerate: bool,
    },
    /// Plan, publish, recover or abandon the selected run's accepted review.
    Publish {
        /// review, none, or comma-separated provider effects; defaults to none,
        /// or update_summary with --abandon.
        #[arg(long)]
        effects: Option<EffectSelection>,
        /// Allow one resend of an absent uncertain creation after complete reconciliation reads.
        #[arg(long, value_enum, default_value = "once")]
        retry_uncertain: UncertainRetry,
        /// Plan without provider writes, publication flags or journal creation.
        #[arg(long, conflicts_with = "abandon")]
        plan: bool,
        /// Close the selected run's unfinished round with a public reason.
        #[arg(long, requires = "reason")]
        abandon: bool,
        /// Public abandonment reason.
        #[arg(long, requires = "abandon")]
        reason: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// Capture the merge request and start a run; with no --mr, use the selected run's merge request.
    Start {
        /// GitLab merge-request URL.
        #[arg(long)]
        mr: Option<String>,
        /// Start another run although an open run exists for this merge request and reviewer.
        #[arg(long)]
        new: bool,
    },
    /// List the runs stored in this worktree.
    List,
    /// Select a stored run after validating its retained identity and files.
    Select {
        /// Run ID reported by run start or run list.
        id: String,
    },
    /// Complete a clean accepted run locally without provider writes.
    Finish,
    /// Stop unfinished work without publishing it.
    Stop {
        /// Explanation recorded in the run manifest.
        #[arg(long)]
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum UncertainRetry {
    Once,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum RenderProfileCommand {
    /// Export the embedded manifest, schemas and templates; preserve existing files.
    Export {
        #[arg(long)]
        dir: PathBuf,
    },
}
