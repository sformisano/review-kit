use review_kit_observation::{Completion, ErrorInfo, Fields, Outcome};

use crate::{
    args::{Args, Command, RunCommand},
    error::CliError,
};

pub fn parsed(result: &Result<Args, clap::Error>) -> Completion {
    use clap::error::ErrorKind;
    match result {
        Ok(args) => Completion::success(arguments(args)),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Completion::success(Fields::new().label("request", "help_or_version"))
        }
        Err(_) => Completion::failure(ErrorInfo::new("arguments", "invalid")),
    }
}

pub fn command_result(result: &Result<crate::commands::CommandOutput, CliError>) -> Completion {
    match result {
        Ok(output) => exit(output.exit),
        Err(error_value) => exit(error_value.exit()).with_error(error(error_value)),
    }
}

pub fn command_input(name: &'static str, options: &crate::commands::CommandOptions) -> Fields {
    Fields::new()
        .label("command", name)
        .flag("explicit_config", options.config.is_some())
        .flag("explicit_profile", options.render_profile.is_some())
}

pub fn operation(command: &Command) -> &'static str {
    match command {
        Command::RenderProfile { .. } => "cli.render_profile.export",
        Command::Run { command } => match command {
            RunCommand::Start { .. } => "cli.run.start",
            RunCommand::List => "cli.run.list",
            RunCommand::Select { .. } => "cli.run.select",
            RunCommand::Finish => "cli.run.finish",
            RunCommand::Stop { .. } => "cli.run.stop",
        },
        Command::Status => "cli.status",
        Command::Check => "cli.check",
        Command::Accept => "cli.accept",
        Command::Revise { .. } => "cli.revise",
        Command::Publish { .. } => "cli.publish.run",
    }
}

pub fn arguments(args: &Args) -> Fields {
    let fields = Fields::new()
        .label("command", operation(&args.command))
        .flag("explicit_config", args.config.is_some())
        .flag("explicit_profile", args.render_profile.is_some());
    match &args.command {
        Command::Run {
            command: RunCommand::Start { mr, new },
        } => fields.flag("explicit_mr", mr.is_some()).flag("new", *new),
        Command::Revise { regenerate, .. } => fields.flag("regenerate", *regenerate),
        Command::Publish {
            effects,
            plan,
            abandon,
            ..
        } => fields.flag("plan", *plan).flag("abandon", *abandon).flag(
            "effects_requested",
            effects
                .as_ref()
                .is_some_and(|effects| !effects.0.is_empty()),
        ),
        _ => fields,
    }
}

pub fn exit(code: u8) -> Completion {
    Completion::new(
        match code {
            0 => Outcome::Success,
            6 => Outcome::Uncertain,
            2..=5 => Outcome::Rejected,
            _ => Outcome::Failure,
        },
        Fields::new().number("exit", u64::from(code)),
    )
}

pub fn error(error: &CliError) -> ErrorInfo {
    let (variant, cause) = match error {
        CliError::MissingConfig => ("missing_config", None),
        CliError::Read { .. } => ("read", Some("io")),
        CliError::ConfigJson(_) => ("configuration_json", Some("serde_json")),
        CliError::ConfigToml(_) => ("configuration_toml", Some("toml")),
        CliError::ConfigEncoding(_) => ("configuration_encoding", Some("utf8")),
        CliError::ConfigConflict => ("configuration_conflict", None),
        CliError::ConfigVersion(_) => ("configuration_version", None),
        CliError::MissingGitLab => ("missing_gitlab", None),
        CliError::MissingToken(_) => ("missing_credential", None),
        CliError::GitLab(_) => ("gitlab", Some("gitlab")),
        CliError::Adapter { .. } => ("adapter", Some("adapter")),
        CliError::Input { .. } => ("invalid_input", None),
        CliError::Assessment(_) => ("assessment", None),
        CliError::WrongReviewer => ("wrong_reviewer", None),
        CliError::State { .. } => ("state", None),
    };
    let error_info = ErrorInfo::new("cli", variant)
        .with_fields(Fields::new().number("exit", u64::from(error.exit())));
    match cause {
        Some(cause) => error_info.caused_by(ErrorInfo::opaque(cause)),
        None => error_info,
    }
}

pub fn result<T>(result: &Result<T, CliError>, output: impl FnOnce(&T) -> Fields) -> Completion {
    match result {
        Ok(value) => Completion::success(output(value)),
        Err(value) => exit(value.exit()).with_error(error(value)),
    }
}
