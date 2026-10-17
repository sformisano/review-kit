//! Fixture families: repository content for one private fixture project.
//!
//! A family directory `families/<name>/` contains `family.json`, the default
//! branch files under `base/`, and one directory per overlay under
//! `overlays/<overlay>/`. An overlay replaces or creates every file it
//! contains and deletes the paths its descriptor lists. Merge requests start
//! from the default branch with their named overlay; follow-up overlays apply
//! as later commits.
//!
//! Every base carries the committed SkillCatalog manifest
//! [`SKILL_MANIFEST`], which a review host installs in the checkout before a
//! reviewer starts. Overlays never write or delete anything below its
//! directory, so every merge request reviews with the same declared skills.
//!
//! Everything a family pushes, including commit messages, titles and
//! descriptions, is reviewer-visible. [`review_hints`] rejects wording that
//! would disclose an expected review outcome.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const SCHEMA: &str = "fixture-family/v1";
pub const DEFAULT_BRANCH: &str = "main";
pub const SKILL_MANIFEST: &str = ".skillcatalog/skillcatalog.yml";
const SKILL_MANIFEST_DIRECTORY: &str = ".skillcatalog/";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub schema_version: String,
    /// GitLab project path inside the campaign namespace.
    pub project: String,
    pub base_message: String,
    /// Repository paths committed with the executable file mode.
    #[serde(default)]
    pub executable: Vec<String>,
    pub merge_requests: Vec<MergeRequest>,
    pub overlays: BTreeMap<String, Overlay>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    pub label: String,
    pub branch: String,
    pub overlay: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    pub message: String,
    #[serde(default)]
    pub delete: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Family {
    pub name: String,
    pub descriptor: Descriptor,
    pub base: BTreeMap<String, String>,
    pub overlays: BTreeMap<String, BTreeMap<String, String>>,
}

/// File changes for one commit: content to write and paths to delete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub write: BTreeMap<String, String>,
    pub delete: Vec<String>,
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/families")
}

/// Names of every family directory, sorted.
pub fn names(root: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read families {}", root.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("family.json").is_file() {
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("family name is not UTF-8"))?,
            );
        }
    }
    names.sort();
    Ok(names)
}

pub fn load(root: &Path, name: &str) -> Result<Family> {
    ensure!(
        slug(name),
        "family name must use lowercase letters, digits and hyphens"
    );
    let directory = root.join(name);
    let descriptor: Descriptor = serde_json::from_slice(
        &fs::read(directory.join("family.json"))
            .with_context(|| format!("read family descriptor for {name}"))?,
    )
    .with_context(|| format!("parse family descriptor for {name}"))?;
    ensure!(
        descriptor.schema_version == SCHEMA,
        "family {name} uses an unsupported schema"
    );
    ensure!(
        slug(&descriptor.project),
        "family {name} has an invalid project path"
    );
    let base = read_tree(&directory.join("base"))?;
    ensure!(
        base.contains_key("README.md"),
        "family {name} base must contain README.md"
    );
    ensure!(
        base.contains_key(SKILL_MANIFEST),
        "family {name} base must contain {SKILL_MANIFEST}"
    );
    for path in &descriptor.executable {
        ensure!(
            base.contains_key(path),
            "executable path {path} is not a base file"
        );
    }
    let mut overlays = BTreeMap::new();
    let overlay_root = directory.join("overlays");
    for entry in fs::read_dir(&overlay_root).with_context(|| format!("read overlays for {name}"))? {
        let entry = entry?;
        let overlay = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("overlay name is not UTF-8"))?;
        ensure!(
            descriptor.overlays.contains_key(&overlay),
            "overlay directory {overlay} has no descriptor entry"
        );
        overlays.insert(overlay, read_tree(&entry.path())?);
    }
    for (overlay, spec) in &descriptor.overlays {
        ensure!(slug(overlay), "overlay {overlay} has an invalid name");
        let files = overlays.entry(overlay.clone()).or_default();
        ensure!(
            !files.is_empty() || !spec.delete.is_empty(),
            "overlay {overlay} changes nothing"
        );
        ensure!(
            files
                .keys()
                .chain(&spec.delete)
                .all(|path| !path.starts_with(SKILL_MANIFEST_DIRECTORY)),
            "overlay {overlay} changes the committed skill manifest"
        );
        for path in &spec.delete {
            ensure!(
                relative(path) && !files.contains_key(path),
                "overlay {overlay} deletes {path} invalidly"
            );
        }
    }
    ensure!(
        !descriptor.merge_requests.is_empty(),
        "family {name} has no merge request"
    );
    let mut labels = BTreeSet::new();
    let mut branches = BTreeSet::from([DEFAULT_BRANCH.to_owned()]);
    for request in &descriptor.merge_requests {
        ensure!(
            slug(&request.label) && labels.insert(request.label.clone()),
            "merge request label {} is invalid or repeated",
            request.label
        );
        ensure!(
            slug(&request.branch) && branches.insert(request.branch.clone()),
            "merge request branch {} is invalid or repeated",
            request.branch
        );
        ensure!(
            descriptor.overlays.contains_key(&request.overlay),
            "merge request {} names an unknown overlay",
            request.label
        );
    }
    Ok(Family {
        name: name.to_owned(),
        descriptor,
        base,
        overlays,
    })
}

