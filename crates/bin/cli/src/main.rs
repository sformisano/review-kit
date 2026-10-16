//! The process boundary parses input, assembles adapters and emits one JSON result.

/// Abort at a named crash point in `fault-injection` builds; expands to nothing otherwise.
macro_rules! fault_point {
    ($point:literal) => {
        #[cfg(feature = "fault-injection")]
        crate::faults::point($point);
    };
}

mod app_home;
mod args;
mod commands;
mod config;
mod effects;
mod error;
#[cfg(feature = "fault-injection")]
mod faults;
mod log_output;
mod logging;
mod progress;

use std::{
    io::{self, Write},
    process::ExitCode,
};

use clap::{Parser, error::ErrorKind};
use serde_json::json;

use args::Args;
use review_kit_observation::{Fields, with_observer};

#[review_kit_observation::operations]
#[helper]
fn main() -> ExitCode {
    // Panic payloads can contain provider or review data; terminal operation logs carry no payload.
    std::panic::set_hook(Box::new(|_| {}));
    let home = app_home::AppHome::from_env();
    let (observer, logging) = log_output::configure(&home);
    let progress = progress::Progress::new();
    let observer = progress.attach(observer);
    ExitCode::from(with_observer(&observer, || run(&home, &logging, progress)))
}

#[review_kit_observation::operations]
#[operation(name = "cli.parse", input = Fields::new(), output = logging::parsed)]
fn parse() -> Result<Args, clap::Error> {
    Args::try_parse()
}

#[review_kit_observation::operations]
#[operation(name = "cli.run", input = Fields::new().object("logging", logging.fields()), output = |exit| logging::exit(*exit))]
fn run(
    home: &app_home::AppHome,
    logging: &log_output::Status,
    mut progress: progress::Progress,
) -> u8 {
    let parsed = parse();
    let args = match parsed {
        Ok(args) => args,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return if error.print().is_ok() { 0 } else { 7 };
        }
        Err(error) => {
            return emit(
                json!({
                    "version": 1,
                    "status": "error",
                    "diagnostics": [{"code": "arguments", "message": error.to_string()}]
                }),
                2,
            );
        }
    };
    progress.start(args.progress);
    let result = commands::execute(args, home);
    progress.finish();
    match result {
        Ok(output) => emit(output.value, output.exit),
        Err(error) => emit(error.document(), error.exit()),
    }
}

#[review_kit_observation::operations]
#[operation(name = "cli.output", input = Fields::new().number("exit", u64::from(status)), output = |exit| logging::exit(*exit))]
fn emit(value: serde_json::Value, status: u8) -> u8 {
    let mut stdout = io::stdout().lock();
    let written = serde_json::to_writer_pretty(&mut stdout, &value)
        .map_err(io::Error::other)
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush());
    if written.is_err() {
        return 7;
    }
    status
}
