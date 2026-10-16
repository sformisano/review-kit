use review_kit_application::{publication::Effects, review_run::RunCompletion};
use review_kit_filesystem_adapter::{WorkingFile, WorkspaceLock};
use review_kit_json_adapter::checkpoints::{ReviewRunDocument, RunLifecycle};
use serde_json::{Value, json};

use super::{
    chain::{Drafts, accepted_review},
    environment,
    journal::{reconcile_local, settlement_value},
    loading::{LoadedRun, StoredRun, load_manifest, load_run, require_open},
};
use crate::{
    commands::{CommandOptions, CommandOutput},
    config::Config,
    error::CliError,
};

/// The selected run under the workspace lock, reconciled with the journal.
pub struct Mutation {
    pub lock: WorkspaceLock,
    pub run: LoadedRun,
    pub journal: &'static str,
    /// Trusted configuration, when it loads; local mutations do not require it.
    pub config: Option<Config>,
}

/// Lock the workspace, pin the selected run, load it and reconcile it with the journal.
///
/// Every accepted-input mutation and `run finish` start here: the retained files
/// must be intact. The run must stay open after reconciliation.
#[review_kit_observation::operations]
#[helper]
pub fn begin(options: &CommandOptions) -> Result<Mutation, CliError> {
    let store = environment::locate()?;
    let lock = environment::lock(&store)?;
    let (workspace, run_id) = environment::selected(lock.store())?;
    let mut run = load_run(lock.store(), Some(&workspace.repository), &run_id)?;
    let config = options.load().ok();
    let journal = reconcile_open(&lock, config.as_ref(), &mut run.manifest)?;
    Ok(Mutation {
        lock,
        run,
        journal,
        config,
    })
}

/// Reconcile `manifest` with the journal, repairing a settled open manifest, and
/// require the run to stay open.
#[review_kit_observation::operations]
#[helper]
fn reconcile_open(
    lock: &WorkspaceLock,
    config: Option<&Config>,
    manifest: &mut ReviewRunDocument,
) -> Result<&'static str, CliError> {
    let writer = lock
        .run(&manifest.run_id)
        .map_err(|error| CliError::storage("open review run", error))?;
    let reconciled = reconcile_local(config, &writer, manifest)?;
    drop(writer);
    if let Err(error) = require_open(manifest) {
        return Err(match &reconciled.repaired {
            Some(entry) => {
                error.with_details(json!({"repaired_from_journal": settlement_value(entry)}))
            }
            None => error,
        });
    }
    Ok(reconciled.journal)
}

/// Complete a clean accepted run locally without provider calls.
#[review_kit_observation::operations]
#[operation(name = "cli.run.finish", input = crate::logging::command_input("cli.run.finish", options), output = crate::logging::command_result)]
pub fn finish(options: &CommandOptions) -> Result<CommandOutput, CliError> {
    let mut mutation = begin(options)?;
    let review_workflow = mutation
        .config
        .as_ref()
        .and_then(|config| config.review_workflow.as_deref());
    accepted_review(
        &mutation.run,
        Effects::none(),
        Drafts::Clean,
        review_workflow,
    )?;
    mutation.run.manifest.lifecycle = RunLifecycle::Completed {
        completion: RunCompletion::LocalOnly,
        reason: None,
    };
    save(&mutation.lock, &mutation.run.manifest)?;
    Ok(CommandOutput::success(lifecycle_output(
        "run finish",
        &mutation.run.manifest,
        mutation.journal,
    )))
}

/// Stop unpublished work; no accepted assessment or provider connection is required.
///
/// Only the manifest's identity and the journal are consulted: retained files may
/// be missing or damaged.
#[review_kit_observation::operations]
#[operation(name = "cli.run.stop", input = crate::logging::command_input("cli.run.stop", options), output = crate::logging::command_result)]
pub fn stop(options: &CommandOptions, reason: &str) -> Result<CommandOutput, CliError> {
    if reason.trim().is_empty() {
        return Err(CliError::Input {
            code: "blank_reason",
            message: "stop reason must not be blank".into(),
        });
    }
    let store = environment::locate()?;
    let lock = environment::lock(&store)?;
    let (workspace, run_id) = environment::selected(lock.store())?;
    let StoredRun { mut manifest, .. } =
        load_manifest(lock.store(), Some(&workspace.repository), &run_id)?;
    let config = options.load().ok();
    let journal = reconcile_open(&lock, config.as_ref(), &mut manifest)?;
    manifest.lifecycle = RunLifecycle::Stopped {
        reason: reason.to_owned(),
    };
    save(&lock, &manifest)?;
    Ok(CommandOutput::success(lifecycle_output(
        "run stop", &manifest, journal,
    )))
}

#[review_kit_observation::operations]
#[helper]
fn save(lock: &WorkspaceLock, manifest: &ReviewRunDocument) -> Result<(), CliError> {
    lock.run(&manifest.run_id)
        .and_then(|writer| writer.replace_json(WorkingFile::Manifest, manifest))
        .map_err(|error| CliError::storage("save run manifest", error))
}

#[review_kit_observation::operations]
#[helper]
fn lifecycle_output(command: &str, manifest: &ReviewRunDocument, journal: &str) -> Value {
    let (status, completion, reason) = super::loading::lifecycle_label(&manifest.lifecycle);
    json!({
        "version": 1,
        "command": command,
        "run_id": manifest.run_id.as_str(),
        "status": status,
        "completion": completion,
        "reason": reason,
        "journal": journal,
    })
}
