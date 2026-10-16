//! Runtime review storage under `<worktree-root>/.review-kit/`.
//!
//! This module owns runtime file names, directory layout, environment checks, the
//! workspace lock and write durability. Document schemas belong to the JSON
//! adapter: callers pass serde values for JSON files and exact bytes for drafts
//! and accepted snapshots. The crate README describes the storage contract.

mod content;
mod digest;
mod durable;
mod environment;
mod error;
mod files;
mod git;
mod layout;
mod lock;
mod observation;
mod run;
mod staging;
mod store;
mod writer;

pub use digest::content_ref;
pub use error::{RunStorageError, SnapshotDifference};
pub use files::{ContentArea, RenderedFile, RunFile, SnapshotFile, WorkingFile};
pub use git::GitWorktree;
pub use lock::WorkspaceLock;
pub use run::{RunDir, StoredContent};
pub use staging::StagedRun;
pub use store::{RunListing, RuntimeStore};
pub use writer::RunWriter;

pub(crate) use observation::run_error;
