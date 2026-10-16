#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use review_kit_observation::{Observer, with_observer};
use serde_json::Value;

use super::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn records(&self) -> (String, Vec<Value>) {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        let records = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
            .collect();
        (text, records)
    }
}

fn failed_sync(path: &Path) -> Result<(), FileError> {
    assert_eq!(fs::read(path).unwrap(), b"SECRET-installed-content");
    Err(FileError::io(
        path,
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "SECRET-sync-failure"),
    ))
}

fn unexpected_sync(_: &Path) -> Result<(), FileError> {
    panic!("a destination that was not installed must not be synchronized")
}

#[test]
fn installed_atomic_files_remain_uncertain_when_parent_sync_fails() {
    for create in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("SECRET-destination");
        if !create {
            fs::write(&path, b"previous bytes").unwrap();
        }
        let installed = Cell::new(false);
        let capture = Capture::default();
        let result = with_observer(&Observer::json(capture.clone()), || {
            if create {
                create_new(&path, b"SECRET-installed-content", &installed, failed_sync)
            } else {
                write(&path, b"SECRET-installed-content", &installed, failed_sync).map(|()| true)
            }
        });
        match result {
            Err(FileError::Io {
                path: actual,
                source,
            }) => {
                assert_eq!(actual, path);
                assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
                assert_eq!(source.to_string(), "SECRET-sync-failure");
            }
            _ => panic!("the parent sync failure must reach the caller"),
        }
        assert_eq!(fs::read(&path).unwrap(), b"SECRET-installed-content");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let (text, records) = capture.records();
        assert_eq!(records.len(), 2);
        assert!(!text.contains("SECRET-"));
        assert_eq!(records[0]["phase"], "started");
        assert_eq!(records[1]["phase"], "completed");
        assert_eq!(records[1]["execution_id"], records[0]["execution_id"]);
        assert!(records[1]["parent_execution_id"].is_null());
        assert_eq!(records[1]["outcome"], "uncertain");
        assert_eq!(records[1]["output"]["installed"], true);
        assert_eq!(records[1]["error"]["variant"], "io");
        assert_eq!(records[1]["error"]["cause"]["variant"], "permission_denied");
        assert_eq!(records[1]["error"]["cause"]["cause"]["opaque"], true);
    }
}

#[test]
fn atomic_failure_before_install_does_not_claim_an_installed_effect() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("SECRET-parent-file");
    fs::write(&parent, b"preserved").unwrap();
    let path = parent.join("destination");
    for create in [false, true] {
        let capture = Capture::default();
        let installed = Cell::new(false);
        let result = with_observer(&Observer::json(capture.clone()), || {
            if create {
                create_new(&path, b"SECRET-content", &installed, unexpected_sync)
            } else {
                write(&path, b"SECRET-content", &installed, unexpected_sync).map(|()| true)
            }
        });
        assert!(matches!(result, Err(FileError::Io { .. })));
        assert_eq!(fs::read(&parent).unwrap(), b"preserved");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let (text, records) = capture.records();
        assert_eq!(records.len(), 2);
        assert!(!text.contains("SECRET-"));
        assert_eq!(records[1]["outcome"], "failure");
        assert_eq!(records[1]["output"]["installed"], false);
    }
}

#[test]
fn existing_atomic_destination_is_preserved_without_sync_or_install() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("SECRET-destination");
    fs::write(&path, b"existing bytes").unwrap();
    let installed = Cell::new(false);
    let capture = Capture::default();
    let created = with_observer(&Observer::json(capture.clone()), || {
        create_new(&path, b"SECRET-replacement", &installed, unexpected_sync).unwrap()
    });
    assert!(!created);
    assert_eq!(fs::read(&path).unwrap(), b"existing bytes");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    let (text, records) = capture.records();
    assert_eq!(records.len(), 2);
    assert!(!text.contains("SECRET-"));
    assert_eq!(records[1]["outcome"], "success");
    assert_eq!(records[1]["output"]["installed"], false);
}

#[test]
fn directory_creation_appears_with_its_file_and_replaces_only_staging_leftovers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("key");
    let staging = staging_directory(&path).unwrap();
    assert_eq!(staging, directory.path().join(".rkc-key.tmp"));
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("kept"), b"kept").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &staging).unwrap();

    assert!(atomic_create_directory(&path, Path::new("journal.json"), b"{}").unwrap());

    assert_eq!(fs::read(path.join("journal.json")).unwrap(), b"{}");
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    assert!(fs::symlink_metadata(&staging).is_err());
    assert_eq!(fs::read(outside.join("kept")).unwrap(), b"kept");

    assert!(!atomic_create_directory(&path, Path::new("journal.json"), b"[]").unwrap());
    assert_eq!(fs::read(path.join("journal.json")).unwrap(), b"{}");
    assert!(fs::symlink_metadata(&staging).is_err());
}
