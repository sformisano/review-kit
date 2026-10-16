use std::collections::HashSet;

use review_kit_application::use_cases::discovery_bundle::ChangedFile;
use review_kit_domain::{Revision, SourcePath};
use serde::{Deserialize, Serialize};

use crate::GitLabError;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Version {
    pub id: u64,
    pub head_commit_sha: String,
    pub base_commit_sha: String,
    pub start_commit_sha: String,
    pub merge_request_id: u64,
    pub state: String,
    pub real_size: String,
}

impl Version {
    pub fn matches(&self, revision: &Revision) -> bool {
        self.head_commit_sha == revision.head()
            && revision.comparison().first() == Some(&self.base_commit_sha)
            && revision.comparison().get(1) == Some(&self.start_commit_sha)
    }

    pub fn validate(&self) -> Result<usize, GitLabError> {
        if !matches!(self.state.as_str(), "collected" | "empty") {
            return Err(GitLabError::Limit);
        }
        if self.real_size.is_empty() || !self.real_size.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(GitLabError::Limit);
        }
        let size = self.real_size.parse().map_err(|_| GitLabError::Limit)?;
        if self.state == "empty" && size != 0 {
            return Err(GitLabError::Limit);
        }
        Ok(size)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DiffVersion {
    #[serde(flatten)]
    pub version: Version,
    pub diffs: Vec<DiffFile>,
}

impl DiffVersion {
    pub fn validate(&self, selected: &Version, revision: &Revision) -> Result<(), GitLabError> {
        if self.version.id != selected.id
            || self.version.merge_request_id != selected.merge_request_id
            || !self.version.matches(revision)
            || self.version.real_size != selected.real_size
        {
            return Err(GitLabError::Protocol);
        }
        if self.version.validate()? != self.diffs.len() {
            return Err(GitLabError::Limit);
        }
        let mut current_paths = HashSet::new();
        let mut old_paths = HashSet::new();
        for file in &self.diffs {
            file.validate()?;
            if !current_paths.insert(&file.new_path) || !old_paths.insert(&file.old_path) {
                return Err(GitLabError::Protocol);
            }
        }
        Ok(())
    }

    pub fn file(&self, path: &SourcePath) -> Result<&DiffFile, GitLabError> {
        self.diffs
            .iter()
            .find(|file| file.new_path == path.as_str() && !file.deleted_file)
            .ok_or(GitLabError::UnsupportedPosition)
    }

