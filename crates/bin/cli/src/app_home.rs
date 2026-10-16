use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

use crate::error::CliError;

const CONFIG_LIMIT: u64 = 1024 * 1024;

pub struct AppHome {
    root: Option<PathBuf>,
    legacy_config: Option<PathBuf>,
}

#[review_kit_observation::operations]
impl AppHome {
    #[helper]
    pub fn from_env() -> Self {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|home| home.is_absolute());
        let legacy_root = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        Self {
            root: home.map(|home| home.join(".review-kit")),
            legacy_config: legacy_root.map(|root| root.join("review-kit/config.json")),
        }
    }

    #[helper]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    #[helper]
    pub fn config_path(&self) -> Option<PathBuf> {
        self.root().map(|root| root.join("config.toml"))
    }

    #[helper]
    pub fn resolve_home_path(&self, path: &Path) -> Option<PathBuf> {
        if path.is_absolute() {
            return Some(path.to_owned());
        }
        let root = self.root.as_ref()?;
        if let Ok(relative) = path.strip_prefix("~") {
            return root.parent().map(|home| home.join(relative));
        }
        Some(root.join(path))
    }

    #[helper]
    pub fn review_config(&self, explicit: Option<&Path>) -> Result<PathBuf, CliError> {
        if let Some(path) = explicit {
            return Ok(path.to_owned());
        }
        let home = self.config_path();
        let home_present = present(home.as_deref())?;
        let legacy_present = present(self.legacy_config.as_deref())?;
        match (home_present, legacy_present) {
            (true, true) => Err(CliError::ConfigConflict),
            (false, true) => self.legacy_config.clone().ok_or(CliError::MissingConfig),
            _ => home
                .or_else(|| self.legacy_config.clone())
                .ok_or(CliError::MissingConfig),
        }
    }
}

#[review_kit_observation::operations]
#[helper]
fn present(path: Option<&Path>) -> Result<bool, CliError> {
    let Some(path) = path else {
        return Ok(false);
    };
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CliError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn read_config(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        // Startup must reject a FIFO without waiting for another process to open it.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > CONFIG_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration must be a regular file no larger than 1 MiB",
        ));
    }
    let mut content = Vec::new();
    file.take(CONFIG_LIMIT + 1).read_to_end(&mut content)?;
    if content.len() as u64 > CONFIG_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration exceeds 1 MiB",
        ));
    }
    Ok(content)
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn decode_config<T: DeserializeOwned>(
    path: &Path,
    content: &[u8],
) -> Result<T, CliError> {
    if path
        .extension()
        .is_some_and(|extension| extension == "toml")
    {
        let content = std::str::from_utf8(content).map_err(CliError::ConfigEncoding)?;
        toml::from_str(content).map_err(CliError::ConfigToml)
    } else {
        serde_json::from_slice(content).map_err(CliError::ConfigJson)
    }
}
