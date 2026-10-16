use std::{error::Error, io, path::PathBuf};

use review_kit_filesystem_adapter::{FileError, RunStorageError};
use review_kit_gitlab_adapter::GitLabError;
use review_kit_json_adapter::{Diagnostic, JsonError};
use serde_json::{Value, json};

/// Boundary errors preserve causes while giving commands a stable process status.
#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum CliError {
    /// specify --config or set HOME or XDG_CONFIG_HOME for review configuration
    MissingConfig,
    /// cannot read {path:?}: {source}
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// invalid configuration JSON: {0}
    ConfigJson(#[source] serde_json::Error),
    /// invalid configuration TOML: {0}
    ConfigToml(#[source] toml::de::Error),
    /// configuration TOML is not UTF-8: {0}
    ConfigEncoding(#[source] std::str::Utf8Error),
    /// both home TOML and default JSON configurations exist; select --config explicitly and retain the publication_state directory containing existing progress
    ConfigConflict,
    /// unsupported configuration version {0}; expected 1
    ConfigVersion(u32),
    /// configuration requires gitlab settings for this command
    MissingGitLab,
    /// set credential environment variable {0}
    MissingToken(String),
    /// {0}
    GitLab(#[source] GitLabError),
    /// {operation}: {source}
    Adapter {
        operation: &'static str,
        code: &'static str,
        exit: u8,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    /// {message}
    Input { code: &'static str, message: String },
    /// assessment has incomplete or invalid inputs
    Assessment(Vec<Diagnostic>),
    /// assessment reviewer differs from the authenticated GitLab account
    WrongReviewer,
    /// {message}
    State {
        code: &'static str,
        exit: u8,
        message: String,
        /// Extra diagnostic fields, such as run IDs; never credentials or bodies.
        details: Value,
    },
}

impl CliError {
    pub fn adapter(
        operation: &'static str,
        code: &'static str,
        exit: u8,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Adapter {
            operation,
            code,
            exit,
            source: Box::new(source),
        }
    }

    pub fn exit(&self) -> u8 {
        match self {
            Self::Read { .. } => 7,
            Self::GitLab(error) => gitlab_read_exit(error),
            Self::Adapter { exit, .. } | Self::State { exit, .. } => *exit,
            Self::WrongReviewer => 4,
            _ => 2,
        }
    }

    pub fn document(&self) -> Value {
        if let Self::Assessment(diagnostics) = self {
            let mut document =
                json!({"version": 1, "status": "invalid", "diagnostics": diagnostics});
            if let Some(next) = crate::commands::rendering::restore_guidance(diagnostics) {
                document["next"] = json!(next);
            }
            return document;
        }
        let code = match self {
            Self::MissingConfig
            | Self::ConfigJson(_)
            | Self::ConfigToml(_)
            | Self::ConfigEncoding(_)
            | Self::ConfigConflict
            | Self::ConfigVersion(_)
            | Self::MissingGitLab
            | Self::MissingToken(_) => "configuration",
            Self::Read { .. } => "filesystem",
            Self::GitLab(GitLabError::Config) => "configuration",
            Self::GitLab(_) => "gitlab",
            Self::Adapter { code, .. } | Self::Input { code, .. } | Self::State { code, .. } => {
                code
            }
            Self::Assessment(_) => "assessment",
            Self::WrongReviewer => "wrong_reviewer",
        };
        let causes = if let Self::Adapter { source, .. } = self {
            let mut causes = Vec::new();
            let mut error = source.source();
            while let Some(cause) = error {
                causes.push(cause.to_string());
                error = cause.source();
            }
            causes
        } else {
            Vec::new()
        };
        let mut diagnostic = json!({"code": code, "message": self.to_string(), "causes": causes});
        if let Self::State {
            details: Value::Object(details),
            ..
        } = self
            && let Value::Object(fields) = &mut diagnostic
        {
            for (key, value) in details {
                fields.insert(key.clone(), value.clone());
            }
        }
        json!({"version": 1, "status": "error", "diagnostics": [diagnostic]})
    }

    /// A run-lifecycle or journal state that prevents the command, with a stable code.
    pub fn state(code: &'static str, exit: u8, message: impl Into<String>) -> Self {
        Self::State {
            code,
            exit,
            message: message.into(),
            details: Value::Null,
        }
    }

    /// Attach diagnostic fields to a state error.
    pub fn with_details(self, details: Value) -> Self {
        match self {
            Self::State {
                code,
                exit,
                message,
                ..
            } => Self::State {
                code,
                exit,
                message,
                details,
            },
            other => other,
        }
    }

    /// Add one diagnostic field to a state error, keeping its other fields.
    pub fn with_detail(self, key: &str, value: Value) -> Self {
        match self {
            Self::State {
                code,
                exit,
                message,
                mut details,
            } => {
                if !details.is_object() {
                    details = json!({});
                }
                details[key] = value;
                Self::State {
                    code,
                    exit,
                    message,
                    details,
                }
            }
            other => other,
        }
    }

    /// Map runtime storage failures to their stable codes and exits.
    pub fn storage(operation: &'static str, error: RunStorageError) -> Self {
        let (code, exit) = match &error {
            RunStorageError::NotInWorktree { .. } => ("not_in_git_worktree", 2),
            RunStorageError::Tracked { .. } => ("runtime_tracked", 2),
            RunStorageError::Unrecognized { .. } => ("runtime_unrecognized", 2),
            RunStorageError::Redirected { .. } | RunStorageError::UnexpectedKind { .. } => {
                ("runtime_redirected", 2)
            }
            RunStorageError::Uninitialized => ("no_selected_run", 2),
            RunStorageError::WorkspaceBusy => ("workspace_busy", 5),
            RunStorageError::RunMissing { .. } => ("run_missing", 2),
            RunStorageError::MissingFile { .. } => ("run_file_missing", 2),
            RunStorageError::ContentMissing { .. } => ("accepted_snapshot_missing", 2),
            RunStorageError::ContentCorrupt { .. } | RunStorageError::SnapshotMismatch { .. } => {
                ("hash_mismatch", 2)
            }
            RunStorageError::Json { .. } => ("invalid_run_document", 2),
            RunStorageError::GitUnavailable(_)
            | RunStorageError::GitListing { .. }
            | RunStorageError::RunExists { .. }
            | RunStorageError::StagingExists { .. }
            | RunStorageError::IncompleteStaging { .. }
            | RunStorageError::FileTooLarge { .. }
            | RunStorageError::Io { .. }
            | RunStorageError::Unsynchronized { .. }
            | RunStorageError::Digest(_) => ("filesystem", 7),
        };
        Self::adapter(operation, code, exit, error)
    }

    /// Map run-bound journal failures to their stable codes and exits.
    pub fn journal(operation: &'static str, error: FileError) -> Self {
        let (code, exit) = match &error {
            FileError::PublicationBusy => ("publication_busy", 5),
            FileError::MissingProgress => ("journal_missing", 5),
            FileError::RunJournalExists => ("run_journal_exists", 5),
            FileError::JournalRegression => ("settlement_conflict", 5),
            FileError::Json(_) | FileError::EvidenceBinding | FileError::InvalidPath => {
                ("journal_unavailable", 5)
            }
            FileError::Io { .. } | FileError::FileTooLarge | FileError::RunStorage(_) => {
                ("journal_unavailable", 5)
            }
        };
        Self::adapter(operation, code, exit, error)
    }

    /// Map execution-evidence storage failures; refused run files keep their storage codes.
    pub fn evidence(operation: &'static str, error: FileError) -> Self {
        match error {
            FileError::RunStorage(error) => Self::storage(operation, error),
            error => Self::adapter(operation, "evidence", 7, error),
        }
    }
}

impl From<JsonError> for CliError {
    fn from(error: JsonError) -> Self {
        Self::Assessment(error.diagnostics())
    }
}

pub fn gitlab_read_exit(error: &(dyn Error + 'static)) -> u8 {
    match gitlab_cause(error) {
        Some(GitLabError::Config) => 2,
        Some(GitLabError::RevisionChanged) => 3,
        Some(GitLabError::Conflict | GitLabError::FindingHistory { .. }) => 5,
        _ => 7,
    }
}

pub fn source_check_exit(error: &(dyn Error + 'static)) -> u8 {
    match gitlab_cause(error) {
        Some(GitLabError::UnsupportedSource | GitLabError::UnsupportedPosition) => 2,
        _ => gitlab_read_exit(error),
    }
}

pub(crate) fn gitlab_cause<'a>(mut error: &'a (dyn Error + 'static)) -> Option<&'a GitLabError> {
    loop {
        if let Some(cause) = error.downcast_ref::<GitLabError>() {
            return Some(cause);
        }
        error = error.source()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_journal_and_binding_state_are_conflicts() {
        assert_eq!(
            CliError::journal(
                "restore publication journal",
                FileError::Json(JsonError::ContextBinding)
            )
            .exit(),
            5
        );
        assert_eq!(
            CliError::journal("restore publication journal", FileError::MissingProgress).exit(),
            5
        );
        let busy = CliError::journal("restore publication journal", FileError::PublicationBusy);
        assert_eq!(busy.exit(), 5);
        assert_eq!(
            busy.document()["diagnostics"][0]["code"],
            "publication_busy"
        );
        let error = FileError::Io {
            path: PathBuf::from("progress.json"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "cannot read progress"),
        };
        assert_eq!(
            CliError::journal("restore publication journal", error).exit(),
            5
        );
    }

    #[test]
    fn source_capture_revision_drift_is_a_stale_review() {
        assert_eq!(CliError::GitLab(GitLabError::RevisionChanged).exit(), 3);
        assert_eq!(CliError::GitLab(GitLabError::Transport).exit(), 7);
    }
}
