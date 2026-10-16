use std::{io, path::Path};

use review_kit_observation::{Completion, ErrorInfo, Fields, Outcome};

use super::{RunFile, RunStorageError};

#[review_kit_observation::operations]
#[helper]
pub(super) fn path_fields(path: &Path) -> Fields {
    Fields::new().secret("path", path.to_string_lossy().into_owned())
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn file_fields(file: RunFile) -> Fields {
    Fields::new().label("file", file.name())
}

/// A visible but unsynchronized change is uncertain, not failed.
#[review_kit_observation::operations]
#[helper]
pub(super) fn completion<T>(
    result: &Result<T, RunStorageError>,
    output: impl FnOnce(&T) -> Fields,
) -> Completion {
    match result {
        Ok(value) => Completion::success(output(value)),
        Err(error @ RunStorageError::Unsynchronized { .. }) => {
            Completion::new(Outcome::Uncertain, Fields::new()).with_error(run_error(error))
        }
        Err(error) => Completion::failure(run_error(error)),
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn run_error(error: &RunStorageError) -> ErrorInfo {
    let variant = match error {
        RunStorageError::GitUnavailable(_) => "git_unavailable",
        RunStorageError::NotInWorktree { .. } => "not_in_worktree",
        RunStorageError::GitListing { .. } => "git_listing",
        RunStorageError::Redirected { .. } => "redirected",
        RunStorageError::UnexpectedKind { .. } => "unexpected_kind",
        RunStorageError::Tracked { .. } => "tracked",
        RunStorageError::Unrecognized { .. } => "unrecognized",
        RunStorageError::Uninitialized => "uninitialized",
        RunStorageError::WorkspaceBusy => "workspace_busy",
        RunStorageError::RunExists { .. } => "run_exists",
        RunStorageError::StagingExists { .. } => "staging_exists",
        RunStorageError::IncompleteStaging { .. } => "incomplete_staging",
        RunStorageError::RunMissing { .. } => "run_missing",
        RunStorageError::MissingFile { .. } => "missing_file",
        RunStorageError::FileTooLarge { .. } => "file_too_large",
        RunStorageError::ContentMissing { .. } => "content_missing",
        RunStorageError::ContentCorrupt { .. } => "content_corrupt",
        RunStorageError::SnapshotMismatch { .. } => "snapshot_mismatch",
        RunStorageError::Io { .. } => "io",
        RunStorageError::Unsynchronized { .. } => "unsynchronized",
        RunStorageError::Json { .. } => "json",
        RunStorageError::Digest(_) => "digest",
    };
    let info = ErrorInfo::new("run_storage", variant);
    match error {
        RunStorageError::GitUnavailable(source)
        | RunStorageError::Io { source, .. }
        | RunStorageError::Unsynchronized { source, .. } => info.caused_by(io_cause(source)),
        RunStorageError::Json { .. } => info.caused_by(ErrorInfo::opaque("json")),
        _ => info,
    }
}

#[review_kit_observation::operations]
#[helper]
fn io_cause(source: &io::Error) -> ErrorInfo {
    ErrorInfo::new(
        "io",
        match source.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            io::ErrorKind::AlreadyExists => "already_exists",
            io::ErrorKind::DirectoryNotEmpty => "directory_not_empty",
            io::ErrorKind::NotADirectory => "not_a_directory",
            io::ErrorKind::IsADirectory => "is_a_directory",
            io::ErrorKind::StorageFull => "storage_full",
            io::ErrorKind::ReadOnlyFilesystem => "read_only_filesystem",
            io::ErrorKind::InvalidInput => "invalid_input",
            io::ErrorKind::InvalidData => "invalid_data",
            io::ErrorKind::Interrupted => "interrupted",
            _ => "other",
        },
    )
    .caused_by(ErrorInfo::opaque("io_source"))
}
