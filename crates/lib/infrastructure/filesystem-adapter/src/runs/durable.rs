//! Durable file primitives that never follow a symbolic link at a managed entry.
//!
//! Every write completes and synchronizes a temporary file in the destination
//! directory, installs it with one rename or hard link, and synchronizes that
//! directory. A failure before installation leaves the destination unchanged.
//! A failure while synchronizing after installation is `Unsynchronized`.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    RunStorageError,
    layout::{MAX_FILE_BYTES, is_temporary, temporary_name},
};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// What a managed path currently names, without following a final symbolic link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry {
    Absent,
    File,
    Directory,
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn inspect(path: &Path) -> Result<Entry, RunStorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(RunStorageError::Redirected { path: path.into() })
        }
        Ok(metadata) if metadata.is_dir() => Ok(Entry::Directory),
        Ok(metadata) if metadata.is_file() => Ok(Entry::File),
        Ok(_) => Err(RunStorageError::UnexpectedKind { path: path.into() }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Entry::Absent),
        Err(error) => Err(RunStorageError::io(path, error)),
    }
}

/// Returns whether `path` is a real directory; any other existing entry is an error.
#[review_kit_observation::operations]
#[helper]
pub(super) fn directory_exists(path: &Path) -> Result<bool, RunStorageError> {
    match inspect(path)? {
        Entry::Absent => Ok(false),
        Entry::Directory => Ok(true),
        Entry::File => Err(RunStorageError::UnexpectedKind { path: path.into() }),
    }
}

/// Returns whether `path` is a regular file; any other existing entry is an error.
#[review_kit_observation::operations]
#[helper]
pub(super) fn file_exists(path: &Path) -> Result<bool, RunStorageError> {
    match inspect(path)? {
        Entry::Absent => Ok(false),
        Entry::File => Ok(true),
        Entry::Directory => Err(RunStorageError::UnexpectedKind { path: path.into() }),
    }
}

/// Create `parent/name` when absent and persist its entry in `parent`.
///
/// The parent is synchronized even when the directory already exists, because
/// an interrupted creation can leave an entry that is visible but not durable.
#[review_kit_observation::operations]
#[helper]
pub(super) fn ensure_directory(parent: &Path, name: &str) -> Result<PathBuf, RunStorageError> {
    let path = parent.join(name);
    if !directory_exists(&path)? {
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists && directory_exists(&path)? => {}
            Err(error) => return Err(RunStorageError::io(&path, error)),
        }
    }
    sync_directory(parent).map_err(|error| RunStorageError::unsynchronized(&path, error))?;
    Ok(path)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    options
}

/// Open an existing or new file without following a final symbolic link.
#[review_kit_observation::operations]
#[helper]
pub(super) fn open_file(
    path: &Path,
    configure: fn(&mut OpenOptions),
) -> Result<File, RunStorageError> {
    let mut options = open_options();
    configure(&mut options);
    options.open(path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return RunStorageError::Redirected { path: path.into() };
        }
        RunStorageError::io(path, error)
    })
}

/// Read a regular file, or `None` when it is absent.
#[review_kit_observation::operations]
#[helper]
pub(super) fn read(path: &Path) -> Result<Option<Vec<u8>>, RunStorageError> {
    if !file_exists(path)? {
        return Ok(None);
    }
    let file = match open_file(path, |options| {
        options.read(true);
    }) {
        Ok(file) => file,
        Err(RunStorageError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| RunStorageError::io(path, error))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(RunStorageError::FileTooLarge { path: path.into() });
    }
    Ok(Some(bytes))
}

/// Flush an existing file whose earlier installation may not have been synchronized.
#[review_kit_observation::operations]
#[helper]
pub(super) fn sync_file(path: &Path) -> Result<(), RunStorageError> {
    open_file(path, |options| {
        options.read(true);
    })?
    .sync_all()
    .map_err(|error| RunStorageError::unsynchronized(path, error))
}

/// A complete, synchronized file in its destination directory.
///
/// Dropping it removes the file unless [`Self::installed`] consumed it.
#[derive(Debug)]
pub(super) struct Temporary {
    path: PathBuf,
    remove: bool,
}

