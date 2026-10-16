use std::{ops::Deref, path::PathBuf};

use review_kit_application::review_run::RunId;
use review_kit_observation::Fields;
use serde::Serialize;

use super::{
    ContentArea, RenderedFile, RunDir, RunFile, RunStorageError, StoredContent, WorkingFile,
    WorkspaceLock, content,
    durable::{create, directory_exists, ensure_directory, remove, remove_temporaries, replace},
    layout::RENDERED_DIR,
    observation::{completion, file_fields},
};

/// An installed run opened under the workspace lock.
///
/// Mutations cannot replace retained snapshot files, and every write is
/// durable. Read access comes from [`RunDir`] through `Deref`.
#[derive(Debug)]
pub struct RunWriter<'lock> {
    run: RunDir,
    _lock: &'lock WorkspaceLock,
}

#[review_kit_observation::operations]
impl<'lock> RunWriter<'lock> {
    /// Open the run and remove temporary files left in its directories.
    #[helper]
    pub(super) fn open(
        lock: &'lock WorkspaceLock,
        run_id: &RunId,
    ) -> Result<Self, RunStorageError> {
        let run = RunDir::open(lock.store().path(), run_id)?;
        remove_temporaries(run.path());
        for area in [ContentArea::Accepted, ContentArea::DraftHistory] {
            remove_temporaries(&run.path().join(area.directory()));
        }
        remove_temporaries(&run.path().join(RENDERED_DIR));
        Ok(Self { run, _lock: lock })
    }

    /// Replace a working file atomically and durably.
    #[operation(
        name = "filesystem.runs.replace_file",
        input = file_fields(file.into()).count("bytes", bytes.len()),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn replace(&self, file: WorkingFile, bytes: &[u8]) -> Result<(), RunStorageError> {
        replace(
            &self.directory(file)?,
            RunFile::from(file).file_name(),
            bytes,
        )
    }

    #[helper]
    pub fn replace_json(
        &self,
        file: WorkingFile,
        document: &impl Serialize,
    ) -> Result<(), RunStorageError> {
        self.replace(file, &self.encode(file, document)?)
    }

    /// Create a working file only when absent. Returns false, leaving the
    /// existing bytes unchanged, when it exists.
    #[operation(
        name = "filesystem.runs.create_file",
        input = file_fields(file.into()).count("bytes", bytes.len()),
        output = |result| completion(result, |created: &bool| Fields::new().flag("created", *created)),
    )]
    pub fn create(&self, file: WorkingFile, bytes: &[u8]) -> Result<bool, RunStorageError> {
        create(
            &self.directory(file)?,
            RunFile::from(file).file_name(),
            bytes,
        )
    }

    #[helper]
    pub fn create_json(
        &self,
        file: WorkingFile,
        document: &impl Serialize,
    ) -> Result<bool, RunStorageError> {
        self.create(file, &self.encode(file, document)?)
    }

    /// Freeze exact checkpoint bytes at `accepted/<hex>/review-assessment.json`.
    ///
    /// Identical stored bytes return `created: false`. Stored bytes that do not
    /// hash to their directory name are `ContentCorrupt` and stay unchanged.
    #[operation(
        name = "filesystem.runs.install_accepted",
        input = Fields::new().count("bytes", bytes.len()),
        output = |result| completion(result, |stored: &StoredContent| Fields::new().flag("created", stored.created)),
    )]
    pub fn install_accepted(&self, bytes: &[u8]) -> Result<StoredContent, RunStorageError> {
        content::install(self.run.path(), ContentArea::Accepted, bytes)
    }

    /// Archive exact draft bytes, valid JSON or not, at
    /// `draft-history/<hex>/review-assessment.draft.json`, with the same rules as
    /// [`Self::install_accepted`].
    #[operation(
        name = "filesystem.runs.archive_draft",
        input = Fields::new().count("bytes", bytes.len()),
        output = |result| completion(result, |stored: &StoredContent| Fields::new().flag("created", stored.created)),
    )]
    pub fn archive_draft(&self, bytes: &[u8]) -> Result<StoredContent, RunStorageError> {
        content::install(self.run.path(), ContentArea::DraftHistory, bytes)
    }

    /// Remove a rendered preview. Returns whether it existed.
    #[operation(
        name = "filesystem.runs.remove_rendered",
        input = file_fields(WorkingFile::Rendered(file).into()),
        output = |result| completion(result, |removed: &bool| Fields::new().flag("removed", *removed)),
    )]
    pub fn remove_rendered(&self, file: RenderedFile) -> Result<bool, RunStorageError> {
        let directory = self.run.path().join(RENDERED_DIR);
        if !directory_exists(&directory)? {
            return Ok(false);
        }
        remove(&directory, file.name())
    }

    /// The directory holding `file`, created when it is a missing subdirectory.
    #[helper]
    fn directory(&self, file: WorkingFile) -> Result<PathBuf, RunStorageError> {
        if !directory_exists(self.run.path())? {
            return Err(RunStorageError::RunMissing {
                run_id: self.run.run_id().clone(),
            });
        }
        match RunFile::from(file).directory() {
            None => Ok(self.run.path().to_owned()),
            Some(name) => ensure_directory(self.run.path(), name),
        }
    }

    #[helper]
    fn encode(
        &self,
        file: WorkingFile,
        document: &impl Serialize,
    ) -> Result<Vec<u8>, RunStorageError> {
        review_kit_json_adapter::encode(document).map_err(|error| {
            RunStorageError::json(self.run.path().join(RunFile::from(file).name()), error)
        })
    }
}

#[review_kit_observation::operations]
impl Deref for RunWriter<'_> {
    type Target = RunDir;

    #[helper]
    fn deref(&self) -> &RunDir {
        &self.run
    }
}
