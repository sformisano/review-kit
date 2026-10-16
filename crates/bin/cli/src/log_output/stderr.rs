use std::io::{self, Write};

/// A log record holds stderr's lock through its newline, so progress cannot split JSON.
#[derive(Default)]
pub(super) struct RecordStderr {
    pending: Vec<u8>,
}

#[review_kit_observation::operations]
impl Write for RecordStderr {
    #[helper]
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    #[helper]
    fn flush(&mut self) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        let mut stderr = io::stderr().lock();
        stderr.write_all(&pending)?;
        stderr.flush()
    }
}
