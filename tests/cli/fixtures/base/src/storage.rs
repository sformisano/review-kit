use std::{fs, io, path::Path};

#[derive(Debug)]
pub enum SaveError {
    Retryable(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("retryable save failure")
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Retryable(source) => Some(source),
        }
    }
}

pub fn save(primary: &Path, _audit: &Path, data: &str) -> Result<(), SaveError> {
    fs::write(primary, data).map_err(SaveError::Retryable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            let root = std::env::var_os("TMPDIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let path = root.join(format!("quantity-store-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saves_primary_text() {
        let directory = Directory::new();
        let primary = directory.0.join("data.txt");
        save(&primary, &directory.0.join("audit.txt"), "three items").unwrap();
        assert_eq!(fs::read_to_string(primary).unwrap(), "three items");
    }
}