impl Family {
    pub fn merge_request(&self, label: &str) -> Result<&MergeRequest> {
        self.descriptor
            .merge_requests
            .iter()
            .find(|request| request.label == label)
            .with_context(|| format!("family {} has no merge request {label}", self.name))
    }

    pub fn change(&self, overlay: &str) -> Result<(Change, &str)> {
        let spec = self
            .descriptor
            .overlays
            .get(overlay)
            .with_context(|| format!("family {} has no overlay {overlay}", self.name))?;
        Ok((
            Change {
                write: self.overlays.get(overlay).cloned().unwrap_or_default(),
                delete: spec.delete.clone(),
            },
            &spec.message,
        ))
    }

    /// Files after the base commit and the given overlays, in order.
    pub fn materialize(&self, overlays: &[&str]) -> Result<BTreeMap<String, String>> {
        let mut files = self.base.clone();
        for overlay in overlays {
            let (change, _) = self.change(overlay)?;
            for path in &change.delete {
                ensure!(
                    files.remove(path).is_some(),
                    "overlay {overlay} deletes a missing file {path}"
                );
            }
            files.extend(change.write);
        }
        Ok(files)
    }

    /// Every text this family pushes to the provider, labelled by origin.
    pub fn published_texts(&self) -> Vec<(String, &str)> {
        let mut texts = vec![(
            "base commit message".to_owned(),
            self.descriptor.base_message.as_str(),
        )];
        texts.extend(
            self.base
                .iter()
                .map(|(path, text)| (format!("base/{path}"), text.as_str())),
        );
        for (overlay, files) in &self.overlays {
            texts.extend(
                files
                    .iter()
                    .map(|(path, text)| (format!("overlays/{overlay}/{path}"), text.as_str())),
            );
        }
        for (overlay, spec) in &self.descriptor.overlays {
            texts.push((format!("overlay {overlay} message"), spec.message.as_str()));
            texts.extend(
                spec.delete
                    .iter()
                    .map(|path| (format!("overlay {overlay} deletion"), path.as_str())),
            );
        }
        for request in &self.descriptor.merge_requests {
            texts.push((
                format!("merge request {} title", request.label),
                request.title.as_str(),
            ));
            texts.push((
                format!("merge request {} description", request.label),
                request.description.as_str(),
            ));
            texts.push((
                format!("merge request {} branch", request.label),
                request.branch.as_str(),
            ));
        }
        texts.push(("project path".to_owned(), self.descriptor.project.as_str()));
        texts
    }
}

/// Markers that would disclose expected review results to a reviewer.
pub fn review_hints(text: &str) -> Vec<&'static str> {
    const MARKERS: [&str; 6] = ["BUG", "FIXME", "TODO", "XXX", "HACK", "NOTE"];
    const PHRASES: [&str; 15] = [
        "defect",
        "vulnerab",
        "seeded",
        "intentional",
        "deliberate",
        "on purpose",
        "finding",
        "oracle",
        "fixture",
        "review kit",
        "review-kit",
        "rkc",
        "expected outcome",
        "regression for the",
        "insecure",
    ];
    let words: BTreeSet<&str> = text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .collect();
    let lower = text.to_ascii_lowercase();
    MARKERS
        .into_iter()
        .filter(|marker| words.contains(marker))
        .chain(PHRASES.into_iter().filter(|phrase| lower.contains(phrase)))
        .collect()
}

