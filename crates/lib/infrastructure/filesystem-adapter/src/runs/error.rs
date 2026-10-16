use std::{ffi::OsString, io, path::PathBuf, process::ExitStatus};

use review_kit_application::review_run::{ContentRef, InvalidContentRef, RunId};
use review_kit_json_adapter::JsonError;

use super::files::{ContentArea, RunFile, SnapshotFile};

/// Runtime storage failures.
///
/// [`Self::Unsynchronized`] reports a visible change that may not survive a
/// crash. Other failures leave stored files unchanged, though a temporary file,
/// an empty directory, the lock file or a staging directory may remain; readers
/// ignore them. After any failure, reopen the store and inspect its state
/// before retrying.
#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum RunStorageError {
    /// cannot start git
    GitUnavailable(#[source] io::Error),
    /// {directory:?} is not inside a Git worktree
    NotInWorktree {
        directory: PathBuf,
        status: ExitStatus,
        /// Git's diagnostic output, kept for operators.
        stderr: String,
    },
    /// git cannot list tracked runtime files
    GitListing { status: ExitStatus, stderr: String },
    /// runtime path {path:?} is a symbolic link
    Redirected { path: PathBuf },
    /// runtime path {path:?} has an unexpected file type
    UnexpectedKind { path: PathBuf },
    /// Git tracks runtime files under .review-kit
    Tracked {
        /// Index paths relative to the worktree root.
        paths: Vec<PathBuf>,
    },
    /// .review-kit contains unrecognized entries and no review-workspace.json
    Unrecognized { entries: Vec<OsString> },
    /// this worktree has no .review-kit directory
    Uninitialized,
    /// another command holds the review workspace lock; retry when it finishes
    WorkspaceBusy,
    /// run {run_id} already exists
    RunExists { run_id: RunId },
    /// staging directory for run {run_id} already exists
    StagingExists { run_id: RunId },
    /// staged run {run_id} lacks required files
    IncompleteStaging {
        run_id: RunId,
        missing: Vec<RunFile>,
    },
    /// run {run_id} does not exist
    RunMissing { run_id: RunId },
    /// run file {file} is missing
    MissingFile { file: RunFile, path: PathBuf },
    /// {path:?} exceeds the supported file size
    FileTooLarge { path: PathBuf },
    /// {area:?} content {reference} is missing
    ContentMissing {
        area: ContentArea,
        reference: ContentRef,
    },
    /// stored {area:?} content does not match its digest {reference}
    ContentCorrupt {
        area: ContentArea,
        reference: ContentRef,
        actual: ContentRef,
    },
    /// retained run files differ from their recorded hashes
    SnapshotMismatch {
        differences: Vec<SnapshotDifference>,
    },
    /// file operation failed for {path:?}
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// {path:?} changed, but synchronizing it failed; the change may not survive a crash
    Unsynchronized {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// {path:?} does not hold a valid JSON document
    Json {
        path: PathBuf,
        #[source]
        source: JsonError,
    },
    /// computed digest is not a valid content reference
    Digest(#[source] InvalidContentRef),
}

/// A retained run file whose bytes differ from the recorded hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDifference {
    pub file: SnapshotFile,
    /// Hash of the current bytes, or `None` when the file is missing.
    pub actual: Option<ContentRef>,
}

#[review_kit_observation::operations]
impl RunStorageError {
    #[helper]
    pub(super) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    #[helper]
    pub(super) fn unsynchronized(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Unsynchronized {
            path: path.into(),
            source,
        }
    }

    #[helper]
    pub(super) fn json(path: impl Into<PathBuf>, source: JsonError) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}
