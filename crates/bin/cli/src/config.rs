use std::{
    env,
    path::{Path, PathBuf},
};

use review_kit_application::publication::Effects;
use review_kit_domain::{ReviewerId, Subject};
use review_kit_filesystem_adapter::FileProgressStore;
use review_kit_gitlab_adapter::{Credential, GitLab, GitLabConfig};
use serde::Deserialize;

use crate::{
    app_home::{AppHome, decode_config, read_config},
    effects::EffectName,
    error::CliError,
};

/// Host-controlled configuration supplies trusted paths, credentials and write authority.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    version: u32,
    #[serde(default = "default_publication_state")]
    pub publication_state: PathBuf,
    pub render_profile: Option<PathBuf>,
    pub review_workflow: Option<String>,
    pub gitlab: Option<GitLabSettings>,
    #[serde(default, rename = "logging")]
    _logging: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitLabSettings {
    pub server: String,
    #[serde(default = "default_token_env")]
    pub token_env: String,
    #[serde(default)]
    pub authority: Vec<EffectName>,
}

#[review_kit_observation::operations]
#[helper]
fn default_token_env() -> String {
    "GITLAB_TOKEN".to_owned()
}

#[review_kit_observation::operations]
#[helper]
fn default_publication_state() -> PathBuf {
    PathBuf::from("publication-state")
}

#[review_kit_observation::operations]
impl Config {
    #[operation(name = "config.load", input = review_kit_observation::Fields::new().flag("explicit_path", path.is_some()), output = |result| crate::logging::result(result, |config| review_kit_observation::Fields::new().flag("gitlab_configured", config.gitlab.is_some()).flag("custom_profile", config.render_profile.is_some())))]
    pub fn load(path: Option<&Path>, home: &AppHome) -> Result<Self, CliError> {
        let path = home.review_config(path)?;
        let content = read_config(&path).map_err(|source| CliError::Read {
            path: path.clone(),
            source,
        })?;
        let mut config: Self = decode_config(&path, &content)?;
        if config.version != 1 {
            return Err(CliError::ConfigVersion(config.version));
        }
        if let Some(name) = &config.review_workflow
            && (name.trim().is_empty()
                || name.len() > 160
                || !name.chars().all(|character| {
                    character.is_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.')
                }))
        {
            return Err(CliError::Input {
                code: "review_workflow",
                message: "review_workflow must be a public name of 1–160 bytes using letters, numbers, spaces, dots, underscores or hyphens".into(),
            });
        }
        if config.publication_state.is_relative() {
            config.publication_state = path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&config.publication_state);
        }
        if let Some(profile) = &mut config.render_profile
            && profile.is_relative()
        {
            *profile = path.parent().unwrap_or(Path::new(".")).join(&*profile);
        }
        Ok(config)
    }

    #[operation(name = "config.connect", input = review_kit_observation::Fields::new(), output = |result| crate::logging::result(result, |_| review_kit_observation::Fields::new()))]
    pub fn connect(&self, mr: &str) -> Result<GitLab, CliError> {
        let settings = self.gitlab.as_ref().ok_or(CliError::MissingGitLab)?;
        let token = env::var(&settings.token_env)
            .map_err(|_| CliError::MissingToken(settings.token_env.clone()))?;
        let credential = Credential::private_token(&token).map_err(CliError::GitLab)?;
        let config = GitLabConfig::new(&settings.server, credential).map_err(CliError::GitLab)?;
        GitLab::connect(config, mr).map_err(CliError::GitLab)
    }

    #[helper]
    pub fn authority(&self) -> Effects {
        Effects::new(
            self.gitlab
                .iter()
                .flat_map(|settings| settings.authority.iter())
                .copied()
                .map(Into::into),
        )
    }

    /// The subject/reviewer journal shared by every review run, `publication-progress.json`.
    #[helper]
    pub fn run_store(&self, subject: &Subject, reviewer: &ReviewerId) -> FileProgressStore {
        FileProgressStore::new(&self.publication_state, subject, reviewer)
    }
}