#[review_kit_observation::operations]
impl Temporary {
    #[helper]
    pub(super) fn write(directory: &Path, bytes: &[u8]) -> Result<Self, RunStorageError> {
        loop {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(temporary_name(&format!(
                "run-{}-{sequence}",
                std::process::id()
            )));
            let mut file = match open_file(&path, |options| {
                options.write(true).create_new(true);
            }) {
                Ok(file) => file,
                Err(RunStorageError::Io { source, .. })
                    if source.kind() == io::ErrorKind::AlreadyExists =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let temporary = Self { path, remove: true };
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| RunStorageError::io(&temporary.path, error))?;
            return Ok(temporary);
        }
    }

    #[helper]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Record that a rename moved this file to its destination.
    #[helper]
    pub(super) fn installed(mut self) {
        self.remove = false;
    }
}

#[review_kit_observation::operations]
impl Drop for Temporary {
    #[helper]
    fn drop(&mut self) {
        if self.remove {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Reject content that [`read`] could not return.
#[review_kit_observation::operations]
#[helper]
pub(super) fn check_size(target: &Path, bytes: &[u8]) -> Result<(), RunStorageError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(RunStorageError::FileTooLarge {
            path: target.into(),
        });
    }
    Ok(())
}

/// Replace `directory/name` atomically with `bytes`.
#[review_kit_observation::operations]
#[helper]
pub(super) fn replace(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), RunStorageError> {
    let target = directory.join(name);
    check_size(&target, bytes)?;
    file_exists(&target)?;
    let temporary = Temporary::write(directory, bytes)?;
    fs::rename(temporary.path(), &target).map_err(|error| RunStorageError::io(&target, error))?;
    temporary.installed();
    sync_directory(directory).map_err(|error| RunStorageError::unsynchronized(&target, error))
}

/// Create `directory/name` with `bytes` only when it is absent.
///
/// Returns false, leaving the existing bytes unchanged, when the file exists.
#[review_kit_observation::operations]
#[helper]
pub(super) fn create(directory: &Path, name: &str, bytes: &[u8]) -> Result<bool, RunStorageError> {
    let target = directory.join(name);
    check_size(&target, bytes)?;
    if file_exists(&target)? {
        return Ok(false);
    }
    let temporary = Temporary::write(directory, bytes)?;
    link(&temporary, directory, name)
}

/// Install a temporary file at `directory/name` without replacing an existing entry.
///
/// The hard link keeps the temporary name until the temporary is dropped.
#[review_kit_observation::operations]
#[helper]
pub(super) fn link(
    temporary: &Temporary,
    directory: &Path,
    name: &str,
) -> Result<bool, RunStorageError> {
    let target = directory.join(name);
    match fs::hard_link(temporary.path(), &target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(RunStorageError::io(&target, error)),
    }
    sync_directory(directory).map_err(|error| RunStorageError::unsynchronized(&target, error))?;
    Ok(true)
}

/// Remove `directory/name` when it is a regular file. Returns whether it existed.
#[review_kit_observation::operations]
#[helper]
pub(super) fn remove(directory: &Path, name: &str) -> Result<bool, RunStorageError> {
    let target = directory.join(name);
    if !file_exists(&target)? {
        return Ok(false);
    }
    match fs::remove_file(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(RunStorageError::io(&target, error)),
    }
    sync_directory(directory).map_err(|error| RunStorageError::unsynchronized(&target, error))?;
    Ok(true)
}

/// Remove temporary files that an interrupted writer left in `directory`.
///
/// Call only while holding the workspace lock, so no cooperating writer owns
/// them. A missing or redirected directory is skipped. Failures leave the files
/// in place; readers never open temporary names.
#[review_kit_observation::operations]
#[helper]
pub(super) fn remove_temporaries(directory: &Path) {
    if !matches!(inspect(directory), Ok(Entry::Directory)) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut removed = false;
    for entry in entries.flatten() {
        if is_temporary(&entry.file_name()) && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            removed |= fs::remove_file(entry.path()).is_ok();
        }
    }
    if removed {
        let _ = sync_directory(directory);
    }
}
