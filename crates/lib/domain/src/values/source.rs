use crate::{LineNumber, Revision};

/// A slash-separated repository-relative path with exact spelling.
///
/// Empty, `.` and `..` components, backslashes, control characters, and a colon
/// in the second byte are rejected.
/// Spaces and Unicode are retained, including whitespace-only components.
/// Construction does not establish filesystem containment or source existence.
/// Accepted paths need not be valid on every host filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourcePath(String);

impl SourcePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SourcePath {
    type Error = SourceError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let drive_prefix = value.as_bytes().get(1) == Some(&b':');
        if value.is_empty()
            || drive_prefix
            || value.contains('\\')
            || value.chars().any(char::is_control)
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(SourceError::UnsafePath);
        }
        Ok(Self(value))
    }
}

/// A repository path and one-based source anchor.
///
/// It locates a finding's concern and anchors any associated suggestion.
///
/// The component types enforce their own shape.
/// The pair does not establish a valid provider diff position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceLocation {
    path: SourcePath,
    line: LineNumber,
}

impl SourceLocation {
    pub fn new(path: SourcePath, line: LineNumber) -> Self {
        Self { path, line }
    }
    pub fn path(&self) -> &SourcePath {
        &self.path
    }
    pub fn line(&self) -> LineNumber {
        self.line
    }
}

/// A replacement over inclusive committed lines, with an anchor inside that range.
///
/// It records a proposed correction alongside the exact source it replaces,
/// so callers can check the proposal against the reviewed committed object.
///
/// Each string represents one line without an LF (`\n`) separator.
/// Carriage returns (`\r`), whitespace, and empty line strings are retained
/// exactly.
/// An empty replacement deletes the range; an empty original range is invalid.
/// Correction evidence must be nonblank.
/// Construction does not prove the correction is valid or publishable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    location: SourceLocation,
    revision: Revision,
    start_line: LineNumber,
    end_line: LineNumber,
    original: Vec<String>,
    replacement: Vec<String>,
    evidence: String,
}

impl Suggestion {
    /// Derives the inclusive end line from `start_line` and the original line
    /// count.
    ///
    /// The original range is supplied text; construction does not read the
    /// reviewed revision.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] for an invalid range, an out-of-range anchor,
    /// an embedded LF, or blank correction evidence.
    pub fn new(
        location: SourceLocation,
        revision: Revision,
        start_line: LineNumber,
        original: Vec<String>,
        replacement: Vec<String>,
        evidence: String,
    ) -> Result<Self, SourceError> {
        let length = u64::try_from(original.len()).map_err(|_| SourceError::RangeOverflow)?;
        let additional = length.checked_sub(1).ok_or(SourceError::EmptyOriginal)?;
        let end = start_line
            .get()
            .checked_add(additional)
            .ok_or(SourceError::RangeOverflow)?;
        let end_line = LineNumber::try_from(end).map_err(|_| SourceError::RangeOverflow)?;
        if location.line() < start_line || location.line() > end_line {
            return Err(SourceError::AnchorOutsideRange);
        }
        if original
            .iter()
            .chain(&replacement)
            .any(|line| line.contains('\n'))
        {
            return Err(SourceError::EmbeddedNewline);
        }
        if evidence.trim().is_empty() {
            return Err(SourceError::MissingEvidence);
        }
        Ok(Self {
            location,
            revision,
            start_line,
            end_line,
            original,
            replacement,
            evidence,
        })
    }
    pub fn location(&self) -> &SourceLocation {
        &self.location
    }
    pub fn revision(&self) -> &Revision {
        &self.revision
    }
    pub fn start_line(&self) -> LineNumber {
        self.start_line
    }
    pub fn end_line(&self) -> LineNumber {
        self.end_line
    }
    pub fn original(&self) -> &[String] {
        &self.original
    }
    pub fn replacement(&self) -> &[String] {
        &self.replacement
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }

    /// Compares the supplied revision, path, and lines exactly, without reading
    /// source.
    /// A match confirms the proposal's original text against the caller's observation;
    /// it does not establish that the replacement corrects the finding.
    ///
    /// The caller supplies the inclusive original range from the requested
    /// committed object.
    /// Whitespace and retained carriage returns participate in the comparison.
    ///
    /// # Errors
    ///
    /// Returns the first mismatch in revision, path, then original lines.
    pub fn verify_original(
        &self,
        revision: &Revision,
        path: &SourcePath,
        lines: &[String],
    ) -> Result<(), SourceError> {
        if revision != &self.revision {
            return Err(SourceError::RevisionMismatch);
        }
        if path != self.location.path() {
            return Err(SourceError::PathMismatch);
        }
        if lines != self.original {
            return Err(SourceError::OriginalMismatch);
        }
        Ok(())
    }
}

