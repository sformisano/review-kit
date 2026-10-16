//! Builds the version 1 review workspaces that earlier `rkc discover` and `rkc init`
//! wrote, so legacy recovery commands can be tested without those commands.
//!
//! Files and layout match the earlier commands: `discovery/<unique>/context.json`,
//! `gitlab-source.json`, `diff.patch`, `latest.json` and `assessment.json`.
#![allow(dead_code, reason = "Each test binary uses a subset of the builders.")]

use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use review_kit_application::{
    publication::PolicyName,
    use_cases::{
        assessment::AssessmentPolicies,
        discovery_bundle::discover_bundle,
        initialization::{DraftCreation, initialize},
    },
};
use review_kit_domain::PolicyId;
use review_kit_filesystem_adapter::Workspace;
use review_kit_gitlab_adapter::GitLab;
use review_kit_json_adapter::ContextDocument;
use serde_json::{Value, json};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Retain the current review, diff and committed source under a new `discovery/` directory.
pub fn discover(gitlab: &GitLab, directory: &Path) -> String {
    let bundle = discover_bundle(
        &mut gitlab.clone(),
        &mut gitlab.clone(),
        gitlab.subject(),
        gitlab.reviewer(),
    )
    .unwrap();
    let source = gitlab
        .capture_source(gitlab.subject(), &bundle.snapshot().revision)
        .unwrap();
    let relative = format!(
        "discovery/{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let context = ContextDocument::new(bundle.snapshot(), bundle.changes());
    fs::create_dir_all(directory.join(&relative)).unwrap();
    fs::write(
        directory.join(&relative).join("context.json"),
        review_kit_json_adapter::encode(&context).unwrap(),
    )
    .unwrap();
    fs::write(
        directory.join(&relative).join("gitlab-source.json"),
        source.encode().unwrap(),
    )
    .unwrap();
    fs::write(
        directory.join(&relative).join("diff.patch"),
        bundle.changes().unified_diff.as_bytes(),
    )
    .unwrap();
    let context = format!("{relative}/context.json");
    fs::write(
        directory.join("latest.json"),
        serde_json::to_vec_pretty(&json!({"version": 1, "context": context})).unwrap(),
    )
    .unwrap();
    context
}

/// Create an unassessed `assessment.json` from the latest context; an existing one is kept.
///
/// Returns whether the draft was created.
pub fn init(directory: &Path, policy_set: &str, policies: &[&str]) -> bool {
    let latest: Value =
        serde_json::from_slice(&fs::read(directory.join("latest.json")).unwrap()).unwrap();
    let context_path = latest["context"].as_str().unwrap();
    let workspace = Workspace::new(directory);
    let context: ContextDocument = workspace.read_json(Path::new(context_path)).unwrap();
    let bundle = context.bundle().unwrap();
    let policies = AssessmentPolicies::new(
        policy_set.to_owned(),
        policies
            .iter()
            .map(|policy| {
                let id = PolicyId::try_from((*policy).to_owned()).unwrap();
                PolicyName {
                    name: id.as_str().to_owned(),
                    id,
                }
            })
            .collect(),
    )
    .unwrap();
    let mut writer = workspace.draft_writer(Path::new("assessment.json"), context_path);
    initialize(&bundle, policies, &mut writer).unwrap().outcome == DraftCreation::Created
}
