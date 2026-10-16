use std::fmt;

use review_kit_application::review_run::{ContentRef, SnapshotFiles};

use super::layout::{ACCEPTED_DIR, DRAFT_HISTORY_DIR, RENDERED_DIR};

/// Files captured while a run is staged. Installed runs never replace them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SnapshotFile {
    Context,
    Source,
    Diff,
    Conversation,
}

#[review_kit_observation::operations]
impl SnapshotFile {
    pub const ALL: [Self; 4] = [Self::Context, Self::Source, Self::Diff, Self::Conversation];

    #[helper]
    pub fn name(self) -> &'static str {
        match self {
            Self::Context => SnapshotFiles::CONTEXT,
            Self::Source => SnapshotFiles::SOURCE,
            Self::Diff => SnapshotFiles::DIFF,
            Self::Conversation => SnapshotFiles::CONVERSATION,
        }
    }

    /// The hash that `files` records for this file.
    #[helper]
    pub fn expected(self, files: &SnapshotFiles) -> &ContentRef {
        match self {
            Self::Context => &files.context,
            Self::Source => &files.source,
            Self::Diff => &files.diff,
            Self::Conversation => &files.conversation,
        }
    }
}

/// Rendered previews stored below the run's `rendered/` directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RenderedFile {
    Summary,
    TerminalSummary,
}

#[review_kit_observation::operations]
impl RenderedFile {
    /// File name within `rendered/`.
    #[helper]
    pub fn name(self) -> &'static str {
        match self {
            Self::Summary => "review-summary.md",
            Self::TerminalSummary => "terminal-review-summary.md",
        }
    }
}

/// Run files that commands may create or replace after installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkingFile {
    Manifest,
    Draft,
    CheckResult,
    RenderProfile,
    PublicationEvents,
    PublicationResult,
    Rendered(RenderedFile),
}

/// Every fixed file name within a run directory.
///
/// Accepted snapshots and archived drafts live in digest directories and are
/// addressed by content reference instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunFile {
    Snapshot(SnapshotFile),
    Working(WorkingFile),
}

#[review_kit_observation::operations]
impl RunFile {
    /// Path relative to the run directory, using `/` separators.
    #[helper]
    pub fn name(self) -> &'static str {
        match self {
            Self::Snapshot(file) => file.name(),
            Self::Working(WorkingFile::Manifest) => "review-run.json",
            Self::Working(WorkingFile::Draft) => "review-assessment.draft.json",
            Self::Working(WorkingFile::CheckResult) => "review-check-result.json",
            Self::Working(WorkingFile::RenderProfile) => "review-render-profile.json",
            Self::Working(WorkingFile::PublicationEvents) => "review-publication-events.json",
            Self::Working(WorkingFile::PublicationResult) => "review-publication-result.json",
            Self::Working(WorkingFile::Rendered(RenderedFile::Summary)) => {
                "rendered/review-summary.md"
            }
            Self::Working(WorkingFile::Rendered(RenderedFile::TerminalSummary)) => {
                "rendered/terminal-review-summary.md"
            }
        }
    }

    /// Subdirectory of the run directory that holds this file, if any.
    #[helper]
    pub(super) fn directory(self) -> Option<&'static str> {
        match self {
            Self::Working(WorkingFile::Rendered(_)) => Some(RENDERED_DIR),
            _ => None,
        }
    }

    /// File name within [`Self::directory`] or the run directory.
    #[helper]
    pub(super) fn file_name(self) -> &'static str {
        match self {
            Self::Working(WorkingFile::Rendered(file)) => file.name(),
            _ => self.name(),
        }
    }
}

#[review_kit_observation::operations]
impl fmt::Display for RunFile {
    #[helper]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[review_kit_observation::operations]
impl From<SnapshotFile> for RunFile {
    #[helper]
    fn from(file: SnapshotFile) -> Self {
        Self::Snapshot(file)
    }
}

#[review_kit_observation::operations]
impl From<WorkingFile> for RunFile {
    #[helper]
    fn from(file: WorkingFile) -> Self {
        Self::Working(file)
    }
}

#[review_kit_observation::operations]
impl From<RenderedFile> for WorkingFile {
    #[helper]
    fn from(file: RenderedFile) -> Self {
        Self::Rendered(file)
    }
}

/// Digest-addressed areas of a run directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentArea {
    /// `accepted/<hex>/review-assessment.json`: frozen checkpoint bytes.
    Accepted,
    /// `draft-history/<hex>/review-assessment.draft.json`: archived draft bytes.
    DraftHistory,
}

#[review_kit_observation::operations]
impl ContentArea {
    #[helper]
    pub(super) fn directory(self) -> &'static str {
        match self {
            Self::Accepted => ACCEPTED_DIR,
            Self::DraftHistory => DRAFT_HISTORY_DIR,
        }
    }

    #[helper]
    pub(super) fn file_name(self) -> &'static str {
        match self {
            Self::Accepted => "review-assessment.json",
            Self::DraftHistory => "review-assessment.draft.json",
        }
    }
}