    pub fn unified_diff(&self) -> Result<String, GitLabError> {
        let mut output = String::new();
        for file in &self.diffs {
            let old = quote_path(&format!("a/{}", file.old_path))?;
            let new = quote_path(&format!("b/{}", file.new_path))?;
            output.push_str(&format!("diff --git {old} {new}\n"));
            if file.new_file {
                output.push_str(&format!("new file mode {}\n", file.b_mode));
            } else if file.deleted_file {
                output.push_str(&format!("deleted file mode {}\n", file.a_mode));
            } else if file.a_mode != file.b_mode {
                output.push_str(&format!(
                    "old mode {}\nnew mode {}\n",
                    file.a_mode, file.b_mode
                ));
            }
            if file.renamed_file {
                output.push_str(&format!(
                    "rename from {}\nrename to {}\n",
                    quote_path(&file.old_path)?,
                    quote_path(&file.new_path)?
                ));
            }
            if !file.diff.is_empty() {
                if file.diff.starts_with("@@") {
                    super::mapping::parse(&file.diff)?;
                    output.push_str(&format!(
                        "--- {}\n+++ {}\n",
                        if file.new_file { "/dev/null" } else { &old },
                        if file.deleted_file { "/dev/null" } else { &new }
                    ));
                } else if !(file.diff.starts_with("Binary files ")
                    || file.diff.starts_with("GIT binary patch\n"))
                {
                    return Err(GitLabError::UnsupportedPosition);
                }
                output.push_str(&file.diff);
                if !file.diff.ends_with('\n') {
                    output.push('\n');
                }
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DiffFile {
    pub old_path: String,
    pub new_path: String,
    pub a_mode: String,
    pub b_mode: String,
    pub new_file: bool,
    pub renamed_file: bool,
    pub deleted_file: bool,
    pub diff: String,
    pub collapsed: bool,
    pub too_large: bool,
}

impl DiffFile {
    fn validate(&self) -> Result<(), GitLabError> {
        if self.collapsed || self.too_large {
            return Err(GitLabError::Limit);
        }
        if u8::from(self.new_file) + u8::from(self.deleted_file) + u8::from(self.renamed_file) > 1
            || (!self.renamed_file && self.old_path != self.new_path)
        {
            return Err(GitLabError::Protocol);
        }
        self.changed_file()?;
        if self.diff.is_empty()
            && !self.new_file
            && !self.deleted_file
            && !self.renamed_file
            && self.a_mode == self.b_mode
        {
            return Err(GitLabError::Limit);
        }
        Ok(())
    }

    pub fn changed_file(&self) -> Result<ChangedFile, GitLabError> {
        let old = SourcePath::try_from(self.old_path.clone())
            .map_err(|_| GitLabError::UnsupportedSource)?;
        let new = SourcePath::try_from(self.new_path.clone())
            .map_err(|_| GitLabError::UnsupportedSource)?;
        Ok(if self.new_file {
            ChangedFile::Added(new)
        } else if self.deleted_file {
            ChangedFile::Deleted(old)
        } else if self.renamed_file {
            ChangedFile::Renamed {
                previous: old,
                current: new,
            }
        } else {
            ChangedFile::Modified(new)
        })
    }
}

pub(super) fn regular_mode(mode: &str) -> bool {
    matches!(mode, "100644" | "100755")
}

pub(super) fn validate_revision(revision: &Revision) -> Result<(), GitLabError> {
    if revision.comparison().len() != 2
        || std::iter::once(revision.head())
            .chain(revision.comparison().iter().map(String::as_str))
            .any(|sha| {
                !matches!(sha.len(), 40 | 64)
                    || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                    || sha.bytes().all(|byte| byte == b'0')
            })
    {
        return Err(GitLabError::Protocol);
    }
    Ok(())
}

fn quote_path(path: &str) -> Result<String, GitLabError> {
    if path
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | '\\'))
    {
        serde_json::to_string(path).map_err(|_| GitLabError::Protocol)
    } else {
        Ok(path.to_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn version() -> DiffVersion {
        serde_json::from_value(json!({
            "id": 12, "merge_request_id": 55,
            "head_commit_sha": "1".repeat(40), "base_commit_sha": "2".repeat(40), "start_commit_sha": "3".repeat(40),
            "state": "collected", "real_size": "1",
            "diffs": [{ "old_path": "old.rs", "new_path": "new.rs", "a_mode": "100644", "b_mode": "100644", "new_file": false, "deleted_file": false, "renamed_file": true, "collapsed": false, "too_large": false, "diff": "@@ -1 +1,2 @@\n same\n+added\n" }]
        })).unwrap()
    }

    #[test]
    fn renamed_files_keep_both_paths_and_exact_hunks() {
        let version = version();
        assert!(
            matches!(version.diffs[0].changed_file().unwrap(), ChangedFile::Renamed { previous, current } if previous.as_str() == "old.rs" && current.as_str() == "new.rs")
        );
        assert_eq!(
            version.unified_diff().unwrap(),
            "diff --git a/old.rs b/new.rs\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1 +1,2 @@\n same\n+added\n"
        );
    }

    #[test]
    fn empty_versions_require_zero_complete_diff_data() {
        let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
        let mut empty = version();
        empty.version.state = "empty".into();
        empty.version.real_size = "0".into();
        empty.diffs.clear();
        assert_eq!(empty.version.validate().unwrap(), 0);
        empty.validate(&empty.version, &revision).unwrap();
        assert_eq!(empty.unified_diff().unwrap(), "");

        let mut nonzero = version();
        nonzero.version.state = "empty".into();
        assert!(matches!(
            nonzero.version.validate(),
            Err(GitLabError::Limit)
        ));
        assert!(matches!(
            nonzero.validate(&nonzero.version, &revision),
            Err(GitLabError::Limit)
        ));

        let mut contradictory = version();
        contradictory.version.state = "empty".into();
        contradictory.version.real_size = "0".into();
        assert!(matches!(
            contradictory.validate(&empty.version, &revision),
            Err(GitLabError::Limit)
        ));
    }

    #[test]
    fn empty_versions_reject_unavailable_states_and_invalid_counts() {
        for state in ["pending", "overflow", "without_files", "unknown", ""] {
            let mut actual = version().version;
            actual.state = state.into();
            actual.real_size = "0".into();
            assert!(
                matches!(actual.validate(), Err(GitLabError::Limit)),
                "{state}"
            );
        }
        for count in ["", "-1", "1+", "invalid", "999999999999999999999999999999"] {
            let mut actual = version().version;
            actual.state = "empty".into();
            actual.real_size = count.into();
            assert!(
                matches!(actual.validate(), Err(GitLabError::Limit)),
                "{count}"
            );
        }
    }

    #[test]
    fn empty_versions_reject_mismatched_identity_counts_and_refs() {
        let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
        let mut data = serde_json::to_value(version()).unwrap();
        data["state"] = json!("empty");
        data["real_size"] = json!("0");
        data["diffs"] = json!([]);
        let expected: DiffVersion = serde_json::from_value(data.clone()).unwrap();
        for (field, value) in [
            ("id", json!(13)),
            ("merge_request_id", json!(56)),
            ("real_size", json!("1")),
            ("head_commit_sha", json!("4".repeat(40))),
            ("base_commit_sha", json!("4".repeat(40))),
            ("start_commit_sha", json!("4".repeat(40))),
        ] {
            let mut invalid = data.clone();
            invalid[field] = value;
            let actual: DiffVersion = serde_json::from_value(invalid).unwrap();
            assert!(
                matches!(
                    actual.validate(&expected.version, &revision),
                    Err(GitLabError::Protocol)
                ),
                "{field}"
            );
        }
    }

    #[test]
    fn version_rejects_overflow_incomplete_and_excluded_diff_data() {
        let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
        let expected = version().version;
        let mut actual = version();
        assert!(actual.validate(&expected, &revision).is_ok());
        actual.diffs.clear();
        assert!(matches!(
            actual.validate(&expected, &revision),
            Err(GitLabError::Limit)
        ));
        let mut actual = version();
        actual.version.state = "overflow".into();
        assert!(matches!(
            actual.validate(&expected, &revision),
            Err(GitLabError::Limit)
        ));
        for large in [false, true] {
            let mut actual = version();
            actual.diffs[0].collapsed = !large;
            actual.diffs[0].too_large = large;
            assert!(matches!(
                actual.validate(&expected, &revision),
                Err(GitLabError::Limit)
            ));
        }
    }

    #[test]
    fn full_immutable_refs_and_regular_file_modes_are_required() {
        assert!(
            validate_revision(
                &Revision::new("main".into(), vec!["2".repeat(40), "3".repeat(40)]).unwrap()
            )
            .is_err()
        );
        assert!(
            validate_revision(&Revision::new("1".repeat(40), vec!["2".repeat(40)]).unwrap())
                .is_err()
        );
        for mode in ["040000", "120000", "160000", "000000"] {
            assert!(!regular_mode(mode));
        }
        assert!(regular_mode("100644"));
        assert!(regular_mode("100755"));
    }
}
