use crate::observation;
use review_kit_observation::Fields;
mod diff;
mod mapping;
mod position;
mod retained;

pub use retained::RetainedSource;

use review_kit_application::{
    ports::{CommittedSource, PortError},
    use_cases::discovery_bundle::{ObservedChanges, ReviewChanges},
};
use review_kit_domain::{
    Finding, FindingDraft, LineNumber, Revision, SourcePath, Subject, Suggestion,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{GitLab, GitLabError, http::segment};
use diff::{DiffFile, DiffVersion, Version};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ObservedPosition {
    Verified { range: Option<(u64, u64)> },
    Outdated,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    mode: String,
}

#[review_kit_observation::operations]
impl GitLab {
    #[operation(
        name = "gitlab.validate_revision",
        input = Fields::new(),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    fn require_revision(&self, revision: &Revision) -> Result<(), GitLabError> {
        if self.refresh()?.revision()? != *revision {
            return Err(GitLabError::RevisionChanged);
        }
        Ok(())
    }

    #[operation(
        name = "gitlab.read_diff",
        input = Fields::new(),
        output = |result| {
            observation::result(result, |diff| {
                Fields::new().count("files", diff.diffs.len())
            })
        },
    )]
    fn diff_version(&self, revision: &Revision) -> Result<DiffVersion, GitLabError> {
        diff::validate_revision(revision)?;
        let versions: Vec<Version> = self
            .api
            .pages(&format!("{}/versions", self.mr_path()), &[])?;
        let mut selected = None;
        let mut ids = std::collections::HashSet::new();
        for version in versions {
            if !ids.insert(version.id)
                || version.id == 0
                || version.merge_request_id != self.binding.mr_id
            {
                return Err(GitLabError::Protocol);
            }
            if version.matches(revision) {
                // Repeated comparisons can have distinct version IDs. Every matching
                // version must remain usable; the newest response entry is retained.
                version.validate()?;
                if selected.is_none() {
                    selected = Some(version);
                }
            }
        }
        let selected = selected.ok_or(GitLabError::UnsupportedPosition)?;
        let version: DiffVersion = self
            .api
            .get(&format!("{}/versions/{}", self.mr_path(), selected.id), &[])?;
        version.validate(&selected, revision)?;
        Ok(version)
    }

    #[operation(
        name = "gitlab.read_source",
        input = Fields::new().secret("path", path.as_str()),
        output = |result| {
            observation::result(result, |bytes| {
                Fields::new()
                    .count("bytes", bytes.len())
                    .secret("source", "committed content")
            })
        },
    )]
    fn committed_bytes(
        &self,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<u8>, GitLabError> {
        diff::validate_revision(revision)?;
        let parent = path
            .as_str()
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        let project = self.binding.source_project.get();
        let entries: Vec<TreeEntry> = self.api.pages(
            &format!("projects/{project}/repository/tree"),
            &[
                ("ref", revision.head().to_owned()),
                ("path", parent.to_owned()),
                ("recursive", "false".to_owned()),
            ],
        )?;
        let mut matches = entries.iter().filter(|entry| entry.path == path.as_str());
        let entry = matches.next().ok_or(GitLabError::UnsupportedSource)?;
        if matches.next().is_some() {
            return Err(GitLabError::Protocol);
        }
        if entry.kind != "blob" || !diff::regular_mode(&entry.mode) {
            return Err(GitLabError::UnsupportedSource);
        }
        let bytes = self.api.bytes(
            &format!(
                "projects/{project}/repository/files/{}/raw",
                segment(path.as_str())
            ),
            &[
                ("ref", revision.head().to_owned()),
                ("lfs", "false".to_owned()),
            ],
        )?;
        Ok(bytes)
    }

    #[helper]
    fn committed_lines(
        &self,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, GitLabError> {
        text_lines(self.committed_bytes(revision, path)?)
    }

    #[helper]
    pub(crate) fn suggestion_contents(
        &self,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<Option<(String, String)>, GitLabError> {
        let Some(suggestion) = finding.suggestion() else {
            return Ok(None);
        };
        let bytes = self.committed_bytes(revision, finding.location().path())?;
        let lines = text_lines(bytes.clone())?;
        let start = usize::try_from(suggestion.start_line().get() - 1)
            .map_err(|_| GitLabError::UnsupportedPosition)?;
        let end = usize::try_from(suggestion.end_line().get())
            .map_err(|_| GitLabError::UnsupportedPosition)?;
        let original = lines
            .get(start..end)
            .ok_or(GitLabError::UnsupportedPosition)?;
        suggestion
            .verify_original(revision, finding.location().path(), original)
            .map_err(|_| GitLabError::UnsupportedPosition)?;
        let text = String::from_utf8(bytes).map_err(|_| GitLabError::UnsupportedSource)?;
        suggestion_contents(
            &text,
            suggestion.start_line().get(),
            suggestion.end_line().get(),
            suggestion.replacement(),
        )
        .map(Some)
    }

    #[helper]
    pub(crate) fn mapped_position(
        &self,
        revision: &Revision,
        finding: &Finding,
        lines: Option<&[String]>,
    ) -> Result<Value, GitLabError> {
        let version = self.diff_version(revision)?;
        let file = version.file(finding.location().path())?;
        position::build(file, revision, finding, lines)
    }

    #[operation(
        name = "gitlab.validate_position",
        input = Fields::new().flag("suggestion", finding.suggestion().is_some()),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    pub(crate) fn position(
        &self,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<Value, GitLabError> {
        self.require_revision(revision)?;
        let lines = self.committed_lines(revision, finding.location().path())?;
        let value = self.mapped_position(revision, finding, Some(&lines))?;
        self.require_revision(revision)?;
        Ok(value)
    }

    #[operation(
        name = "gitlab.verify_position",
        input = Fields::new().flag("suggestion", finding.suggestion().is_some()),
        output = |result| {
            observation::result(result, |position| {
                Fields::new().flag("outdated", matches!(position, ObservedPosition::Outdated))
            })
        },
    )]
    pub(crate) fn verify_observed_position(
        &self,
        observed: &Value,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<ObservedPosition, GitLabError> {
        let original_position = self.mapped_position(revision, finding, None)?;
        if position::verify_observed(observed, &original_position).is_ok() {
            return Ok(ObservedPosition::Verified {
                range: finding
                    .suggestion()
                    .map(|s| (s.start_line().get(), s.end_line().get())),
            });
        }
        let (tracked_revision, location) = position::tracked_location(observed)?;
        if tracked_revision == *revision {
            return Err(GitLabError::UnsupportedPosition);
        }
        let version = self.diff_version(&tracked_revision)?;
        let file = version.file(location.path())?;
        let original = self.committed_lines(revision, finding.location().path())?;
        let current = self.committed_lines(&tracked_revision, location.path())?;
        let mut tracked_draft = FindingDraft {
            id: finding.id(),
            policy_id: finding.policy_id().clone(),
            title: finding.title().to_owned(),
            body: finding.body().to_owned(),
            severity: finding.severity(),
            disposition: finding.disposition(),
            status: finding.status(),
            evidence: finding.evidence().to_owned(),
            validated: finding.validated(),
            location: location.clone(),
            suggestion: None,
        };
        let anchor =
            Finding::new(tracked_draft.clone()).map_err(|_| GitLabError::UnsupportedPosition)?;
        let expected_anchor = position::build(file, &tracked_revision, &anchor, Some(&current))?;
        position::verify_anchor(observed, &expected_anchor)?;
        let original_anchor = usize::try_from(finding.location().line().get() - 1)
            .map_err(|_| GitLabError::UnsupportedPosition)?;
        let current_anchor = usize::try_from(location.line().get() - 1)
            .map_err(|_| GitLabError::UnsupportedPosition)?;
        let original_line = original
            .get(original_anchor)
            .ok_or(GitLabError::UnsupportedPosition)?;
        if current.get(current_anchor) != Some(original_line) {
            return Ok(ObservedPosition::Outdated);
        }
        let suggestion = if let Some(s) = finding.suggestion() {
            let above = finding.location().line().get() - s.start_line().get();
            let Some(start) = location
                .line()
                .get()
                .checked_sub(above)
                .filter(|line| *line > 0)
            else {
                return Ok(ObservedPosition::Outdated);
            };
            Some(
                Suggestion::new(
                    location.clone(),
                    tracked_revision.clone(),
                    LineNumber::try_from(start).map_err(|_| GitLabError::UnsupportedPosition)?,
                    s.original().to_vec(),
                    s.replacement().to_vec(),
                    s.evidence().to_owned(),
                )
                .map_err(|_| GitLabError::UnsupportedPosition)?,
            )
        } else {
            None
        };
        if let Some(suggestion) = &suggestion {
            let start = usize::try_from(suggestion.start_line().get() - 1)
                .map_err(|_| GitLabError::UnsupportedPosition)?;
            let end = usize::try_from(suggestion.end_line().get())
                .map_err(|_| GitLabError::UnsupportedPosition)?;
            if current.get(start..end) != Some(suggestion.original()) {
                return Ok(ObservedPosition::Outdated);
            }
        }
        // GitLab tracks surviving positions; retained finding evidence stays at creation.
        tracked_draft.suggestion = suggestion;
        let tracked = Finding::new(tracked_draft).map_err(|_| GitLabError::UnsupportedPosition)?;
        let expected = position::build(file, &tracked_revision, &tracked, Some(&current))?;
        position::verify_tracked(observed, &expected, &original_position)?;
        Ok(ObservedPosition::Verified {
            range: tracked
                .suggestion()
                .map(|s| (s.start_line().get(), s.end_line().get())),
        })
    }
}

#[review_kit_observation::operations]
impl CommittedSource for GitLab {
    #[operation(
        name = "gitlab.source_lines",
        input = Fields::new().secret("path", path.as_str()),
        output = |result| {
            observation::port_result(result, |lines| Fields::new().count("lines", lines.len()))
        },
    )]
    fn lines(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        self.check_subject(subject)
            .and_then(|()| self.committed_lines(revision, path))
            .map_err(|error| PortError::new("read committed GitLab source", error))
    }

    #[operation(
        name = "gitlab.check_position",
        input = Fields::new(),
        output = |result| observation::port_result(result, |_| Fields::new()),
    )]
    fn validate_position(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<(), PortError> {
        self.check_subject(subject)
            .and_then(|()| self.position(revision, finding).map(|_| ()))
            .map_err(|error| PortError::new("validate GitLab diff position", error))
    }
}

