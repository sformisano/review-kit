use review_kit_application::{review_run::ContentRef, use_cases::checkpoints::Successor};
use review_kit_filesystem_adapter::{RunWriter, WorkingFile};
use review_kit_json_adapter::{
    Diagnostic,
    checkpoints::{AcceptanceResultDocument, AcceptanceStatus},
};
use serde_json::json;

use super::{
    missing_draft, plan_summary, previews, record_result, rejected_next, renderer, validate,
};
use crate::{
    commands::{
        CommandOptions, CommandOutput, rendering,
        runs::{
            chain::{accepted_bytes, draft, reference},
            lifecycle::begin,
            loading::LoadedRun,
        },
    },
    config::Config,
    error::CliError,
};

/// Freeze exact validated bytes before advancing the manifest reference.
/// An identical retry verifies the existing snapshot and preserves its reference.
#[review_kit_observation::operations]
#[operation(name = "cli.accept", input = crate::logging::command_input("cli.accept", options), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions) -> Result<CommandOutput, CliError> {
    let config = options.load()?;
    let mut rendering = renderer(&config)?;
    let mut mutation = begin(options)?;
    let run_id = mutation.run.run_id().clone();
    let writer = mutation
        .lock
        .run(&run_id)
        .map_err(|error| CliError::storage("open review run", error))?;
    let run = &mut mutation.run;
    let Some(bytes) = draft(&run.dir)? else {
        previews(&writer, None)?;
        return rejected(&writer, run, missing_draft(), &config);
    };
    let submitted = reference(&bytes)?;
    let current = run.manifest.review_assessment.clone();
    let status = if current.as_ref() == Some(&submitted) {
        accepted_bytes(&run.dir, &submitted)?;
        AcceptanceStatus::Existing
    } else {
        AcceptanceStatus::Stored
    };
    let successor = match status {
        AcceptanceStatus::Existing => Successor::Retained,
        AcceptanceStatus::Stored => Successor::Current(current.as_ref()),
    };
    let checked = validate(run, &bytes, successor, &mut rendering)?;
    let plan = match checked {
        Ok(plan) => plan,
        Err(diagnostics) => {
            previews(&writer, None)?;
            return rejected(&writer, run, diagnostics, &config);
        }
    };
    if status == AcceptanceStatus::Stored {
        fault_point!("accept.before_install");
        writer
            .install_accepted(&bytes)
            .map_err(|error| CliError::storage("store accepted snapshot", error))?;
        fault_point!("accept.after_install");
        run.manifest.review_assessment = Some(submitted.clone());
        writer
            .replace_json(WorkingFile::Manifest, &run.manifest)
            .map_err(|error| CliError::storage("advance run manifest", error))?;
        fault_point!("accept.after_manifest");
    }
    let receipt = AcceptanceResultDocument {
        run_id: run_id.clone(),
        status,
        reference: submitted,
    };
    let mut output = serde_json::to_value(&receipt)
        .map_err(|error| CliError::adapter("encode acceptance result", "encoding", 7, error))?;
    output["command"] = json!("accept");
    output["diagnostics"] = json!([]);
    output["next"] = json!(
        "Run rkc publish --effects review --plan, then publish with the permitted effects; or run rkc run finish for a local-only review."
    );
    output["previews"] = json!(previews(&writer, Some(&plan))?);
    output["render_profile"] = rendering.identity.clone();
    output["plan"] = plan_summary(&plan);
    rendering::save_run_preview(&writer, &rendering.identity)?;
    record_result(&writer, &output)?;
    Ok(CommandOutput::success(output))
}

#[review_kit_observation::operations]
#[helper]
fn rejected(
    writer: &RunWriter<'_>,
    run: &LoadedRun,
    diagnostics: Vec<Diagnostic>,
    config: &Config,
) -> Result<CommandOutput, CliError> {
    let next = rejected_next(config, run, &diagnostics, "run rkc accept again")?;
    let output = json!({
        "version": 1,
        "command": "accept",
        "run_id": run.run_id().as_str(),
        "status": "rejected",
        "diagnostics": diagnostics,
        "accepted": run.manifest.review_assessment.as_ref().map(ContentRef::as_str),
        "next": next,
    });
    record_result(writer, &output)?;
    Ok(CommandOutput {
        value: output,
        exit: 2,
    })
}
