use crate::observation::{file_error, progress_fields};
use review_kit_observation::{Completion, Fields};

use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use review_kit_application::use_cases::publishing::{ManagedHistory, PublicationProgress};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_json_adapter::ProgressDocument;
use sha2::{Digest, Sha256};

use crate::{FileError, Workspace};

const PROGRESS_FILE: &str = "progress.json";
const JOURNAL_FILE: &str = "publication-progress.json";

/// Publication intent shared by all revision workspaces for one subject and reviewer.
#[derive(Debug, Clone)]
pub struct FileProgressStore {
    root: PathBuf,
    workspace: Workspace,
    pub(crate) subject: Subject,
    pub(crate) reviewer: ReviewerId,
}

/// Excludes cooperating commands using the same store until this guard is dropped.
/// The stable lock file remains present; closing its handle releases the OS lock.
#[derive(Debug)]
#[must_use = "retain the lock through progress loading, publication and final persistence"]
pub struct ProgressLock {
    _file: File,
}

/// How `open_journal` established the run-bound journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalOrigin {
    Loaded,
    /// An empty journal was created for a store without a review-key directory.
    Bootstrapped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedJournal {
    pub progress: PublicationProgress,
    pub origin: JournalOrigin,
}

/// The run-bound journal as stored, read without changing any file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalView {
    /// The review-key directory does not exist.
    Absent,
    /// The required current `publication-progress.json`.
    Journal(Box<PublicationProgress>),
}