#[review_kit_observation::operations]
impl ReviewChanges for GitLab {
    #[operation(
        name = "gitlab.read_changes",
        input = Fields::new(),
        output = |result| {
            observation::port_result(result, |changes| {
                Fields::new()
                    .count("files", changes.files.len())
                    .count("diff_bytes", changes.unified_diff.len())
            })
        },
    )]
    fn read_changes(
        &mut self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<ObservedChanges, PortError> {
        let read = || {
            self.check_subject(subject)?;
            self.require_revision(revision)?;
            let version = self.diff_version(revision)?;
            let files = version
                .diffs
                .iter()
                .map(DiffFile::changed_file)
                .collect::<Result<Vec<_>, _>>()?;
            let unified_diff = version.unified_diff()?;
            self.require_revision(revision)?;
            Ok(ObservedChanges {
                subject: subject.clone(),
                revision: revision.clone(),
                files,
                unified_diff,
            })
        };
        read().map_err(|error: GitLabError| PortError::new("read GitLab changes", error))
    }
}

#[review_kit_observation::operations]
#[helper]
fn text_lines(bytes: Vec<u8>) -> Result<Vec<String>, GitLabError> {
    let text = String::from_utf8(bytes).map_err(|_| GitLabError::UnsupportedSource)?;
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(GitLabError::UnsupportedSource);
    }
    Ok(text.split_terminator('\n').map(str::to_owned).collect())
}

