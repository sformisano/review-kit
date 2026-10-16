use std::{error::Error, path::Path};

use review_kit_application::{ports::PortError, use_cases::publishing::PublicationProgress};
use review_kit_observation::{ErrorInfo, Fields};

use crate::FileError;

pub(crate) fn path_fields(path: &Path) -> Fields {
    Fields::new().secret("path", path.to_string_lossy().into_owned())
}

pub(crate) fn progress_fields(progress: &PublicationProgress) -> Fields {
    Fields::new()
        .flag("unresolved", progress.unresolved.is_some())
        .count("verified_resolutions", progress.verified_resolutions.len())
}

pub(crate) fn file_error(error: &FileError) -> ErrorInfo {
    match error {
        FileError::Io { source, .. } => ErrorInfo::new("file", "io").caused_by(
            ErrorInfo::new(
                "io",
                match source.kind() {
                    std::io::ErrorKind::NotFound => "not_found",
                    std::io::ErrorKind::PermissionDenied => "permission_denied",
                    std::io::ErrorKind::AlreadyExists => "already_exists",
                    std::io::ErrorKind::InvalidInput => "invalid_input",
                    std::io::ErrorKind::InvalidData => "invalid_data",
                    std::io::ErrorKind::WriteZero => "write_zero",
                    std::io::ErrorKind::Interrupted => "interrupted",
                    std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
                    _ => "other",
                },
            )
            .caused_by(ErrorInfo::opaque("io_source")),
        ),
        FileError::Json(_) => ErrorInfo::new("file", "json").caused_by(ErrorInfo::opaque("json")),
        FileError::InvalidPath => ErrorInfo::new("file", "invalid_path"),
        FileError::MissingProgress => ErrorInfo::new("file", "missing_progress"),
        FileError::PublicationBusy => ErrorInfo::new("file", "publication_busy"),
        FileError::EvidenceBinding => ErrorInfo::new("file", "evidence_binding"),
        FileError::FileTooLarge => ErrorInfo::new("file", "file_too_large"),
        FileError::RunJournalExists => ErrorInfo::new("file", "run_journal_exists"),
        FileError::JournalRegression => ErrorInfo::new("file", "journal_regression"),
        FileError::RunStorage(error) => {
            ErrorInfo::new("file", "run_storage").caused_by(crate::runs::run_error(error))
        }
    }
}

pub(crate) fn port_error(error: &PortError) -> ErrorInfo {
    let cause = error.source().map(|source| {
        source
            .downcast_ref::<FileError>()
            .map_or_else(|| ErrorInfo::opaque("port_source"), file_error)
    });
    let projected = ErrorInfo::new("port", "storage");
    match cause {
        Some(cause) => projected.caused_by(cause),
        None => projected,
    }
}
