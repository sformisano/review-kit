//! Local review storage and evidence persisted before provider dispatch.

mod atomic;
mod context;
mod draft;
mod evidence;
mod observation;
mod progress;
mod runs;
mod workspace;

pub use atomic::{atomic_create_new, atomic_write};
pub use context::SavedContext;
pub use draft::FileDraftWriter;
pub use evidence::FileEvidence;
pub use progress::{FileProgressStore, JournalOrigin, JournalView, OpenedJournal, ProgressLock};
pub use runs::{
    ContentArea, GitWorktree, RenderedFile, RunDir, RunFile, RunListing, RunStorageError,
    RunWriter, RuntimeStore, SnapshotDifference, SnapshotFile, StagedRun, StoredContent,
    WorkingFile, WorkspaceLock, content_ref,
};
pub use workspace::Workspace;

use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum FileError {
    /// file operation failed for {path:?}
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// invalid review document
    Json(
        #[from]
        #[source]
        review_kit_json_adapter::JsonError,
    ),
    /// workspace path must be relative without traversal
    InvalidPath,
    /// publication progress is missing from an initialized review state
    MissingProgress,
    /// another command holds this review's progress lock; retry when it finishes
    PublicationBusy,
    /// evidence belongs to another subject or reviewer
    EvidenceBinding,
    /// review document exceeds the supported file size
    FileTooLarge,
    /// current run journal owns this review; generic progress cannot replace it
    RunJournalExists,
    /// saving this progress would discard a retained publication settlement
    JournalRegression,
    /// review run storage refused the file
    RunStorage(
        #[from]
        #[source]
        RunStorageError,
    ),
}

impl FileError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub struct SystemClock;

impl review_kit_application::use_cases::publication_run::Clock for SystemClock {
    fn now(&mut self) -> SystemTime {
        SystemTime::now()
    }
}
