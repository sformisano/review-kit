use std::path::{Path, PathBuf};

use review_kit_application::review_run::{ContentRef, RunId, SnapshotFiles};
use review_kit_observation::Fields;
use serde::de::DeserializeOwned;

use super::{
    ContentArea, RunFile, RunStorageError, SnapshotDifference, SnapshotFile, content,
    digest::digest,
    durable::{directory_exists, read},
    layout::RUNS_DIR,
    observation::{completion, file_fields},
};

/// An installed run directory, `review-runs/<run-id>/`, opened for reading.
///
/// Opening checks that the directory exists and is not a symbolic link; each
/// access checks the entries it reads. Retained references such as
/// `review-context.json` resolve against [`Self::path`], never against a digest
/// directory.
#[derive(Debug, Clone)]
pub struct RunDir {
    run_id: RunId,
    path: PathBuf,
}

/// Bytes stored in a digest directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContent {
    pub reference: ContentRef,
    /// False when identical bytes were already stored.
    pub created: bool,
}

#[review_kit_observation::operations]
impl RunDir {
    #[helper]
    pub(super) fn open(runtime: &Path, run_id: &RunId) -> Result<Self, RunStorageError> {
        let runs = runtime.join(RUNS_DIR);
        let path = runs.join(run_id.as_str());
        if !directory_exists(&runs)? || !directory_exists(&path)? {
            return Err(RunStorageError::RunMissing {
                run_id: run_id.clone(),
            });
        }
        Ok(Self {
            run_id: run_id.clone(),
            path,
        })
    }

    #[helper]
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The run root, for adapters that resolve retained references against it.
    #[helper]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read a run file, or `None` when it is absent.
    #[operation(
        name = "filesystem.runs.read_file",
        input = file_fields(file),
        output = |result| completion(result, |bytes: &Option<Vec<u8>>| {
            Fields::new().flag("present", bytes.is_some())
        }),
    )]
    pub fn read_optional_bytes(&self, file: RunFile) -> Result<Option<Vec<u8>>, RunStorageError> {
        if !directory_exists(&self.path)? {
            return Err(RunStorageError::RunMissing {
                run_id: self.run_id.clone(),
            });
        }
        let directory = match file.directory() {
            None => self.path.clone(),
            Some(name) => {
                let directory = self.path.join(name);
                if !directory_exists(&directory)? {
                    return Ok(None);
                }
                directory
            }
        };
        read(&directory.join(file.file_name()))
    }

    /// Read a run file that must exist.
    #[helper]
    pub fn read_bytes(&self, file: RunFile) -> Result<Vec<u8>, RunStorageError> {
        self.read_optional_bytes(file)?
            .ok_or_else(|| RunStorageError::MissingFile {
                file,
                path: self.path.join(file.name()),
            })
    }

    /// Decode a run file that must exist.
    #[operation(
        name = "filesystem.runs.read_json",
        input = file_fields(file),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn read_json<T: DeserializeOwned>(&self, file: RunFile) -> Result<T, RunStorageError> {
        review_kit_json_adapter::decode(&self.read_bytes(file)?)
            .map_err(|error| RunStorageError::json(self.path.join(file.name()), error))
    }

    /// The content reference of a run file's current bytes.
    #[operation(
        name = "filesystem.runs.hash_file",
        input = file_fields(file),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn hash(&self, file: RunFile) -> Result<ContentRef, RunStorageError> {
        digest(&self.read_bytes(file)?)
    }

    /// Check the four retained files against their recorded hashes.
    ///
    /// Equal revisions do not imply equal bytes; only the hashes are compared.
    #[operation(
        name = "filesystem.runs.verify_snapshot",
        input = Fields::new(),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn verify_snapshot(&self, expected: &SnapshotFiles) -> Result<(), RunStorageError> {
        let mut differences = Vec::new();
        for file in SnapshotFile::ALL {
            let actual = self
                .read_optional_bytes(file.into())?
                .map(|bytes| digest(&bytes))
                .transpose()?;
            if actual.as_ref() != Some(file.expected(expected)) {
                differences.push(SnapshotDifference { file, actual });
            }
        }
        if differences.is_empty() {
            Ok(())
        } else {
            Err(RunStorageError::SnapshotMismatch { differences })
        }
    }

    /// Read an accepted snapshot and verify that its bytes hash to `reference`.
    #[operation(
        name = "filesystem.runs.read_accepted",
        input = Fields::new(),
        output = |result| completion(result, |bytes: &Vec<u8>| Fields::new().count("bytes", bytes.len())),
    )]
    pub fn read_accepted(&self, reference: &ContentRef) -> Result<Vec<u8>, RunStorageError> {
        content::read_verified(&self.path, ContentArea::Accepted, reference)
    }

    /// Read an archived draft and verify that its bytes hash to `reference`.
    #[operation(
        name = "filesystem.runs.read_archived_draft",
        input = Fields::new(),
        output = |result| completion(result, |bytes: &Vec<u8>| Fields::new().count("bytes", bytes.len())),
    )]
    pub fn read_archived_draft(&self, reference: &ContentRef) -> Result<Vec<u8>, RunStorageError> {
        content::read_verified(&self.path, ContentArea::DraftHistory, reference)
    }
}
