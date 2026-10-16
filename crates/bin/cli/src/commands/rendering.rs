use std::path::Path;

use review_kit_application::ports::RenderProfileIdentity;
use review_kit_filesystem_adapter::{RunDir, RunWriter, WorkingFile};
use review_kit_json2md_adapter::Json2MdRenderer;
use serde_json::{Value, json};

use super::{CommandOptions, CommandOutput};
use crate::{config::Config, error::CliError};

/// Code of a refusal of the selected profile, for a changed preview or for
/// unfinished work that another profile rendered.
pub const PROFILE_CHANGED: &str = "render_profile_changed";

/// How unfinished work that another profile rendered continues.
pub const RESTORE_PROFILE: &str = "An unfinished round continues only with the exact profile that rendered it. Restore that profile's manifest and assets and select it with --render-profile PATH or render_profile in configuration until the round finishes.";

/// The guidance for `diagnostics` that refuse the selected profile.
#[review_kit_observation::operations]
#[helper]
pub fn restore_guidance(
    diagnostics: &[review_kit_json_adapter::Diagnostic],
) -> Option<&'static str> {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == PROFILE_CHANGED)
        .then_some(RESTORE_PROFILE)
}

/// A changed profile refused before publication, with the way forward for new
/// work and for unfinished work.
#[review_kit_observation::operations]
#[helper]
fn profile_changed(message: &str, revalidate: &str) -> CliError {
    CliError::state(PROFILE_CHANGED, 2, message)
        .with_details(json!({"next": format!("{revalidate} {RESTORE_PROFILE}")}))
}

#[review_kit_observation::operations]
#[operation(name = "rendering.load", input = review_kit_observation::Fields::new(), output = |result| crate::logging::result(result, |_| review_kit_observation::Fields::new()))]
pub fn load(config: &Config) -> Result<Json2MdRenderer, CliError> {
    match &config.render_profile {
        Some(path) => Json2MdRenderer::from_profile(path),
        None => Json2MdRenderer::new(),
    }
    .map_err(|error| CliError::adapter("load rendering profile", "render_profile", 2, error))
}

#[review_kit_observation::operations]
#[helper]
pub fn identity(profile: &RenderProfileIdentity) -> Value {
    json!({
        "id": profile.id,
        "input_contract": profile.input_contract,
        "fingerprint": profile.fingerprint,
    })
}

#[review_kit_observation::operations]
#[operation(name = "cli.render_profile.export", input = crate::logging::command_input("cli.render_profile.export", options), output = crate::logging::command_result)]
pub fn export(options: &CommandOptions, directory: &Path) -> Result<CommandOutput, CliError> {
    let manifest = Json2MdRenderer::export_default(directory).map_err(|error| {
        CliError::adapter("export rendering profile", "render_profile", 2, error)
    })?;
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "render-profile export",
        "status": "exported",
        "manifest": manifest,
    })))
}

/// Record the profile identity of a run's latest successful assessment acceptance or plan.
#[review_kit_observation::operations]
#[operation(name = "rendering.save_run_preview", input = review_kit_observation::Fields::new(), output = |result| crate::logging::result(result, |_| review_kit_observation::Fields::new()))]
pub fn save_run_preview(writer: &RunWriter<'_>, profile: &Value) -> Result<(), CliError> {
    writer
        .replace_json(
            WorkingFile::RenderProfile,
            &json!({
                "version": 1,
                "render_profile": profile,
            }),
        )
        .map_err(|error| CliError::storage("save preview profile identity", error))
}

/// Publication of a run requires the profile of its latest successful acceptance or plan.
///
/// When `required`, a run without a recorded identity is refused as well: nothing
/// shows which profile produced the reviewed previews, so publication waits for
/// `rkc publish --plan` or acceptance to record one. Recovery after a possible
/// dispatch passes `required: false`; its retained intent and pending round pin
/// the profile.
#[review_kit_observation::operations]
#[operation(name = "rendering.verify_run_preview", input = review_kit_observation::Fields::new().flag("required", required), output = |result| crate::logging::result(result, |_| review_kit_observation::Fields::new()))]
pub fn verify_run_preview(run: &RunDir, profile: &Value, required: bool) -> Result<(), CliError> {
    let previous = run
        .read_optional_bytes(WorkingFile::RenderProfile.into())
        .map_err(|error| CliError::storage("read preview profile identity", error))?
        .map(|bytes| review_kit_json_adapter::decode::<Value>(&bytes))
        .transpose()?;
    let message = match previous {
        Some(previous) if previous.get("render_profile") == Some(profile) => return Ok(()),
        None if !required => return Ok(()),
        Some(_) => {
            "the selected rendering profile differs from the run's latest accepted or planned preview; run rkc publish --plan with this profile before publication. An unfinished round still requires its original profile and retained write intent"
        }
        None => {
            "the run records no preview profile identity in review-render-profile.json; run rkc publish --plan with this profile before publication"
        }
    };
    Err(profile_changed(
        message,
        "To publish new work with the selected profile, run rkc publish --effects SET --plan with it first.",
    ))
}