/// A source value fails construction or disagrees with supplied committed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum SourceError {
    /// The path fails [`SourcePath`]'s syntax checks.
    #[displaydoc("source path must be a portable repository-relative path")]
    UnsafePath,
    /// A suggestion has no original lines to replace.
    #[displaydoc("suggestion must replace at least one original line")]
    EmptyOriginal,
    /// The original line count or inclusive end exceeds the `u64` coordinate
    /// range.
    #[displaydoc("suggestion line range exceeds supported coordinates")]
    RangeOverflow,
    /// The source anchor falls outside the inclusive original range.
    #[displaydoc("suggestion anchor must be inside its original range")]
    AnchorOutsideRange,
    /// An original or replacement line contains LF (`\n`).
    #[displaydoc("suggestion lines must exclude newline separators")]
    EmbeddedNewline,
    /// Correction evidence is empty after [`str::trim`].
    #[displaydoc("suggestion requires correction evidence")]
    MissingEvidence,
    /// The supplied revision differs in head or ordered comparison references.
    #[displaydoc("committed source revision differs from suggestion revision")]
    RevisionMismatch,
    /// The supplied path differs from the suggestion's exact path spelling.
    #[displaydoc("committed source path differs from suggestion path")]
    PathMismatch,
    /// The supplied lines differ in count or exact contents.
    #[displaydoc("committed lines differ from suggestion original lines")]
    OriginalMismatch,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn portable_paths_reject_traversal_and_preserve_legal_spelling() -> TestResult {
        for path in [
            "", "/a", "../a", "a/../b", "./a", "a//b", "a/", "C:/a", "a\\b", "a\0b", "a\nb",
        ] {
            assert_eq!(
                SourcePath::try_from(path.to_owned()),
                Err(SourceError::UnsafePath)
            );
        }
        assert_eq!(
            SourcePath::try_from("src/é file.rs".to_owned())?.as_str(),
            "src/é file.rs"
        );
        Ok(())
    }

    #[test]
    fn multiline_suggestion_preserves_exact_source_and_allows_deletion() -> TestResult {
        let path = SourcePath::try_from("src/lib.rs".to_owned())?;
        let revision = Revision::new("head".into(), vec![])?;
        let original = vec!["  first".to_owned(), "second\r".to_owned()];
        let suggestion = Suggestion::new(
            SourceLocation::new(path.clone(), LineNumber::try_from(3)?),
            revision.clone(),
            LineNumber::try_from(2)?,
            original.clone(),
            vec![],
            "redundant branches".into(),
        )?;
        assert_eq!(suggestion.end_line().get(), 3);
        assert_eq!(
            suggestion.verify_original(&revision, &path, &original),
            Ok(())
        );
        assert_eq!(
            suggestion.verify_original(&revision, &path, &["first".into(), "second".into()]),
            Err(SourceError::OriginalMismatch)
        );
        let other = Revision::new("other".into(), vec![])?;
        assert_eq!(
            suggestion.verify_original(&other, &path, &original),
            Err(SourceError::RevisionMismatch)
        );
        Ok(())
    }

    #[test]
    fn suggestion_rejects_invalid_ranges_and_payloads() -> TestResult {
        let location = SourceLocation::new(
            SourcePath::try_from("a".to_owned())?,
            LineNumber::try_from(2)?,
        );
        let revision = Revision::new("head".into(), vec![])?;
        let make = |start, original, replacement, evidence| {
            Suggestion::new(
                location.clone(),
                revision.clone(),
                start,
                original,
                replacement,
                evidence,
            )
        };
        assert_eq!(
            make(LineNumber::try_from(2)?, vec![], vec![], "proof".into()),
            Err(SourceError::EmptyOriginal)
        );
        assert_eq!(
            make(
                LineNumber::try_from(u64::MAX)?,
                vec!["a".into(), "b".into()],
                vec![],
                "proof".into()
            ),
            Err(SourceError::RangeOverflow)
        );
        assert_eq!(
            make(
                LineNumber::try_from(3)?,
                vec!["a".into()],
                vec![],
                "proof".into()
            ),
            Err(SourceError::AnchorOutsideRange)
        );
        assert_eq!(
            make(
                LineNumber::try_from(2)?,
                vec!["a\nb".into()],
                vec![],
                "proof".into()
            ),
            Err(SourceError::EmbeddedNewline)
        );
        assert_eq!(
            make(
                LineNumber::try_from(2)?,
                vec!["a".into()],
                vec![],
                " ".into()
            ),
            Err(SourceError::MissingEvidence)
        );
        Ok(())
    }
}