fn slug(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && text.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Reads every regular UTF-8 file below `root`, keyed by its `/`-separated
/// relative path. Symbolic links are rejected so content stays in this tree.
fn read_tree(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in
            fs::read_dir(&directory).with_context(|| format!("read {}", directory.display()))?
        {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("family path is not UTF-8")?
                    .to_owned();
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("family file {} is not UTF-8 text", path.display()))?;
                files.insert(relative, text);
            } else {
                bail!(
                    "family entry {} is not a regular file or directory",
                    path.display()
                );
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_loads_with_merge_requests_and_contract_documents() {
        let names = names(&root()).unwrap();
        assert_eq!(
            names,
            [
                "authorization-boundary",
                "clean-control",
                "compatibility-migration",
                "custom-url-criteria",
                "documentation-workflow",
                "quantity-invariant",
                "readme-contract",
            ]
        );
        for name in names {
            let family = load(&root(), &name).unwrap();
            for request in &family.descriptor.merge_requests {
                let files = family.materialize(&[&request.overlay]).unwrap();
                assert!(files.contains_key("README.md"), "{name}");
            }
            for overlay in family.descriptor.overlays.keys() {
                let (change, message) = family.change(overlay).unwrap();
                assert!(!message.is_empty());
                for (path, text) in &change.write {
                    assert_ne!(
                        family.base.get(path),
                        Some(text),
                        "{name}/{overlay}/{path} repeats base content"
                    );
                }
            }
        }
    }

    #[test]
    fn every_base_declares_the_review_skill_profile() {
        for name in names(&root()).unwrap() {
            let family = load(&root(), &name).unwrap();
            let manifest = &family.base[SKILL_MANIFEST];
            let profile = format!("  id: {}-review\n", family.descriptor.project);
            assert!(manifest.starts_with("profile:\n"), "{name}");
            assert!(manifest.contains(&profile), "{name}");
            assert!(
                manifest.contains("    source: http://127.0.0.1:18731/ai-skills.git\n"),
                "{name}"
            );
            assert!(
                manifest
                    .contains("- catalog_id: ai-skills\n  kind: stack\n  slug: codebase-review\n"),
                "{name}"
            );
        }
    }

    #[test]
    fn overlays_cannot_change_the_skill_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let family = directory.path().join("sample");
        let manifest = family.join("base").join(SKILL_MANIFEST);
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(family.join("base/README.md"), "# Sample\n").unwrap();
        fs::write(&manifest, "profile:\n").unwrap();
        let replacement = family.join("overlays/edit").join(SKILL_MANIFEST);
        fs::create_dir_all(replacement.parent().unwrap()).unwrap();
        fs::write(&replacement, "profile: {}\n").unwrap();
        fs::write(
            family.join("family.json"),
            r#"{"schema_version": "fixture-family/v1", "project": "sample", "base_message": "Start",
                "merge_requests": [{"label": "review", "branch": "edit", "overlay": "edit", "title": "Edit", "description": "Edit"}],
                "overlays": {"edit": {"message": "Edit"}}}"#,
        )
        .unwrap();
        let error = load(directory.path(), "sample").unwrap_err();
        assert_eq!(
            error.to_string(),
            "overlay edit changes the committed skill manifest"
        );

        fs::remove_dir_all(family.join("overlays/edit")).unwrap();
        fs::create_dir_all(family.join("overlays/edit")).unwrap();
        fs::write(family.join("overlays/edit/README.md"), "# Edited\n").unwrap();
        assert!(load(directory.path(), "sample").is_ok());
        let descriptor = fs::read_to_string(family.join("family.json")).unwrap();
        fs::write(
            family.join("family.json"),
            descriptor.replace(
                r#"{"message": "Edit"}"#,
                r#"{"message": "Edit", "delete": [".skillcatalog/skillcatalog.yml"]}"#,
            ),
        )
        .unwrap();
        let error = load(directory.path(), "sample").unwrap_err();
        assert_eq!(
            error.to_string(),
            "overlay edit changes the committed skill manifest"
        );
        fs::write(family.join("family.json"), descriptor).unwrap();
        fs::remove_file(&manifest).unwrap();
        let error = load(directory.path(), "sample").unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("family sample base must contain {SKILL_MANIFEST}")
        );
    }

    #[test]
    fn pushed_family_content_discloses_no_review_outcome() {
        for name in names(&root()).unwrap() {
            let family = load(&root(), &name).unwrap();
            for (origin, text) in family.published_texts() {
                assert_eq!(review_hints(text), Vec::<&str>::new(), "{name}: {origin}");
            }
        }
    }

    #[test]
    fn review_hints_detect_markers_and_phrases() {
        assert_eq!(review_hints("// TODO: later"), ["TODO"]);
        assert_eq!(review_hints("todo list"), Vec::<&str>::new());
        assert_eq!(review_hints("A seeded Defect"), ["defect", "seeded"]);
        assert!(review_hints("fn debug() {}").is_empty());
    }
}