#[review_kit_observation::operations]
impl FileProgressStore {
    #[helper]
    pub fn new(root: impl AsRef<Path>, subject: &Subject, reviewer: &ReviewerId) -> Self {
        let root = root.as_ref().to_owned();
        let mut hash = Sha256::new();
        hash.update(b"review-kit-progress-v1");
        let request = subject.request().to_string();
        for value in [
            subject.provider(),
            subject.host(),
            subject.repository(),
            request.as_str(),
            reviewer.as_str(),
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        let workspace = Workspace::new(root.join(format!("{:x}", hash.finalize())));
        Self {
            root,
            workspace,
            subject: subject.clone(),
            reviewer: reviewer.clone(),
        }
    }

    #[helper]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The generic library `progress.json` file.
    #[helper]
    pub fn path(&self) -> PathBuf {
        self.workspace.root().join(PROGRESS_FILE)
    }

    /// The run-bound `publication-progress.json` file.
    #[helper]
    pub fn journal_path(&self) -> PathBuf {
        self.workspace.root().join(JOURNAL_FILE)
    }

    /// Acquire before loading or importing progress and hold through the final save.
    /// Separate state roots and writers that ignore this lock require host coordination.
    #[operation(
        name = "filesystem.lock_progress",
        input = Fields::new().label("store", "shared"),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn try_lock(&self) -> Result<ProgressLock, FileError> {
        crate::atomic::create_directories(&self.root)?;
        let path = self.workspace.root().with_extension("lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|error| FileError::io(&path, error))?;
        match file.try_lock() {
            Ok(()) => Ok(ProgressLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(FileError::PublicationBusy),
            Err(TryLockError::Error(error)) => Err(FileError::io(&path, error)),
        }
    }

    /// Import the supplied workspace only when this review has no shared state directory.
    /// An existing current run journal refuses generic loading.
    #[operation(
        name = "filesystem.load_progress",
        input = Fields::new().label("store", "shared"),
        output = |result| Completion::result(result, progress_fields, file_error),
    )]
    pub fn load(&self, previous: &Workspace) -> Result<PublicationProgress, FileError> {
        if self.directory_present()? {
            self.refuse_run_journal()?;
            return self.load_existing();
        }
        let progress = previous.load_progress(&self.subject, &self.reviewer)?;
        let document = ProgressDocument::new(&self.subject, &self.reviewer, &progress);
        self.workspace
            .create_json(Path::new(PROGRESS_FILE), &document)?;
        self.load_existing()
    }

    /// Preserve the shared record's binding and refuse to recreate missing intent.
    /// An existing current run journal refuses generic saving.
    #[operation(
        name = "filesystem.save_progress",
        input = progress_fields(progress).label("store", "shared"),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn save(&self, progress: &PublicationProgress) -> Result<(), FileError> {
        self.refuse_run_journal()?;
        self.load_existing()?;
        self.workspace
            .save_final_progress(&self.subject, &self.reviewer, progress)
    }

    /// Open the current run journal under the publication lock.
    /// Bootstrap requires an absent key directory and history that
    /// [`ManagedHistory::admits_empty_journal`]. An existing directory without
    /// its journal remains missing.
    #[operation(
        name = "filesystem.open_journal",
        input = Fields::new()
            .flag("managed_history", !history.is_absent())
            .flag("admits_empty_journal", history.admits_empty_journal()),
        output = |result| Completion::result(result, |opened| progress_fields(&opened.progress).label("origin", origin_label(opened.origin)), file_error),
    )]
    pub fn open_journal(&self, history: &ManagedHistory) -> Result<OpenedJournal, FileError> {
        if history.subject() != &self.subject || history.reviewer() != &self.reviewer {
            return Err(FileError::EvidenceBinding);
        }
        if self.directory_present()? {
            return self.open_present();
        }
        if !history.admits_empty_journal() {
            return Err(FileError::MissingProgress);
        }
        let document = ProgressDocument::journal(
            &self.subject,
            &self.reviewer,
            &PublicationProgress::default(),
        );
        if !crate::atomic::atomic_create_directory(
            self.workspace.root(),
            Path::new(JOURNAL_FILE),
            &review_kit_json_adapter::encode(&document)?,
        )? {
            return self.open_present();
        }
        Ok(OpenedJournal {
            progress: self.read_journal()?,
            origin: JournalOrigin::Bootstrapped,
        })
    }

    /// Read the current journal without creating or removing files.
    #[operation(
        name = "filesystem.inspect_journal",
        input = Fields::new(),
        output = |result| Completion::result(result, |view| Fields::new().label("journal", view_label(view)), file_error),
    )]
    pub fn inspect_journal(&self) -> Result<JournalView, FileError> {
        if !self.directory_present()? {
            return Ok(JournalView::Absent);
        }
        Ok(JournalView::Journal(Box::new(self.load_journal()?)))
    }

    /// Load the required current journal; unrelated files remain untouched.
    #[operation(
        name = "filesystem.load_journal",
        input = Fields::new(),
        output = |result| Completion::result(result, progress_fields, file_error),
    )]
    pub fn load_journal(&self) -> Result<PublicationProgress, FileError> {
        if !self.workspace.exists(Path::new(JOURNAL_FILE))? {
            return Err(FileError::MissingProgress);
        }
        self.read_journal()
    }

    /// Replace an existing run-bound journal. Every retained settlement must survive.
    #[operation(
        name = "filesystem.save_journal",
        input = progress_fields(progress)
        .flag("bound", progress.coverage_binding.is_some())
        .count("settled_runs", progress.settled_runs.len()),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn save_journal(&self, progress: &PublicationProgress) -> Result<(), FileError> {
        let stored = self.load_journal()?;
        if stored
            .settled_runs
            .iter()
            .any(|entry| !progress.settled_runs.contains(entry))
        {
            return Err(FileError::JournalRegression);
        }
        let document = ProgressDocument::journal(&self.subject, &self.reviewer, progress);
        document
            .clone()
            .into_journal(&self.subject, &self.reviewer)?;
        self.workspace
            .write_json(Path::new(JOURNAL_FILE), &document)
    }

    #[helper]
    fn open_present(&self) -> Result<OpenedJournal, FileError> {
        Ok(OpenedJournal {
            progress: self.load_journal()?,
            origin: JournalOrigin::Loaded,
        })
    }

    #[helper]
    fn read_journal(&self) -> Result<PublicationProgress, FileError> {
        self.workspace
            .read_json::<ProgressDocument>(Path::new(JOURNAL_FILE))?
            .into_journal(&self.subject, &self.reviewer)
            .map_err(Into::into)
    }

    #[helper]
    fn directory_present(&self) -> Result<bool, FileError> {
        match std::fs::symlink_metadata(self.workspace.root()) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(FileError::io(self.workspace.root(), error)),
        }
    }

    #[helper]
    fn refuse_run_journal(&self) -> Result<(), FileError> {
        if self.workspace.exists(Path::new(JOURNAL_FILE))? {
            return Err(FileError::RunJournalExists);
        }
        Ok(())
    }

    #[helper]
    fn load_existing(&self) -> Result<PublicationProgress, FileError> {
        match self
            .workspace
            .read_json::<ProgressDocument>(Path::new(PROGRESS_FILE))
        {
            Ok(document) => document
                .into_progress(&self.subject, &self.reviewer)
                .map_err(Into::into),
            Err(FileError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                Err(FileError::MissingProgress)
            }
            Err(error) => Err(error),
        }
    }
}

#[review_kit_observation::operations]
#[helper]
fn origin_label(origin: JournalOrigin) -> &'static str {
    match origin {
        JournalOrigin::Loaded => "loaded",
        JournalOrigin::Bootstrapped => "bootstrapped",
    }
}

#[review_kit_observation::operations]
#[helper]
fn view_label(view: &JournalView) -> &'static str {
    match view {
        JournalView::Absent => "absent",
        JournalView::Journal(_) => "journal",
    }
}
