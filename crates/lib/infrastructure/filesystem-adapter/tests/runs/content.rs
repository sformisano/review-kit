use std::fs;

use review_kit_application::review_run::ContentRef;
use review_kit_filesystem_adapter::{ContentArea, RunStorageError, StoredContent, content_ref};

use super::harness::{Repository, entries, install_run, run_id};

const ASSESSMENT: &[u8] = b"{\"policy_set\": \"rust\"}\n";

#[test]
fn content_references_hash_exact_bytes() {
    assert_eq!(
        content_ref(b"abc").unwrap().as_str(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(
        content_ref(b"{\"a\":1}").unwrap(),
        content_ref(b"{\"a\": 1}").unwrap()
    );
}

// Acceptance freezes exact bytes in a digest directory; repeating it with identical
// bytes is idempotent.
#[test]
fn accepted_installation_is_idempotent() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let reference = content_ref(ASSESSMENT).unwrap();

    let first = run.install_accepted(ASSESSMENT).unwrap();
    let second = run.install_accepted(ASSESSMENT).unwrap();

    assert_eq!(
        first,
        StoredContent {
            reference: reference.clone(),
            created: true
        }
    );
    assert_eq!(
        second,
        StoredContent {
            reference: reference.clone(),
            created: false
        }
    );
    let accepted = run.path().join("accepted");
    assert_eq!(entries(&accepted), vec![reference.hex().to_owned()]);
    assert_eq!(
        fs::read(
            accepted
                .join(reference.hex())
                .join("review-assessment.json")
        )
        .unwrap(),
        ASSESSMENT
    );
    assert_eq!(run.read_accepted(&reference).unwrap(), ASSESSMENT);
}

// Stored snapshot bytes that differ from their digest are corruption; installing
// the original bytes again reports it and leaves the stored bytes untouched.
#[test]
fn accepted_installation_detects_corrupt_existing_bytes() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let reference = run.install_accepted(ASSESSMENT).unwrap().reference;
    let stored = run
        .path()
        .join("accepted")
        .join(reference.hex())
        .join("review-assessment.json");
    fs::write(&stored, b"tampered").unwrap();

    match run.install_accepted(ASSESSMENT) {
        Err(RunStorageError::ContentCorrupt {
            area,
            reference: expected,
            actual,
        }) => {
            assert_eq!(area, ContentArea::Accepted);
            assert_eq!(expected, reference);
            assert_eq!(actual, content_ref(b"tampered").unwrap());
        }
        other => panic!("expected ContentCorrupt, got {other:?}"),
    }
    assert_eq!(fs::read(&stored).unwrap(), b"tampered");
}

// Reading an accepted snapshot verifies its digest.
#[test]
fn accepted_reads_verify_their_digest() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let unknown = ContentRef::from_hex(&"0".repeat(64)).unwrap();
    assert!(matches!(
        run.read_accepted(&unknown),
        Err(RunStorageError::ContentMissing {
            area: ContentArea::Accepted,
            ..
        })
    ));

    let reference = run.install_accepted(ASSESSMENT).unwrap().reference;
    assert!(matches!(
        run.read_archived_draft(&reference),
        Err(RunStorageError::ContentMissing {
            area: ContentArea::DraftHistory,
            ..
        })
    ));

    fs::write(
        run.path()
            .join("accepted")
            .join(reference.hex())
            .join("review-assessment.json"),
        b"{}",
    )
    .unwrap();
    let reader = repository.store().open_run(&run_id(1)).unwrap();
    match reader.read_accepted(&reference) {
        Err(RunStorageError::ContentCorrupt { actual, .. }) => {
            assert_eq!(actual, content_ref(b"{}").unwrap());
        }
        other => panic!("expected ContentCorrupt, got {other:?}"),
    }
}

// Revision archives the exact draft bytes, even malformed JSON, once per digest.
#[test]
fn draft_archive_keeps_exact_bytes_and_is_idempotent() {
    let repository = Repository::new();
    let lock = repository.store().create_and_lock().unwrap();
    let run = install_run(&lock, &run_id(1));
    let malformed = b"{\"policies\": [ unterminated";

    let first = run.archive_draft(malformed).unwrap();
    let second = run.archive_draft(malformed).unwrap();

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.reference, second.reference);
    assert_eq!(
        fs::read(
            run.path()
                .join("draft-history")
                .join(first.reference.hex())
                .join("review-assessment.draft.json")
        )
        .unwrap(),
        malformed
    );
    assert_eq!(
        run.read_archived_draft(&first.reference).unwrap(),
        malformed
    );
    assert!(!run.path().join("accepted").exists());
}
