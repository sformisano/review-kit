use crate::observation::{file_error, path_fields, progress_fields};
use review_kit_observation::{Completion, Fields};

use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

use review_kit_application::use_cases::publishing::PublicationProgress;
use review_kit_domain::{ReviewerId, Subject};
use review_kit_json_adapter::{ContextDocument, DraftDocument, ProgressDocument};
use serde::{Serialize, de::DeserializeOwned};

use crate::{FileDraftWriter, FileError, atomic_create_new, atomic_write};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

#[review_kit_observation::operations]
impl Workspace {
    #[helper]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().into(),
        }
    }
    #[helper]
    pub fn root(&self) -> &Path {
        &self.root
    }
    #[helper]
    pub fn path(&self, path: &Path) -> Result<PathBuf, FileError> {
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(FileError::InvalidPath);
        }
        Ok(self.root.join(path))
    }
    #[operation(
        name = "filesystem.read_bytes",
        input = path_fields(path),
        output = |result| {
            Completion::result(
                result,
                |bytes| Fields::new().count("bytes", bytes.len()),
                file_error,
            )
        },
    )]
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, FileError> {
        read_file(&self.path(path)?)
    }
    #[operation(
        name = "filesystem.write_bytes",
        input = path_fields(path).count("bytes", bytes.len()),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn write_bytes(&self, path: &Path, bytes: &[u8]) -> Result<(), FileError> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(FileError::FileTooLarge);
        }
        atomic_write(&self.path(path)?, bytes)
    }
    #[operation(
        name = "filesystem.create_bytes",
        input = path_fields(path).count("bytes", bytes.len()),
        output = |result| {
            Completion::result(
                result,
                |created| Fields::new().flag("created", *created),
                file_error,
            )
        },
    )]
    pub fn create_bytes(&self, path: &Path, bytes: &[u8]) -> Result<bool, FileError> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(FileError::FileTooLarge);
        }
        atomic_create_new(&self.path(path)?, bytes)
    }
    #[operation(
        name = "filesystem.read_json",
        input = path_fields(path),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn read_json<T: DeserializeOwned>(&self, path: &Path) -> Result<T, FileError> {
        review_kit_json_adapter::decode(&self.read_bytes(path)?).map_err(Into::into)
    }
    #[operation(
        name = "filesystem.write_json",
        input = path_fields(path),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn write_json(&self, path: &Path, value: &impl Serialize) -> Result<(), FileError> {
        self.write_bytes(path, &review_kit_json_adapter::encode(value)?)
    }
    #[operation(
        name = "filesystem.create_json",
        input = path_fields(path),
        output = |result| {
            Completion::result(
                result,
                |created| Fields::new().flag("created", *created),
                file_error,
            )
        },
    )]
    pub fn create_json(&self, path: &Path, value: &impl Serialize) -> Result<bool, FileError> {
        self.create_bytes(path, &review_kit_json_adapter::encode(value)?)
    }
    #[operation(
        name = "filesystem.load_context",
        input = path_fields(path),
        output = |result| {
            Completion::result(
                result,
                |context| Fields::new().count("files", context.changes.files.len()),
                file_error,
            )
        },
    )]
    pub fn load_context(&self, path: &Path) -> Result<ContextDocument, FileError> {
        let context: ContextDocument = self.read_json(path)?;
        context.validate()?;
        Ok(context)
    }
    #[operation(
        name = "filesystem.save_context",
        input = path_fields(path).count("files", context.changes.files.len()),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn save_context(&self, path: &Path, context: &ContextDocument) -> Result<(), FileError> {
        context.validate()?;
        self.write_json(path, context)
    }
    #[operation(
        name = "filesystem.load_draft",
        input = path_fields(path),
        output = |result| {
            Completion::result(
                result,
                |draft| {
                    Fields::new()
                        .count("policies", draft.policies.len())
                        .count("findings", draft.findings.len())
                        .count("new_findings", draft.new_findings.len())
                },
                file_error,
            )
        },
    )]
    pub fn load_draft(&self, path: &Path) -> Result<DraftDocument, FileError> {
        let draft: DraftDocument = self.read_json(path)?;
        draft.validate_reference()?;
        Ok(draft)
    }
    #[helper]
    pub fn draft_writer(&self, path: &Path, context: &str) -> FileDraftWriter {
        FileDraftWriter::new(self.clone(), path.into(), context.into())
    }
    #[operation(
        name = "filesystem.load_progress",
        input = Fields::new().label("store", "workspace"),
        output = |result| Completion::result(result, progress_fields, file_error),
    )]
    pub fn load_progress(
        &self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<PublicationProgress, FileError> {
        let path = Path::new("progress.json");
        match self.read_json::<ProgressDocument>(path) {
            Ok(document) => document
                .into_progress(subject, reviewer)
                .map_err(Into::into),
            Err(FileError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                if self.exists(Path::new("evidence.json"))?
                    || self.exists(Path::new("result.json"))?
                {
                    return Err(FileError::MissingProgress);
                }
                let document =
                    ProgressDocument::new(subject, reviewer, &PublicationProgress::default());
                self.create_json(path, &document)?;
                self.read_json::<ProgressDocument>(path)?
                    .into_progress(subject, reviewer)
                    .map_err(Into::into)
            }
            Err(error) => Err(error),
        }
    }
    #[operation(
        name = "filesystem.save_progress",
        input = progress_fields(progress).label("store", "workspace"),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn save_final_progress(
        &self,
        subject: &Subject,
        reviewer: &ReviewerId,
        progress: &PublicationProgress,
    ) -> Result<(), FileError> {
        let document = ProgressDocument::new(subject, reviewer, progress);
        document.clone().into_progress(subject, reviewer)?;
        self.write_json(Path::new("progress.json"), &document)
    }
    #[helper]
    pub fn exists(&self, path: &Path) -> Result<bool, FileError> {
        let path = self.path(path)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(FileError::io(&path, error)),
        }
    }
}

#[review_kit_observation::operations]
#[operation(
    name = "filesystem.read_file",
    input = path_fields(path),
    output = |result| {
        Completion::result(
            result,
            |bytes| Fields::new().count("bytes", bytes.len()),
            file_error,
        )
    },
)]
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, FileError> {
    let file = File::open(path).map_err(|error| FileError::io(path, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| FileError::io(path, error))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(FileError::FileTooLarge);
    }
    Ok(bytes)
}
