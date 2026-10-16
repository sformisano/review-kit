use review_kit_filesystem_adapter::{RuntimeStore, WorkingFile};
use review_kit_json_adapter::checkpoints::ReviewRunDocument;
use serde_json::{Value, json};

use super::{
    chain::assessment_state,
    environment::{self, run_directory},
    loading::lifecycle_label,
};
use crate::{
    commands::{CommandOptions, CommandOutput},
    error::CliError,
};

/// List stored runs without changing any file.
#[review_kit_observation::operations]
#[operation(name = "cli.run.list", input = crate::logging::command_input("cli.run.list", options), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions) -> Result<CommandOutput, CliError> {
    let store = environment::locate()?;
    let workspace = environment::workspace(&store)?;
    let selected = workspace
        .as_ref()
        .map(|workspace| &workspace.selected_run_id);
    let listing = store
        .list_runs()
        .map_err(|error| CliError::storage("list review runs", error))?;
    let runs = listing
        .runs
        .iter()
        .map(|run_id| {
            let mut entry = describe(&store, run_id);
            entry["selected"] = json!(selected == Some(run_id));
            entry
        })
        .collect::<Vec<_>>();
    let staging = store
        .staging_leftovers()
        .map_err(|error| CliError::storage("list incomplete runs", error))?;
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "run list",
        "status": "listed",
        "selected_run_id": selected.map(|run_id| run_id.as_str()),
        "runs": runs,
        "incomplete_staging": staging.runs.iter().map(|run_id| run_id.as_str()).collect::<Vec<_>>(),
    })))
}

/// One run's identity, lifecycle and assessment readiness, or the reason it cannot be read.
#[review_kit_observation::operations]
#[helper]
fn describe(store: &RuntimeStore, run_id: &review_kit_application::review_run::RunId) -> Value {
    let loaded = store.open_run(run_id).and_then(|dir| {
        dir.read_json::<ReviewRunDocument>(WorkingFile::Manifest.into())
            .map(|manifest| (dir, manifest))
    });
    let (dir, manifest) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            return json!({
                "run_id": run_id.as_str(),
                "run_directory": run_directory(run_id),
                "error": CliError::storage("read run manifest", error).document()["diagnostics"][0].clone(),
            });
        }
    };
    let (status, completion, reason) = lifecycle_label(&manifest.lifecycle);
    let assessment = assessment_state(&dir, manifest.review_assessment.as_ref())
        .map(|state| json!(state.label()))
        .unwrap_or(Value::Null);
    json!({
        "run_id": run_id.as_str(),
        "run_directory": run_directory(run_id),
        "identity_matches_directory": &manifest.run_id == run_id,
        "subject": review_kit_json_adapter::domain::SubjectData::from(&manifest.subject),
        "reviewer": manifest.reviewer.as_str(),
        "head": manifest.revision.head(),
        "created_at": manifest.created_at.as_str(),
        "status": status,
        "completion": completion,
        "reason": reason,
        "publication_attempted": manifest.publication_attempted,
        "assessment": assessment,
    })
}
