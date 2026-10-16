//! Process logging is configured before argument parsing and never supplies review authority.

mod file;
mod stderr;

use std::{io, path::PathBuf};

use review_kit_observation::{Fields, Observer};
use serde::Deserialize;

use crate::app_home::{AppHome, decode_config, read_config};
use file::RecordFile;
use stderr::RecordStderr;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Destination {
    #[default]
    File,
    Stderr,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    destination: Destination,
    directory: PathBuf,
}

#[review_kit_observation::operations]
impl Default for Settings {
    #[helper]
    fn default() -> Self {
        Self {
            destination: Destination::File,
            directory: PathBuf::from("logs"),
        }
    }
}

enum Configuration {
    Default,
    Configured,
    Invalid,
    Unavailable,
}

/// Only fixed categories cross the observation boundary from bootstrap.
pub struct Status {
    configuration: Configuration,
    destination: Destination,
    fallback: bool,
}

#[review_kit_observation::operations]
impl Status {
    #[helper]
    pub fn fields(&self) -> Fields {
        Fields::new()
            .label(
                "configuration",
                match self.configuration {
                    Configuration::Default => "default",
                    Configuration::Configured => "configured",
                    Configuration::Invalid => "invalid",
                    Configuration::Unavailable => "unavailable",
                },
            )
            .label(
                "destination",
                match self.destination {
                    Destination::File => "file",
                    Destination::Stderr => "stderr",
                },
            )
            .flag("fallback", self.fallback)
    }
}

// Bootstrap and the sink cannot observe themselves; cli.run carries their safe status.
#[review_kit_observation::operations]
#[helper]
pub fn configure(home: &AppHome) -> (Observer, Status) {
    let (settings, configuration) = settings(home);
    let mut status = Status {
        fallback: matches!(
            configuration,
            Configuration::Invalid | Configuration::Unavailable
        ),
        configuration,
        destination: settings.destination,
    };
    if matches!(settings.destination, Destination::File) {
        if let Some(directory) = home.resolve_home_path(&settings.directory)
            && let Ok(writer) = RecordFile::open(&directory)
        {
            return (Observer::json(writer), status);
        }
        status.destination = Destination::Stderr;
        status.fallback = true;
    }
    (Observer::json(RecordStderr::default()), status)
}

#[review_kit_observation::operations]
#[helper]
fn settings(home: &AppHome) -> (Settings, Configuration) {
    let Some(path) = home.config_path() else {
        return (Settings::default(), Configuration::Unavailable);
    };
    let text = match read_config(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (Settings::default(), Configuration::Default);
        }
        Err(_) => return (Settings::default(), Configuration::Unavailable),
    };
    let Ok(table) = decode_config::<toml::Table>(&path, &text) else {
        return (Settings::default(), Configuration::Invalid);
    };
    let Some(value) = table.get("logging") else {
        return (Settings::default(), Configuration::Default);
    };
    match value.clone().try_into::<Settings>() {
        Ok(settings) if !settings.directory.as_os_str().is_empty() => {
            (settings, Configuration::Configured)
        }
        _ => (Settings::default(), Configuration::Invalid),
    }
}
