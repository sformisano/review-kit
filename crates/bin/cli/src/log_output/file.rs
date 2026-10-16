use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

/// Each flush appends one redacted JSON record under a cross-process file lock.
pub(super) struct RecordFile {
    file: File,
    pending: Vec<u8>,
}

#[review_kit_observation::operations]
impl RecordFile {
    #[helper]
    pub(super) fn open(directory: &Path) -> io::Result<Self> {
        let mut directories = fs::DirBuilder::new();
        directories.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directories.mode(0o700);
        }
        directories.create(directory)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(directory.join("review-kit.jsonl"))?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("logging requires a regular file"));
        }
        Ok(Self {
            file,
            pending: Vec::new(),
        })
    }
}

#[review_kit_observation::operations]
impl Write for RecordFile {
    #[helper]
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    #[helper]
    fn flush(&mut self) -> io::Result<()> {
        // A failed record must not be retried or prefixed to the next record.
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() {
            return Ok(());
        }
        self.file.lock()?;
        let written = self
            .file
            .write_all(&pending)
            .and_then(|()| self.file.flush());
        let unlocked = self.file.unlock();
        written.and(unlocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use review_kit_observation::{Completion, Fields, Observer, observe, with_observer};

    #[test]
    fn failed_append_is_not_replayed_after_the_writer_recovers() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("review-kit.jsonl");
        fs::write(&path, [])?;
        let mut writer = RecordFile {
            file: File::open(&path)?,
            pending: Vec::new(),
        };
        writer.write_all(b"{\"failed\":true}\n")?;
        assert!(writer.flush().is_err());
        writer.file = OpenOptions::new().append(true).open(&path)?;
        writer.write_all(b"{\"successful\":true}\n")?;
        writer.flush()?;
        assert_eq!(fs::read_to_string(path)?, "{\"successful\":true}\n");
        Ok(())
    }

    #[test]
    fn file_write_failure_preserves_the_business_result() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("review-kit.jsonl");
        fs::write(&path, [])?;
        let observer = Observer::json(RecordFile {
            file: File::open(&path)?,
            pending: Vec::new(),
        });
        let result = with_observer(&observer, || {
            observe(
                "test.business",
                Fields::new(),
                || 23,
                |_| Completion::success(Fields::new()),
            )
        });
        assert_eq!(result, 23);
        assert!(fs::read(path)?.is_empty());
        Ok(())
    }
}
