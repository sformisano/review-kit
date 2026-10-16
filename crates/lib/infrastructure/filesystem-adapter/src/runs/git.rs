use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use review_kit_observation::Fields;

use super::{
    RunStorageError,
    layout::RUNTIME_DIR,
    observation::{completion, path_fields},
};

/// The top-level directory of one Git worktree, as Git reports it.
///
/// Linked worktrees of one repository have distinct roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    root: PathBuf,
}

#[review_kit_observation::operations]
impl GitWorktree {
    /// Locate the worktree containing `directory` with `git rev-parse --show-toplevel`.
    ///
    /// Bare repositories and `.git` directories are not inside a worktree.
    #[operation(
        name = "filesystem.runs.locate_worktree",
        input = path_fields(directory),
        output = |result| completion(result, |_| Fields::new()),
    )]
    pub fn locate(directory: &Path) -> Result<Self, RunStorageError> {
        let metadata =
            std::fs::metadata(directory).map_err(|error| RunStorageError::io(directory, error))?;
        if !metadata.is_dir() {
            return Err(RunStorageError::io(
                directory,
                io::Error::from(io::ErrorKind::NotADirectory),
            ));
        }
        let output = git(directory, &["rev-parse", "--show-toplevel"])?;
        let mut stdout = output.stdout;
        if stdout.last() == Some(&b'\n') {
            stdout.pop();
        }
        if !output.status.success() || stdout.is_empty() {
            return Err(RunStorageError::NotInWorktree {
                directory: directory.into(),
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim_end().into(),
            });
        }
        Ok(Self {
            root: path_from_bytes(directory, stdout)?,
        })
    }

    #[helper]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Index paths under `.review-kit`, relative to the root, from `git ls-files -z`.
    #[operation(
        name = "filesystem.runs.list_tracked_runtime",
        input = Fields::new(),
        output = |result| completion(result, |paths: &Vec<PathBuf>| Fields::new().count("paths", paths.len())),
    )]
    pub fn tracked_runtime_paths(&self) -> Result<Vec<PathBuf>, RunStorageError> {
        let output = git(&self.root, &["ls-files", "-z", "--", RUNTIME_DIR])?;
        if !output.status.success() {
            return Err(RunStorageError::GitListing {
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim_end().into(),
            });
        }
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| path_from_bytes(&self.root, path.to_vec()))
            .collect()
    }
}

#[review_kit_observation::operations]
#[helper]
fn git(directory: &Path, arguments: &[&str]) -> Result<Output, RunStorageError> {
    Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .output()
        .map_err(RunStorageError::GitUnavailable)
}

#[review_kit_observation::operations]
#[helper]
fn path_from_bytes(directory: &Path, bytes: Vec<u8>) -> Result<PathBuf, RunStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let _ = directory;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).map(PathBuf::from).map_err(|_| {
            RunStorageError::io(directory, io::Error::from(io::ErrorKind::InvalidData))
        })
    }
}
