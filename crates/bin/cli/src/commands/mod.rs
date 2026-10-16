use std::path::PathBuf;

use serde_json::Value;

use crate::{
    app_home::AppHome,
    args::{Args, Command, RenderProfileCommand, RunCommand},
    config::Config,
    effects::EffectSelection,
    error::CliError,
};

mod history;
mod persistence;
pub(crate) mod rendering;
mod runs;
mod shared;
mod status;

pub struct CommandOutput {
    pub value: Value,
    pub exit: u8,
}

#[review_kit_observation::operations]
impl CommandOutput {
    #[helper]
    pub fn success(value: Value) -> Self {
        Self { value, exit: 0 }
    }
}

pub struct CommandOptions<'home> {
    pub config: Option<PathBuf>,
    pub render_profile: Option<PathBuf>,
    home: &'home AppHome,
}

#[review_kit_observation::operations]
impl CommandOptions<'_> {
    #[helper]
    fn load(&self) -> Result<Config, CliError> {
        let mut config = Config::load(self.config.as_deref(), self.home)?;
        if let Some(profile) = &self.render_profile {
            config.render_profile = Some(profile.clone());
        }
        Ok(config)
    }
}

#[review_kit_observation::operations]
#[helper]
pub fn execute(args: Args, home: &AppHome) -> Result<CommandOutput, CliError> {
    let options = CommandOptions {
        config: args.config,
        render_profile: args.render_profile,
        home,
    };
    match args.command {
        Command::RenderProfile {
            command: RenderProfileCommand::Export { dir },
        } => rendering::export(&options, &dir),
        Command::Run { command } => match command {
            RunCommand::Start { mr, new } => runs::start::execute(&options, mr.as_deref(), new),
            RunCommand::List => runs::listing::execute(&options),
            RunCommand::Select { id } => runs::select::execute(&options, &id),
            RunCommand::Finish => runs::lifecycle::finish(&options),
            RunCommand::Stop { reason } => runs::lifecycle::stop(&options, &reason),
        },
        Command::Status => runs::report::execute(&options),
        Command::Check => runs::checkpoints::check::execute(&options),
        Command::Accept => runs::checkpoints::accept::execute(&options),
        Command::Revise { reason, regenerate } => {
            runs::checkpoints::revise::execute(&options, &reason, regenerate)
        }
        Command::Publish {
            effects,
            plan,
            retry_uncertain,
            abandon,
            reason,
            ..
        } => {
            let request = match (abandon, reason) {
                (true, Some(reason)) => runs::publication::Request::Abandon(
                    selected(effects, "update_summary")?,
                    reason,
                ),
                _ => {
                    let effects = selected(effects, "none")?;
                    if plan || effects.is_empty() {
                        runs::publication::Request::Plan(effects)
                    } else {
                        runs::publication::Request::Publish(effects, retry_uncertain)
                    }
                }
            };
            runs::publication::execute(&options, request)
        }
    }
}

/// The requested effects, or the mode's default.
#[review_kit_observation::operations]
#[helper]
fn selected(
    effects: Option<EffectSelection>,
    default: &str,
) -> Result<review_kit_application::publication::Effects, CliError> {
    match effects {
        Some(selection) => Ok(selection.0),
        None => default
            .parse::<EffectSelection>()
            .map(|selection| selection.0)
            .map_err(|error| CliError::adapter("select effects", "arguments", 2, error)),
    }
}
