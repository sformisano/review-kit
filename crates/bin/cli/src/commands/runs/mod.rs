//! Review runs stored under the current Git worktree's `.review-kit/` directory.
//!
//! Every command resolves storage from the worktree root, pins the selected run
//! once, and validates the run's identity and retained files before using them.
//! Local mutations hold the workspace lock; journal access holds the
//! subject/reviewer lock second.

pub mod chain;
pub mod checkpoints;
pub mod environment;
pub mod journal;
pub mod lifecycle;
pub mod listing;
pub mod loading;
pub mod publication;
pub mod report;
pub mod select;
pub mod start;
