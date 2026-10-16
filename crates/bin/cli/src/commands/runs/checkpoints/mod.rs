//! Diagnosis, acceptance and revision of the selected review.

pub mod accept;
pub mod check;
pub mod revise;

use review_kit_application::{
    publication::{Effects, Plan},
    use_cases::{
        checkpoints::Successor,
        publishing::{ManagedHistory, PublicationProgress, check_review_with_progress},
    },
};
use review_kit_filesystem_adapter::{RenderedFile, RunWriter, WorkingFile};
use review_kit_gitlab_adapter::validate_plan_bodies;
use review_kit_json_adapter::{CheckDocument, Diagnostic, checkpoints::AssessmentInputs};
use review_kit_json2md_adapter::Json2MdRenderer;
use serde_json::{Value, json};

use super::{
    chain::{DRAFT_FILE, publication_request},
    journal,
    loading::{LoadedRun, retained_source},
};
use crate::{
    commands::{rendering, shared::PREVIEW_NOTE},
    config::Config,
    error::CliError,
};

/// The diagnostic of an assessment other than the provider's unfinished round's.
const CHANGED_PENDING_ASSESSMENT: &str = "changed_pending_assessment";

/// Validation either provides a completely checked rendered plan or diagnostics.
pub type Checked = Result<Box<Plan>, Vec<Diagnostic>>;

#[review_kit_observation::operations]
#[helper]
fn diagnostic(code: &str, message: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code, message)]
}

/// The configured renderer, its identity and the trusted review workflow name.
pub struct Rendering {
    pub renderer: Json2MdRenderer,
    pub identity: Value,
    pub review_workflow: Option<String>,
}

/// Validate and render the complete review before its bytes can be accepted.
#[review_kit_observation::operations]
#[helper]
pub fn validate(
    run: &LoadedRun,
    bytes: &[u8],
    successor: Successor<'_>,
    rendering: &mut Rendering,
) -> Result<Checked, CliError> {
    let request = match publication_request(
        bytes,
        &AssessmentInputs {
            run_id: run.run_id(),
            context: &run.context,
            successor,
        },
        Effects::review(),
        rendering.review_workflow.as_deref(),
    ) {
        Ok(request) => request,
        Err(diagnostics) => return Ok(Err(diagnostics)),
    };
    let snapshot = run.context.snapshot.clone().into_snapshot()?;
    let mut source = retained_source(&run.dir)?;
    let result = check_review_with_progress(
        &request,
        &snapshot,
        &mut source,
        &mut rendering.renderer,
        &PublicationProgress::default(),
    );
    let document = CheckDocument::from_result(&result);
    Ok(match result {
        Ok(plan) => match validate_plan_bodies(&plan) {
            Ok(()) => Ok(Box::new(plan)),
            Err(error) => Err(diagnostic("gitlab_body", &error.to_string())),
        },
        Err(_) => Err(document.diagnostics),
    })
}

#[review_kit_observation::operations]
#[helper]
pub fn missing_draft() -> Vec<Diagnostic> {
    diagnostic(
        "missing_draft",
        "review-assessment.draft.json is missing; run rkc revise --reason TEXT --regenerate to prepare it",
    )
}

/// What to do about a rejected draft; `retry` ends "Fix the draft and …".
///
/// A provider round that only the publication state that started it can
/// finish comes first, because no draft change lets this run finish it.
#[review_kit_observation::operations]
#[helper]
pub fn rejected_next(
    config: &Config,
    run: &LoadedRun,
    diagnostics: &[Diagnostic],
    retry: &str,
) -> Result<String, CliError> {
    if let Some(round) = unfinished_round(config, run, diagnostics)? {
        return Ok(format!(
            "GitLab shows round {round} of this reviewer's summary unfinished. Accepting that round's exact assessment here would only lead rkc publish to refuse with journal_missing. {}",
            journal::unfinished_guidance(round)
        ));
    }
    Ok(rendering::restore_guidance(diagnostics)
        .map_or_else(|| format!("Fix {DRAFT_FILE} and {retry}."), str::to_owned))
}

/// The provider's unfinished round that no draft change lets this run finish.
///
/// Only a `changed_pending_assessment` rejection asks for it. The journal is
/// read without changing files.
#[review_kit_observation::operations]
#[helper]
fn unfinished_round(
    config: &Config,
    run: &LoadedRun,
    diagnostics: &[Diagnostic],
) -> Result<Option<u64>, CliError> {
    if !diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == CHANGED_PENDING_ASSESSMENT)
    {
        return Ok(None);
    }
    let reading = journal::read(&config.run_store(&run.manifest.subject, &run.manifest.reviewer));
    let snapshot = run.context.snapshot.clone().into_snapshot()?;
    Ok(journal::unfinished_without_journal(
        &reading,
        &ManagedHistory::from(&snapshot),
    ))
}

/// Write previews of a valid assessment; remove them otherwise so stale previews never pass for current.
#[review_kit_observation::operations]
#[helper]
pub fn previews(
    writer: &RunWriter<'_>,
    plan: Option<&Plan>,
) -> Result<Vec<&'static str>, CliError> {
    let Some(plan) = plan else {
        for file in [RenderedFile::Summary, RenderedFile::TerminalSummary] {
            writer
                .remove_rendered(file)
                .map_err(|error| CliError::storage("remove stale preview", error))?;
        }
        return Ok(Vec::new());
    };
    writer
        .replace(
            WorkingFile::Rendered(RenderedFile::Summary),
            plan.summary_body.as_bytes(),
        )
        .map_err(|error| CliError::storage("save summary preview", error))?;
    writer
        .replace(
            WorkingFile::Rendered(RenderedFile::TerminalSummary),
            format!("> {PREVIEW_NOTE}\n\n{}", plan.terminal_summary_preview).as_bytes(),
        )
        .map_err(|error| CliError::storage("save terminal preview", error))?;
    Ok(vec![
        "rendered/review-summary.md",
        "rendered/terminal-review-summary.md",
    ])
}

/// Record the latest check or acceptance result; it is diagnostic output, not authority.
#[review_kit_observation::operations]
#[helper]
pub fn record_result(writer: &RunWriter<'_>, output: &Value) -> Result<(), CliError> {
    writer
        .replace_json(WorkingFile::CheckResult, output)
        .map_err(|error| CliError::storage("save check result", error))
}

/// Load the configured renderer, its identity and the review workflow for assessment validation.
#[review_kit_observation::operations]
#[helper]
pub fn renderer(config: &Config) -> Result<Rendering, CliError> {
    let renderer = rendering::load(config)?;
    let identity = rendering::identity(renderer.identity());
    Ok(Rendering {
        renderer,
        identity,
        review_workflow: config.review_workflow.clone(),
    })
}

/// Summary counts of a valid assessment for command output.
#[review_kit_observation::operations]
#[helper]
pub fn plan_summary(plan: &Plan) -> Value {
    json!({
        "round": plan.round.number,
        "actions": plan.actions.len(),
        "unchanged": plan.unchanged,
    })
}
