use review_kit_domain::{Finding, LineNumber, Revision, SourceLocation, SourcePath};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};

use super::{
    diff::{DiffFile, regular_mode},
    mapping::{MappedLine, parse},
};
use crate::GitLabError;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Position {
    base_sha: String,
    start_sha: String,
    head_sha: String,
    position_type: String,
    old_path: String,
    new_path: String,
    new_line: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_range: Option<LineRange>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct LineRange {
    start: Endpoint,
    end: Endpoint,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Endpoint {
    line_code: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    new_line: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_line: Option<u64>,
}

pub(super) fn tracked_location(
    observed: &Value,
) -> Result<(Revision, SourceLocation), GitLabError> {
    let observed = observed_anchor(observed)?;
    if observed.position_type != "text" {
        return Err(GitLabError::UnsupportedPosition);
    }
    let revision = Revision::new(
        observed.head_sha,
        vec![observed.base_sha, observed.start_sha],
    )
    .map_err(|_| GitLabError::UnsupportedPosition)?;
    let path =
        SourcePath::try_from(observed.new_path).map_err(|_| GitLabError::UnsupportedPosition)?;
    let line =
        LineNumber::try_from(observed.new_line).map_err(|_| GitLabError::UnsupportedPosition)?;
    Ok((revision, SourceLocation::new(path, line)))
}

fn observed_anchor(observed: &Value) -> Result<Position, GitLabError> {
    let mut anchor = observed.clone();
    // Tracked ranges can span old and new sides after a source edit.
    anchor
        .as_object_mut()
        .ok_or(GitLabError::UnsupportedPosition)?
        .remove("line_range");
    serde_json::from_value(anchor).map_err(|_| GitLabError::UnsupportedPosition)
}

pub(super) fn verify_anchor(observed: &Value, expected: &Value) -> Result<(), GitLabError> {
    if observed_anchor(observed)? == observed_anchor(expected)? {
        Ok(())
    } else {
        Err(GitLabError::UnsupportedPosition)
    }
}

pub(super) fn build(
    file: &DiffFile,
    revision: &Revision,
    finding: &Finding,
    source: Option<&[String]>,
) -> Result<Value, GitLabError> {
    if file.deleted_file
        || file.new_path != finding.location().path().as_str()
        || !regular_mode(&file.b_mode)
        || file.collapsed
        || file.too_large
    {
        return Err(GitLabError::UnsupportedPosition);
    }
    super::diff::validate_revision(revision)?;
    let map = parse(&file.diff)?;
    let anchor_line = finding.location().line().get();
    let anchor = map
        .get(&anchor_line)
        .ok_or(GitLabError::UnsupportedPosition)?;
    verify_source_line(anchor, source)?;
    let mut range = None;
    if let Some(suggestion) = finding.suggestion() {
        if suggestion.revision() != revision {
            return Err(GitLabError::RevisionChanged);
        }
        let start = suggestion.start_line().get();
        let end = suggestion.end_line().get();
        if anchor_line - start > 100 || end - anchor_line > 100 {
            return Err(GitLabError::UnsupportedPosition);
        }
        for (line, expected) in (start..=end).zip(suggestion.original()) {
            let mapped = map.get(&line).ok_or(GitLabError::UnsupportedPosition)?;
            if mapped.hunk != anchor.hunk || &mapped.text != expected {
                return Err(GitLabError::UnsupportedPosition);
            }
            verify_source_line(mapped, source)?;
        }
        if start != end {
            range = Some(LineRange {
                start: endpoint(
                    &file.new_path,
                    map.get(&start).ok_or(GitLabError::UnsupportedPosition)?,
                ),
                end: endpoint(
                    &file.new_path,
                    map.get(&end).ok_or(GitLabError::UnsupportedPosition)?,
                ),
            });
        }
    }
    let position = Position {
        base_sha: revision.comparison()[0].clone(),
        start_sha: revision.comparison()[1].clone(),
        head_sha: revision.head().to_owned(),
        position_type: "text".into(),
        old_path: file.old_path.clone(),
        new_path: file.new_path.clone(),
        new_line: anchor.new_line,
        old_line: anchor.old_line,
        line_range: range,
    };
    serde_json::to_value(position).map_err(|_| GitLabError::Protocol)
}

fn endpoint(path: &str, mapped: &MappedLine) -> Endpoint {
    // GitLab line codes retain both parser cursors, including the old cursor
    // of an added line whose public old_line coordinate is absent.
    let hash = format!("{:x}", Sha1::digest(path.as_bytes()));
    Endpoint {
        line_code: format!("{hash}_{}_{}", mapped.old_position, mapped.new_line),
        kind: Some(
            if mapped.old_line.is_some() {
                "old"
            } else {
                "new"
            }
            .into(),
        ),
        old_line: mapped.old_line,
        new_line: mapped.new_line,
    }
}

fn verify_source_line(mapped: &MappedLine, source: Option<&[String]>) -> Result<(), GitLabError> {
    if let Some(source) = source {
        let index =
            usize::try_from(mapped.new_line - 1).map_err(|_| GitLabError::UnsupportedPosition)?;
        if source.get(index) != Some(&mapped.text) {
            return Err(GitLabError::UnsupportedPosition);
        }
    }
    Ok(())
}

pub(super) fn verify_observed(observed: &Value, expected: &Value) -> Result<(), GitLabError> {
    let observed: Position =
        serde_json::from_value(observed.clone()).map_err(|_| GitLabError::UnsupportedPosition)?;
    let expected: Position =
        serde_json::from_value(expected.clone()).map_err(|_| GitLabError::Protocol)?;
    if observed != expected {
        return Err(GitLabError::UnsupportedPosition);
    }
    Ok(())
}

pub(super) fn verify_tracked(
    observed: &Value,
    tracked: &Value,
    original: &Value,
) -> Result<(), GitLabError> {
    let mut observed: Position =
        serde_json::from_value(observed.clone()).map_err(|_| GitLabError::UnsupportedPosition)?;
    let mut tracked: Position =
        serde_json::from_value(tracked.clone()).map_err(|_| GitLabError::Protocol)?;
    let original: Position =
        serde_json::from_value(original.clone()).map_err(|_| GitLabError::Protocol)?;
    let observed_range = observed.line_range.take();
    let tracked_range = tracked.line_range.take();
    if observed != tracked {
        return Err(GitLabError::UnsupportedPosition);
    }
    // GitLab either preserves the original range or traces its endpoints.
    if observed_range == original.line_range || observed_range == tracked_range {
        return Ok(());
    }
    let traced_range = tracked_range.map(|mut range| {
        for endpoint in [&mut range.start, &mut range.end] {
            if endpoint.old_line.is_some() {
                endpoint.kind = None;
            }
        }
        range
    });
    if observed_range == traced_range {
        Ok(())
    } else {
        Err(GitLabError::UnsupportedPosition)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use review_kit_domain::{
        Disposition, FindingDraft, FindingId, FindingStatus, LineNumber, PolicyId, Severity,
        SourceLocation, SourcePath, Suggestion,
    };
    use serde_json::json;

    use super::*;

    fn revision() -> Revision {
        Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap()
    }

    fn file(diff: &str) -> DiffFile {
        serde_json::from_value(json!({ "old_path": "old.rs", "new_path": "new.rs", "a_mode": "100644", "b_mode": "100644", "new_file": false, "renamed_file": true, "deleted_file": false, "collapsed": false, "too_large": false, "diff": diff })).unwrap()
    }

    fn finding(line: u64, suggestion: Option<(u64, Vec<String>, Vec<String>)>) -> Finding {
        let location = SourceLocation::new(
            SourcePath::try_from("new.rs".to_owned()).unwrap(),
            LineNumber::try_from(line).unwrap(),
        );
        let suggestion = suggestion.map(|(start, original, replacement)| {
            Suggestion::new(
                location.clone(),
                revision(),
                LineNumber::try_from(start).unwrap(),
                original,
                replacement,
                "verified replacement".into(),
            )
            .unwrap()
        });
        Finding::new(FindingDraft {
            id: FindingId::try_from(1).unwrap(),
            policy_id: PolicyId::try_from("policy".to_owned()).unwrap(),
            title: "Keep the value".into(),
            body: "The value is discarded.".into(),
            severity: Severity::P1,
            disposition: Disposition::Advisory,
            status: FindingStatus::Open,
            evidence: "Observed return path".into(),
            validated: true,
            location,
            suggestion,
        })
        .unwrap()
    }

    #[test]
    fn context_and_added_lines_emit_exact_new_side_positions() {
        let file = file("@@ -1,2 +1,3 @@\n same\n+added\n last\n");
        let source = vec!["same".into(), "added".into(), "last".into()];
        let added = build(&file, &revision(), &finding(2, None), Some(&source)).unwrap();
        assert_eq!(added["new_line"], 2);
        assert!(added.get("old_line").is_none());
        assert_eq!(added["old_path"], "old.rs");
        assert_eq!(added["new_path"], "new.rs");
        let context = build(&file, &revision(), &finding(3, None), Some(&source)).unwrap();
        assert_eq!(context["new_line"], 3);
        assert_eq!(context["old_line"], 2);
        assert_eq!(context["base_sha"], "2".repeat(40));
        assert_eq!(context["start_sha"], "3".repeat(40));
        assert_eq!(context["head_sha"], "1".repeat(40));
    }

    #[test]
    fn multiline_ranges_keep_context_type_and_added_line_cursor_code() {
        let file = file("@@ -1,2 +1,3 @@\n same\n+added\n last\n");
        let multiline = finding(
            2,
            Some((
                1,
                vec!["same".into(), "added".into(), "last".into()],
                vec!["replacement".into()],
            )),
        );
        let position = build(&file, &revision(), &multiline, None).unwrap();
        assert_eq!(position["line_range"]["start"]["type"], "old");
        assert_eq!(position["line_range"]["start"]["old_line"], 1);
        assert_eq!(position["line_range"]["end"]["old_line"], 2);
        assert_eq!(position["line_range"]["end"]["new_line"], 3);
        let added_range = finding(2, Some((2, vec!["added".into(), "last".into()], vec![])));
        let position = build(&file, &revision(), &added_range, None).unwrap();
        assert_eq!(position["line_range"]["start"]["type"], "new");
        assert!(position["line_range"]["start"].get("old_line").is_none());
        assert!(
            position["line_range"]["start"]["line_code"]
                .as_str()
                .unwrap()
                .ends_with("_2_2")
        );
    }

    #[test]
    fn single_line_deletion_suggestion_keeps_an_inline_anchor() {
        let file = file("@@ -1 +1 @@\n+new\n-old\n");
        let finding = finding(1, Some((1, vec!["new".into()], vec![])));
        let position = build(&file, &revision(), &finding, Some(&["new".into()])).unwrap();
        assert_eq!(position["new_line"], 1);
        assert!(position.get("line_range").is_none());
    }

    #[test]
    fn missing_changed_source_and_hunk_crossing_suggestions_are_rejected() {
        let file = file("@@ -1 +1 @@\n one\n@@ -2 +2 @@\n two\n");
        assert!(build(&file, &revision(), &finding(3, None), None).is_err());
        assert!(
            build(
                &file,
                &revision(),
                &finding(1, None),
                Some(&["other".into()])
            )
            .is_err()
        );
        let range = finding(1, Some((1, vec!["one".into(), "two".into()], vec![])));
        assert!(build(&file, &revision(), &range, None).is_err());
        let incorrect = finding(1, Some((1, vec!["different".into()], vec![])));
        assert!(build(&file, &revision(), &incorrect, None).is_err());
    }

    #[test]
    fn deletion_only_binary_and_nonregular_positions_are_rejected() {
        for diff in [
            "@@ -1 +0,0 @@\n-old\n",
            "Binary files a/old.rs and b/new.rs differ\n",
        ] {
            assert!(build(&file(diff), &revision(), &finding(1, None), None).is_err());
        }
        for mode in ["120000", "160000", "040000"] {
            let mut nonregular = file("@@ -0,0 +1 @@\n+line\n");
            nonregular.b_mode = mode.into();
            assert!(build(&nonregular, &revision(), &finding(1, None), None).is_err());
        }
    }

    #[test]
    fn observed_null_coordinates_and_metadata_normalize_but_changed_positions_fail() {
        let expected = build(
            &file("@@ -0,0 +1 @@\n+line\n"),
            &revision(),
            &finding(1, None),
            None,
        )
        .unwrap();
        let mut observed = expected.clone();
        observed["old_line"] = Value::Null;
        observed["line_range"] = Value::Null;
        observed["provider_metadata"] = json!({ "id": 42 });
        assert!(verify_observed(&observed, &expected).is_ok());
        observed["new_line"] = json!(2);
        assert!(verify_observed(&observed, &expected).is_err());
        observed["new_line"] = json!(1);
        observed["start_sha"] = json!("4".repeat(40));
        assert!(verify_observed(&observed, &expected).is_err());
    }

    #[test]
    fn suggestions_cannot_exceed_the_documented_offset_bound() {
        let original = vec!["line".to_owned(); 102];
        let diff = format!("@@ -0,0 +1,102 @@\n{}", "+line\n".repeat(102));
        let finding = finding(1, Some((1, original, vec![])));
        assert!(build(&file(&diff), &revision(), &finding, None).is_err());
    }
}
