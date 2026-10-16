use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use review_kit_application::ports::ReviewReader;
use review_kit_filesystem_adapter::{FileProgressStore, SavedContext};
use review_kit_json_adapter::encode;
use review_kit_observation::{Observer, with_observer};
use serde::{Serialize, Serializer, ser::Error};
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
    fn read(&self) -> (String, Vec<Value>) {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        let records = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
            .collect();
        (text, records)
    }
}

fn completion<'a>(records: &'a [Value], operation: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["operation"] == operation && record["phase"] == "completed")
        .unwrap()
}

#[test]
fn library_storage_logs_nested_operations_without_exposing_documents() {
    let capture = Capture::default();
    let observer = Observer::json(capture.clone());
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, set, request) = fixture();
    let secret = "SECRET-storage-source-and-review-content";
    let secret_path = Path::new("SECRET-storage-path/context.json");
    let context = ContextDocument::new(
        &snapshot,
        &ObservedChanges {
            subject: snapshot.subject.clone(),
            revision: snapshot.revision.clone(),
            files: Vec::new(),
            unified_diff: secret.into(),
        },
    );
    with_observer(&observer, || {
        workspace.save_context(secret_path, &context).unwrap();
        let loaded = workspace.load_context(secret_path).unwrap();
        assert_eq!(loaded.changes.unified_diff, secret);
        let mut saved = SavedContext::new(&loaded).unwrap();
        assert_eq!(
            saved.discover(&snapshot.subject).unwrap().subject,
            snapshot.subject
        );
        let bundle = loaded.bundle().unwrap();
        let mut writer = workspace.draft_writer(
            Path::new("assessment.json"),
            "SECRET-storage-path/context.json",
        );
        assert_eq!(
            initialize(&bundle, set.clone(), &mut writer)
                .unwrap()
                .outcome,
            DraftCreation::Created
        );
        assert_eq!(
            initialize(&bundle, set.clone(), &mut writer)
                .unwrap()
                .outcome,
            DraftCreation::Existing
        );
        workspace.load_draft(Path::new("assessment.json")).unwrap();

        let store = FileProgressStore::new(
            directory.0.path().join("shared"),
            &snapshot.subject,
            &snapshot.reviewer,
        );
        let progress = store.load(&workspace).unwrap();
        store.save(&progress).unwrap();
        let mut evidence = FileEvidence::with_progress_store(&workspace, &store).unwrap();
        evidence
            .record(ExecutionEvent::Started {
                at: std::time::SystemTime::now(),
                assessment: &request.assessment,
            })
            .unwrap();
    });
    let (text, records) = capture.read();
    assert!(!records.is_empty());
    for secret in [
        secret,
        "SECRET-storage-path",
        "SECRET-policy-name",
        snapshot.subject.host(),
        snapshot.subject.repository(),
    ] {
        assert!(
            !text.contains(secret),
            "sensitive value appeared in operation output"
        );
    }
    let save = completion(&records, "filesystem.save_context");
    assert!(save["parent_execution_id"].is_null());
    assert_eq!(save["outcome"], "success");
    assert!(save["duration_us"].is_u64());
    let started = records
        .iter()
        .find(|record| {
            record["execution_id"] == save["execution_id"] && record["phase"] == "started"
        })
        .unwrap();
    assert!(started["input"]["files"].is_u64());
    assert!(started["input"]["path"].is_string());
    let write = completion(&records, "filesystem.write_json");
    assert_eq!(write["parent_execution_id"], save["execution_id"]);
    let encoding = completion(&records, "json.encode");
    assert_eq!(encoding["parent_execution_id"], write["execution_id"]);
    assert!(encoding["output"]["bytes"].is_u64());
    let atomic = completion(&records, "filesystem.atomic_write");
    assert!(atomic["output"]["installed"].is_boolean());
    assert_eq!(atomic["output"]["installed"], true);
    for operation in [
        "filesystem.load_context",
        "filesystem.restore_context",
        "filesystem.discover_saved_context",
        "filesystem.create_draft",
        "filesystem.load_draft",
        "filesystem.load_progress",
        "filesystem.save_progress",
        "filesystem.load_evidence",
        "filesystem.record_evidence",
        "json.decode",
        "json.context_bundle",
        "json.restore_progress",
    ] {
        assert_eq!(
            completion(&records, operation)["outcome"],
            "success",
            "{operation}"
        );
    }
    assert!(records.iter().any(|record| {
        record["operation"] == "filesystem.create_draft"
            && record["phase"] == "completed"
            && record["output"]["created"] == false
    }));
}

struct SerializationFailure;

impl Serialize for SerializationFailure {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("SECRET-serializer-error-message"))
    }
}

#[test]
fn library_storage_failures_keep_typed_errors_and_hide_foreign_causes() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let capture = Capture::default();
    let observer = Observer::json(capture.clone());
    with_observer(&observer, || {
        assert!(matches!(
            workspace.read_bytes(Path::new("../SECRET-traversal")),
            Err(FileError::InvalidPath)
        ));
        assert!(matches!(
            workspace.read_bytes(Path::new("SECRET-missing")),
            Err(FileError::Io { .. })
        ));
        workspace
            .write_bytes(
                Path::new("SECRET-malformed"),
                br#"{"SECRET-unknown-field":"SECRET-raw-content"}"#,
            )
            .unwrap();
        assert!(matches!(
            workspace.read_json::<ContextDocument>(Path::new("SECRET-malformed")),
            Err(FileError::Json(_))
        ));
        assert!(matches!(
            encode(&SerializationFailure),
            Err(review_kit_json_adapter::JsonError::Syntax(_))
        ));
    });
    let (text, records) = capture.read();
    assert!(!records.is_empty());
    assert!(!text.contains("SECRET-"));
    let file = completion(&records, "filesystem.read_file");
    assert_eq!(file["outcome"], "failure");
    assert_eq!(file["error"]["variant"], "io");
    assert_eq!(file["error"]["cause"]["variant"], "not_found");
    assert_eq!(file["error"]["cause"]["cause"]["opaque"], true);
    let json = completion(&records, "json.decode");
    assert_eq!(json["error"]["variant"], "syntax");
    assert_eq!(json["error"]["cause"]["variant"], "data");
    assert_eq!(json["error"]["cause"]["cause"]["opaque"], true);
    assert_eq!(completion(&records, "json.encode")["outcome"], "failure");
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("SECRET-log-writer-failure"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("SECRET-log-flush-failure"))
    }
}

#[test]
fn failed_log_delivery_keeps_evidence_and_progress_persistence_working() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, request) = fixture();
    with_observer(&Observer::json(FailingWriter), || {
        workspace
            .load_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap();
        let mut evidence =
            FileEvidence::new(&workspace, &snapshot.subject, &snapshot.reviewer).unwrap();
        evidence
            .record(ExecutionEvent::Started {
                at: std::time::SystemTime::now(),
                assessment: &request.assessment,
            })
            .unwrap();
    });
    let evidence: review_kit_json_adapter::EvidenceDocument =
        workspace.read_json(Path::new("evidence.json")).unwrap();
    assert_eq!(evidence.events.len(), 1);
    assert!(
        workspace
            .load_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap()
            .unresolved
            .is_none()
    );
}