#[review_kit_observation::operations]
#[helper]
fn suggestion_contents(
    text: &str,
    start: u64,
    end: u64,
    replacement: &[String],
) -> Result<(String, String), GitLabError> {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let start = usize::try_from(start - 1).map_err(|_| GitLabError::UnsupportedPosition)?;
    let end = usize::try_from(end).map_err(|_| GitLabError::UnsupportedPosition)?;
    let from = lines
        .get(start..end)
        .ok_or(GitLabError::UnsupportedPosition)?
        .concat();
    let last = lines.get(end - 1).ok_or(GitLabError::UnsupportedPosition)?;
    let ending = last.find(['\r', '\n']).map_or("", |index| {
        if last[index..].starts_with("\r\n") {
            "\r\n"
        } else if last[index..].starts_with('\r') {
            "\r"
        } else {
            "\n"
        }
    });
    let replacement = replacement.join("\n");
    let to = if replacement.is_empty() {
        String::new()
    } else {
        format!("{replacement}{ending}")
    };
    Ok((from, to))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_content_matches_gitlab_raw_range_and_last_line_break() -> Result<(), GitLabError>
    {
        for (source, start, end, replacement, from, to) in [
            ("one\nlast\n", 2, 2, vec!["new".into()], "last\n", "new\n"),
            (
                "one\nlast\r\n",
                2,
                2,
                vec!["new".into()],
                "last\r\n",
                "new\r\n",
            ),
            ("one\nlast", 2, 2, vec!["new".into()], "last", "new"),
            ("one\nlast\r", 2, 2, vec!["new".into()], "last\r", "new\r"),
            (
                "last\rinside\n",
                1,
                1,
                vec!["new".into()],
                "last\rinside\n",
                "new\r",
            ),
            (
                "one\n\n",
                1,
                2,
                vec!["new".into(), "".into()],
                "one\n\n",
                "new\n\n",
            ),
            ("last\n", 1, 1, Vec::new(), "last\n", ""),
        ] {
            assert_eq!(
                suggestion_contents(source, start, end, &replacement)?,
                (from.into(), to.into())
            );
        }
        Ok(())
    }

    #[test]
    fn committed_text_preserves_carriage_returns_and_terminal_line_semantics() {
        assert_eq!(
            text_lines(b"first\r\n\nthird\r".to_vec()).unwrap(),
            ["first\r", "", "third\r"]
        );
        assert!(text_lines(Vec::new()).unwrap().is_empty());
        assert_eq!(text_lines(b"\n".to_vec()).unwrap(), [""]);
        assert_eq!(text_lines(b"first\n".to_vec()).unwrap(), ["first"]);
    }

    #[test]
    fn binary_and_invalid_utf8_are_unsupported_source() {
        assert!(matches!(
            text_lines(vec![0xff]),
            Err(GitLabError::UnsupportedSource)
        ));
        assert!(matches!(
            text_lines(b"one\0two".to_vec()),
            Err(GitLabError::UnsupportedSource)
        ));
        assert!(matches!(
            text_lines(b"one\x01two".to_vec()),
            Err(GitLabError::UnsupportedSource)
        ));
    }
}
