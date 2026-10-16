use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use review_kit_filesystem_adapter::GitWorktree;
use review_kit_observation::{Observer, with_observer};
use serde_json::Value;

use super::harness::{Repository, install_run, run_id};

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
    fn completed(&self, operation: &str) -> (String, Vec<Value>) {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        let records = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
            .filter(|record| record["operation"] == operation && record["phase"] == "completed")
            .collect();
        (text, records)
    }
}

// Storage logs classify outcomes with fixed labels and never disclose paths.
#[test]
fn run_storage_records_mask_paths_and_classify_errors() {
    let repository = Repository::new();
    let plain = repository.outside("SECRET-plain");
    let capture = Capture::default();
    with_observer(&Observer::json(capture.clone()), || {
        assert!(GitWorktree::locate(&plain).is_err());
        let lock = repository.store().create_and_lock().unwrap();
        let run = install_run(&lock, &run_id(1));
        run.install_accepted(b"{\"SECRET\": 1}").unwrap();
    });

    let (text, located) = capture.completed("filesystem.runs.locate_worktree");
    assert!(!text.contains("SECRET"));
    assert!(!text.contains(repository.root().to_str().unwrap()));
    assert_eq!(located[0]["outcome"], "failure");
    assert_eq!(located[0]["error"]["type"], "run_storage");
    assert_eq!(located[0]["error"]["variant"], "not_in_worktree");
    let (_, accepted) = capture.completed("filesystem.runs.install_accepted");
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["outcome"], "success");
    assert_eq!(accepted[0]["output"]["created"], true);
}
