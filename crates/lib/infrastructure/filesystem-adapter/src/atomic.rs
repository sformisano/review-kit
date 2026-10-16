use std::{
    cell::Cell,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use review_kit_observation::{Completion, Fields, Outcome};

use crate::{
    FileError,
    observation::{file_error, path_fields},
};

#[cfg(test)]
mod tests;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct StagedFile {
    path: PathBuf,
}

#[review_kit_observation::operations]
impl Drop for StagedFile {
    #[helper]
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[review_kit_observation::operations]
#[helper]
fn stage(path: &Path, bytes: &[u8]) -> Result<StagedFile, FileError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_directories(parent)?;
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".rkc-{}-{sequence}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(FileError::io(&temporary, error)),
        };
        let staged = StagedFile { path: temporary };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| FileError::io(&staged.path, error))?;
        return Ok(staged);
    }
}

#[review_kit_observation::operations]
#[helper]
fn sync_parent(path: &Path) -> Result<(), FileError> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| FileError::io(parent, error))?;
    }
    Ok(())
}

/// Persist directory entries, including those left by an interrupted creation.
#[review_kit_observation::operations]
#[helper]
pub(crate) fn create_directories(path: &Path) -> Result<(), FileError> {
    fs::create_dir_all(path).map_err(|error| FileError::io(path, error))?;
    for directory in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        sync_parent(directory)?;
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    let installed = Cell::new(false);
    write(path, bytes, &installed, sync_parent)
}

#[review_kit_observation::operations]
#[operation(
    name = "filesystem.atomic_write",
    input = path_fields(path).count("bytes", bytes.len()),
    output = |result| atomic_completion(result, installed.get()),
)]
fn write(
    path: &Path,
    bytes: &[u8],
    installed: &Cell<bool>,
    synchronize: fn(&Path) -> Result<(), FileError>,
) -> Result<(), FileError> {
    let staged = stage(path, bytes)?;
    fs::rename(&staged.path, path).map_err(|error| FileError::io(path, error))?;
    installed.set(true);
    synchronize(path)
}

/// Returns false when the destination already exists, without changing its bytes.
#[review_kit_observation::operations]
#[helper]
pub fn atomic_create_new(path: &Path, bytes: &[u8]) -> Result<bool, FileError> {
    let installed = Cell::new(false);
    create_new(path, bytes, &installed, sync_parent)
}

#[review_kit_observation::operations]
#[operation(
    name = "filesystem.atomic_create",
    input = path_fields(path).count("bytes", bytes.len()),
    output = |result| atomic_completion(result, installed.get()),
)]
fn create_new(
    path: &Path,
    bytes: &[u8],
    installed: &Cell<bool>,
    synchronize: fn(&Path) -> Result<(), FileError>,
) -> Result<bool, FileError> {
    let staged = stage(path, bytes)?;
    match fs::hard_link(&staged.path, path) {
        Ok(()) => {
            installed.set(true);
            synchronize(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(FileError::io(path, error)),
    }
}

/// A directory under construction; dropping it removes the directory and its contents.
struct StagedDirectory {
    path: Option<PathBuf>,
}

#[review_kit_observation::operations]
impl Drop for StagedDirectory {
    #[helper]
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// The sibling that holds `path` while it is built: `.rkc-<name>.tmp`.
#[review_kit_observation::operations]
#[helper]
fn staging_directory(path: &Path) -> Result<PathBuf, FileError> {
    let name = path.file_name().ok_or(FileError::InvalidPath)?;
    Ok(path.with_file_name(format!(".rkc-{}.tmp", name.to_string_lossy())))
}

/// Create directory `path` holding exactly `file` with `bytes`, or return
/// false when `path` already exists.
///
/// The directory is built under its staging sibling, its file and entries are
/// synchronized, and it is renamed into place, so an existing `path` was
/// created with its file. A staging sibling left by an interrupted creation is
/// removed first; the caller holds the lock that serializes creators.
#[review_kit_observation::operations]
#[helper]
pub(crate) fn atomic_create_directory(
    path: &Path,
    file: &Path,
    bytes: &[u8],
) -> Result<bool, FileError> {
    let installed = Cell::new(false);
    create_directory(path, file, bytes, &installed, sync_parent)
}

#[review_kit_observation::operations]
#[operation(
    name = "filesystem.atomic_create_directory",
    input = path_fields(path).count("bytes", bytes.len()),
    output = |result| atomic_completion(result, installed.get()),
)]
fn create_directory(
    path: &Path,
    file: &Path,
    bytes: &[u8],
    installed: &Cell<bool>,
    synchronize: fn(&Path) -> Result<(), FileError>,
) -> Result<bool, FileError> {
    let staging = staging_directory(path)?;
    if let Some(parent) = staging.parent().filter(|path| !path.as_os_str().is_empty()) {
        create_directories(parent)?;
    }
    remove_leftover(&staging)?;
    fs::create_dir(&staging).map_err(|error| FileError::io(&staging, error))?;
    let mut staged = StagedDirectory {
        path: Some(staging.clone()),
    };
    let target = staging.join(file);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut created = options
        .open(&target)
        .map_err(|error| FileError::io(&target, error))?;
    created
        .write_all(bytes)
        .and_then(|()| created.sync_all())
        .map_err(|error| FileError::io(&target, error))?;
    sync_parent(&target)?;
    if exists(path)? {
        return Ok(false);
    }
    if let Err(error) = fs::rename(&staging, path) {
        return if exists(path)? {
            Ok(false)
        } else {
            Err(FileError::io(path, error))
        };
    }
    staged.path = None;
    installed.set(true);
    synchronize(path)?;
    Ok(true)
}

/// Remove an interrupted creation's staging entry without following links.
#[review_kit_observation::operations]
#[helper]
fn remove_leftover(staging: &Path) -> Result<(), FileError> {
    let removed = match fs::symlink_metadata(staging) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(staging),
        Ok(_) => fs::remove_file(staging),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => Err(error),
    };
    removed.map_err(|error| FileError::io(staging, error))?;
    sync_parent(staging)
}

#[review_kit_observation::operations]
#[helper]
fn exists(path: &Path) -> Result<bool, FileError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(FileError::io(path, error)),
    }
}

#[review_kit_observation::operations]
#[helper]
fn atomic_completion<T>(result: &Result<T, FileError>, installed: bool) -> Completion {
    let output = Fields::new().flag("installed", installed);
    match result {
        Ok(_) => Completion::success(output),
        Err(error) => Completion::new(
            if installed {
                Outcome::Uncertain
            } else {
                Outcome::Failure
            },
            output,
        )
        .with_error(file_error(error)),
    }
}
