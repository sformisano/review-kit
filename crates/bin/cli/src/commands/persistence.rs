use std::path::Path;

use review_kit_application::use_cases::publishing::PublicationProgress;
use review_kit_filesystem_adapter::{FileProgressStore, Workspace};
use review_kit_json_adapter::Diagnostic;
use serde::Serialize;

/// Attempt both final saves even after execution or storage failure.
/// Storage failure changes the process exit without replacing the actual outcome.
#[review_kit_observation::operations]
#[operation(name = "filesystem.save_result", input = review_kit_observation::Fields::new().number("command_exit", u64::from(exit)), output = |result| {
    let fields = review_kit_observation::Fields::new().number("exit", u64::from(result.0)).count("storage_failures", result.1.len());
    review_kit_observation::Completion::new(if result.1.is_empty() { review_kit_observation::Outcome::Success } else { review_kit_observation::Outcome::Failure }, fields)
})]
pub fn save_result(
    workspace: &Workspace,
    store: &FileProgressStore,
    progress: &PublicationProgress,
    document: &impl Serialize,
    exit: u8,
) -> (u8, Vec<Diagnostic>) {
    let mut failures = Vec::new();
    if let Err(error) = store.save(progress) {
        failures.push(Diagnostic::new("progress_storage", &error.to_string()));
    }
    if let Err(error) = workspace.write_json(Path::new("result.json"), document) {
        failures.push(Diagnostic::new("result_storage", &error.to_string()));
    }
    (if failures.is_empty() { exit } else { 7 }, failures)
}

#[cfg(test)]
#[path = "persistence/tests.rs"]
mod tests;
