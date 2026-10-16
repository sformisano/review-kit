use review_kit_application::use_cases::checkpoints::Successor;
use review_kit_json_adapter::{CheckDocument, checkpoints::RunLifecycle};
use serde_json::json;

use super::{
    missing_draft, plan_summary, previews, record_result, rejected_next, renderer, validate,
};
use crate::{
    commands::{
        CommandOptions, CommandOutput,
        runs::{
            chain::{draft, reference},
            environment,
            loading::load_run,
        },
        shared::PREVIEW_NOTE,
    },
    error::CliError,
};

/// Diagnose the review draft with the acceptance validations; accepted references never change.
///
/// For an open run, assessment checking renders previews of a valid draft and
/// removes previews of an invalid one, and the result is saved as
/// `review-check-result.json`. A terminal run stays inspectable: checking it
/// reports diagnostics and changes no file.
#[review_kit_observation::operations]
#[operation(name = "cli.check", input = crate::logging::command_input("cli.check", options), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions) -> Result<CommandOutput, CliError> {
    let config = options.load()?;
    let mut rendering = renderer(&config)?;
    let store = environment::locate()?;
    let lock = environment::lock(&store)?;
    let (workspace, run_id) = environment::selected(lock.store())?;
    let run = load_run(lock.store(), Some(&workspace.repository), &run_id)?;
    let terminal = run.manifest.lifecycle != RunLifecycle::Open;
    let bytes = draft(&run.dir)?;
    let current = run.manifest.review_assessment.as_ref();
    let checked = match &bytes {
        Some(bytes) => {
            let successor = if current == Some(&reference(bytes)?) {
                Successor::Retained
            } else {
                Successor::Current(current)
            };
            validate(&run, bytes, successor, &mut rendering)?
        }
        None => Err(missing_draft()),
    };
    let plan = checked.as_ref().ok().map(|plan| plan.as_ref());
    let writer = if terminal {
        None
    } else {
        Some(
            lock.run(&run_id)
                .map_err(|error| CliError::storage("open review run", error))?,
        )
    };
    let previews = match &writer {
        Some(writer) => previews(writer, plan)?,
        _ => Vec::new(),
    };
    let valid = checked.is_ok();
    let next = match &checked {
        _ if terminal => {
            "This run is terminal; checking changed no file. Start a fresh review with rkc run start.".to_owned()
        }
        Ok(_) => "Run rkc accept.".to_owned(),
        Err(diagnostics) => rejected_next(&config, &run, diagnostics, "check it again")?,
    };
    let mut output = json!({
        "version": 1,
        "command": "check",
        "run_id": run_id.as_str(),
        "status": if valid { "valid" } else { "invalid" },
        "checked": bytes.as_deref().map(reference).transpose()?.map(|reference| reference.as_str().to_owned()),
        "diagnostics": checked.as_ref().err().cloned().unwrap_or_default(),
        "accepted": run.manifest.review_assessment.as_ref().map(|reference| reference.as_str()),
        "recorded": !terminal,
        "next": next,
    });
    output["render_profile"] = rendering.identity.clone();
    output["previews"] = json!(previews);
    output["preview_note"] = json!(PREVIEW_NOTE);
    if let Some(plan) = plan {
        output["plan"] = plan_summary(plan);
        output["check"] = json!(CheckDocument::from_result(&Ok(plan.clone())));
    }
    if let Some(writer) = &writer {
        record_result(writer, &output)?;
    }
    Ok(CommandOutput {
        value: output,
        exit: if valid { 0 } else { 2 },
    })
}
