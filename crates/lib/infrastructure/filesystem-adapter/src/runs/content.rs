//! Digest-addressed accepted assessments and archived drafts inside a run directory.
//!
//! An entry is written once and never replaced. Installation writes a
//! synchronized temporary file in `<area>/`, creates `<area>/<hex>/`, links the
//! file into it without replacement, and synchronizes both directories. An
//! interrupted installation can leave an empty digest directory or a temporary
//! file; both are ignored, and a retry completes the entry.

use std::{io, path::Path};

use review_kit_application::review_run::ContentRef;

use super::{
    ContentArea, RunStorageError, StoredContent,
    digest::digest,
    durable::{
        Temporary, check_size, directory_exists, ensure_directory, link, read, sync_directory,
        sync_file,
    },
};

#[review_kit_observation::operations]
#[helper]
pub(super) fn install(
    run: &Path,
    area: ContentArea,
    bytes: &[u8],
) -> Result<StoredContent, RunStorageError> {
    let reference = digest(bytes)?;
    let name = area.file_name();
    check_size(
        &run.join(area.directory()).join(reference.hex()).join(name),
        bytes,
    )?;
    let area_directory = ensure_directory(run, area.directory())?;
    if let Some(stored) = existing(&area_directory, area, &reference)? {
        return Ok(stored);
    }
    let temporary = Temporary::write(&area_directory, bytes)?;
    let directory = ensure_directory(&area_directory, reference.hex())?;
    if link(&temporary, &directory, name)? {
        return Ok(StoredContent {
            reference,
            created: true,
        });
    }
    existing(&area_directory, area, &reference)?.ok_or_else(|| {
        RunStorageError::io(
            directory.join(name),
            io::Error::from(io::ErrorKind::AlreadyExists),
        )
    })
}

/// Verify an existing entry and synchronize it again, since an earlier
/// installation may have failed after linking it.
#[review_kit_observation::operations]
#[helper]
fn existing(
    area_directory: &Path,
    area: ContentArea,
    reference: &ContentRef,
) -> Result<Option<StoredContent>, RunStorageError> {
    let directory = area_directory.join(reference.hex());
    if !directory_exists(&directory)? {
        return Ok(None);
    }
    let path = directory.join(area.file_name());
    let Some(bytes) = read(&path)? else {
        return Ok(None);
    };
    verify(area, reference, &bytes)?;
    sync_file(&path)?;
    for directory in [&directory, area_directory] {
        sync_directory(directory).map_err(|error| RunStorageError::unsynchronized(&path, error))?;
    }
    Ok(Some(StoredContent {
        reference: reference.clone(),
        created: false,
    }))
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn read_verified(
    run: &Path,
    area: ContentArea,
    reference: &ContentRef,
) -> Result<Vec<u8>, RunStorageError> {
    let area_directory = run.join(area.directory());
    let directory = area_directory.join(reference.hex());
    let bytes = if directory_exists(&area_directory)? && directory_exists(&directory)? {
        read(&directory.join(area.file_name()))?
    } else {
        None
    };
    let bytes = bytes.ok_or_else(|| RunStorageError::ContentMissing {
        area,
        reference: reference.clone(),
    })?;
    verify(area, reference, &bytes)?;
    Ok(bytes)
}

#[review_kit_observation::operations]
#[helper]
fn verify(area: ContentArea, reference: &ContentRef, bytes: &[u8]) -> Result<(), RunStorageError> {
    let actual = digest(bytes)?;
    if actual != *reference {
        return Err(RunStorageError::ContentCorrupt {
            area,
            reference: reference.clone(),
            actual,
        });
    }
    Ok(())
}
