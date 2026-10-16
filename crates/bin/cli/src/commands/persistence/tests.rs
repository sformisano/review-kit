#![allow(clippy::unwrap_used)]

use std::fs;

use review_kit_application::{
    publication::{ProviderId, Write},
    use_cases::publishing::{PublicationProgress, UnresolvedWrite},
};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_filesystem_adapter::{FileProgressStore, Workspace};
use serde_json::{Value, json};

use super::save_result;

#[test]
fn final_saves_preserve_outcome_and_attempt_both_independent_storage_paths() {
    for (fail_progress, fail_result) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(root.path().join("review"));
        let subject = Subject::new(
            "gitlab".into(),
            "https://gitlab.example.com".into(),
            "team/project".into(),
            1,
        )
        .unwrap();
        let reviewer = ReviewerId::try_from("17".to_owned()).unwrap();
        let store = FileProgressStore::new(root.path().join("state"), &subject, &reviewer);
        store.load(&workspace).unwrap();
        let progress = PublicationProgress {
            verified_resolutions: Vec::new(),
            unresolved: Some(UnresolvedWrite {
                phase: Default::default(),
                baseline: None,
                renderer: None,
                subject,
                reviewer,
                write: Write::SetResolved {
                    thread: ProviderId::new("31".to_owned()).unwrap(),
                    resolved: true,
                },
            }),
            ..Default::default()
        };
        let document = json!({"result": {"status": "uncertain"}});
        if fail_progress {
            fs::remove_file(store.path()).unwrap();
        }
        if fail_result {
            fs::create_dir(workspace.root().join("result.json")).unwrap();
        }

        let (exit, diagnostics) = save_result(&workspace, &store, &progress, &document, 6);

        assert_eq!(exit, if fail_progress || fail_result { 7 } else { 6 });
        let codes: Vec<_> = diagnostics
            .iter()
            .map(|error| error.code.as_str())
            .collect();
        let expected: Vec<_> = [
            fail_progress.then_some("progress_storage"),
            fail_result.then_some("result_storage"),
        ]
        .into_iter()
        .flatten()
        .collect();
        assert_eq!(codes, expected);
        if fail_progress {
            assert!(!store.path().exists());
        } else {
            assert_eq!(store.load(&workspace).unwrap(), progress);
        }
        if fail_result {
            assert!(workspace.root().join("result.json").is_dir());
        } else {
            assert_eq!(
                workspace
                    .read_json::<Value>(std::path::Path::new("result.json"))
                    .unwrap(),
                document
            );
        }
    }
}
