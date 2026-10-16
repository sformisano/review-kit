//! Review-run steps of the live scenarios. Each review workspace is its own
//! Git worktree below the fixture directory, so runs, drafts and accepted
//! assessments stay separate per scenario step. Every worktree uses the
//! fixture configuration and therefore shares one publication journal per
//! merge request and reviewer.

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    assessments,
    fixture::{CliOptions, Fixture, process, read_json, write_json},
};

/// One stored review run and the worktree that holds it.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub worktree: PathBuf,
    pub dir: PathBuf,
    /// The `run start` output.
    pub started: Value,
}

impl Run {
    /// Opens the run stored in `dir`, a `.review-kit/review-runs/<id>` directory.
    pub fn load(dir: &Path) -> Result<Self> {
        let dir = dir.canonicalize().context("review run directory")?;
        let manifest = read_json(dir.join("review-run.json"))?;
        let id = manifest["run_id"].as_str().context("run ID")?.to_owned();
        let worktree = dir
            .ancestors()
            .nth(3)
            .context("review run worktree")?
            .to_owned();
        ensure!(
            dir == worktree.join(".review-kit/review-runs").join(&id),
            "{} is not a stored review run",
            dir.display()
        );
        Ok(Self {
            id,
            worktree,
            dir,
            started: Value::Null,
        })
    }

    pub fn draft_path(&self) -> PathBuf {
        self.dir.join("review-assessment.draft.json")
    }

    pub fn draft(&self) -> Result<Value> {
        read_json(self.draft_path())
    }

    /// Write the draft with the checker confirmations its validated blocking findings need.
    pub fn write_draft(&self, value: &Value) -> Result<()> {
        let mut value = value.clone();
        assessments::confirm_blocking(&mut value);
        write_json(self.draft_path(), &value)
    }

    pub fn manifest(&self) -> Result<Value> {
        read_json(self.dir.join("review-run.json"))
    }

    pub fn context(&self) -> Result<Value> {
        read_json(self.dir.join("review-context.json"))
    }

    /// The exact assessment bytes referenced by the run manifest.
    pub fn accepted(&self) -> Result<Value> {
        let manifest = self.manifest()?;
        let reference = manifest["review_assessment"]
            .as_str()
            .context("accepted assessment reference")?;
        let hex = reference
            .strip_prefix("sha256:")
            .context("assessment reference")?;
        read_json(
            self.dir
                .join("accepted")
                .join(hex)
                .join("review-assessment.json"),
        )
    }
}

impl Fixture {
    /// Creates an empty Git worktree for review runs. rkc captures every
    /// reviewed file from GitLab, so the worktree needs no checkout content.
    pub fn worktree(&mut self, label: &str) -> Result<PathBuf> {
        let path = self.directory.join("worktrees").join(label);
        ensure!(!path.exists(), "worktree {label} already exists");
        fs::create_dir_all(&path)?;
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(&path).args(["init", "--quiet"]);
        process::execute_git(
            &self.directory,
            &self.tokens,
            &format!("worktree-{label}"),
            command,
            Duration::from_secs(60),
        )?;
        Ok(path.canonicalize()?)
    }

    /// Runs `rkc` in `worktree` and returns its JSON output.
    pub fn rkc(
        &mut self,
        label: &str,
        worktree: &Path,
        arguments: &[&str],
        options: CliOptions<'_>,
    ) -> Result<Value> {
        let mut command = self.cli_command(arguments, &options)?;
        let output = process::execute_in(
            &self.directory,
            &self.tokens,
            label,
            &mut command,
            Some(options.expected),
            worktree,
        )?;
        serde_json::from_slice(&output.stdout).context("CLI JSON output")
    }

    /// Starts and selects a new run for `mr` in `worktree`.
    pub fn start(
        &mut self,
        label: &str,
        worktree: &Path,
        mr: &Value,
        options: CliOptions<'_>,
    ) -> Result<Run> {
        let url = mr["url"].as_str().context("MR URL")?;
        let started = self.rkc(label, worktree, &["run", "start", "--mr", url], options)?;
        ensure!(started["status"] == "created", "run start: {started}");
        let id = started["run_id"].as_str().context("run ID")?.to_owned();
        let dir = worktree.join(started["run_directory"].as_str().context("run directory")?);
        Ok(Run {
            id,
            worktree: worktree.to_owned(),
            dir,
            started,
        })
    }

    /// Accept the complete current assessment and expect newly stored bytes.
    pub fn accept(&mut self, label: &str, run: &Run, options: CliOptions<'_>) -> Result<Value> {
        let accepted = self.rkc(label, &run.worktree, &["accept"], options)?;
        ensure!(
            accepted["status"] == "stored" && accepted["run_id"] == run.id.as_str(),
            "accept assessment: {accepted}"
        );
        Ok(accepted)
    }

    /// Start a run and author its declared policy rows and captured carried data.
    pub fn prepare(
        &mut self,
        label: &str,
        worktree: &Path,
        mr: &Value,
        options: CliOptions<'_>,
    ) -> Result<(Run, Value)> {
        let run = self.start(&format!("{label}-start"), worktree, mr, options)?;
        let mut draft = assessments::declare_policies(run.draft()?);
        let context = review_kit_json_adapter::decode::<review_kit_json_adapter::ContextDocument>(
            &fs::read(run.dir.join("review-context.json"))?,
        )?;
        let bundle = context.bundle()?;
        for slot in draft["findings"].as_array_mut().context("finding slots")? {
            let finding = bundle
                .findings()
                .iter()
                .find(|finding| Some(finding.id().get()) == slot["id"].as_u64())
                .context("captured current carried finding")?;
            slot["reassessment"] =
                serde_json::to_value(review_kit_json_adapter::domain::FindingData::from(finding))?;
        }
        run.write_draft(&draft)?;
        Ok((run, draft))
    }
}

/// The first diagnostic code of a command's output, or an empty string.
pub fn first_code(value: &Value) -> &str {
    value["diagnostics"][0]["code"].as_str().unwrap_or_default()
}

/// Codes of a failed command's diagnostics, in output order.
pub fn codes(value: &Value) -> Vec<String> {
    value["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["code"].as_str().map(str::to_owned))
        .collect()
}
