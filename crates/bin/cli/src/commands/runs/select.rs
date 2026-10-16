use review_kit_application::{review_run::RunId, use_cases::publishing::RunPublication};
use review_kit_json_adapter::checkpoints::{
    RepositoryBinding, ReviewRunDocument, ReviewWorkspaceDocument,
};
use serde_json::json;

use super::{
    chain::{accepted_bytes, assessment_state},
    environment::{self, run_directory},
    journal::{self, JournalReading},
    loading::{LoadedRun, lifecycle_label, load_run},
};
use crate::{
    commands::{CommandOptions, CommandOutput},
    error::CliError,
};

/// Select a stored run after validating it; never recaptures or reopens it.
///
/// The run must belong to the workspace repository, its retained files and
/// accepted snapshots must match their hashes, and an available journal must
/// describe the same accepted inputs for the run's binding or settlement. A
/// changed draft remains inspectable; open, completed and stopped runs are all selectable.
#[review_kit_observation::operations]
#[operation(name = "cli.run.select", input = crate::logging::command_input("cli.run.select", options), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions, id: &str) -> Result<CommandOutput, CliError> {
    let run_id = RunId::try_from(id.to_owned())
        .map_err(|error| CliError::adapter("decode run ID", "invalid_run_id", 2, error))?;
    let store = environment::locate()?;
    let lock = environment::lock(&store)?;
    let workspace = environment::workspace(lock.store())?;
    let repository = environment::repository_binding(&lock, workspace.as_ref())?;
    let run = load_run(lock.store(), repository.as_ref(), &run_id)?;
    if let Some(reference) = &run.manifest.review_assessment {
        accepted_bytes(&run.dir, reference)?;
    }
    let state = assessment_state(&run.dir, run.manifest.review_assessment.as_ref())?;
    let reading = match options.load() {
        Ok(config) => {
            journal::read(&config.run_store(&run.manifest.subject, &run.manifest.reviewer))
        }
        Err(error) => JournalReading::Unavailable(error),
    };
    check_journal(&run, &reading)?;
    lock.write_workspace(&ReviewWorkspaceDocument {
        repository: repository.unwrap_or_else(|| RepositoryBinding::of(&run.manifest.subject)),
        selected_run_id: run_id.clone(),
    })
    .map_err(|error| CliError::storage("select review run", error))?;
    let (status, completion, reason) = lifecycle_label(&run.manifest.lifecycle);
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "run select",
        "status": "selected",
        "selected_run_id": run_id.as_str(),
        "run_directory": run_directory(&run_id),
        "run_status": status,
        "completion": completion,
        "reason": reason,
        "assessment": state.label(),
        "journal": reading.label(),
        "publication_owner": reading.owner().map(|owner| owner.as_str()),
        "next": "Run rkc status for this review and its next action.",
    })))
}

/// The journal's binding or settlement for this run must name its accepted inputs.
#[review_kit_observation::operations]
#[helper]
fn check_journal(run: &LoadedRun, reading: &JournalReading) -> Result<(), CliError> {
    let manifest: &ReviewRunDocument = &run.manifest;
    match reading.publication(manifest) {
        Some(RunPublication::Settled(entry)) => {
            journal::require_inputs(manifest, &entry.review_assessment, &entry.snapshot_files)
        }
        Some(RunPublication::BoundUnsettled(binding) | RunPublication::BoundUnsent(binding)) => {
            journal::require_inputs(
                manifest,
                &binding.review_assessment,
                &binding.snapshot_files,
            )
        }
        Some(RunPublication::Unbound) | None => Ok(()),
    }
}
