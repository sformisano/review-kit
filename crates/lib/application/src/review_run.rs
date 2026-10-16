//! Identities shared by review runs, their accepted checkpoints and publication state.
//!
//! These values carry no file or clock I/O. Adapters compute content digests and
//! generate run identifiers; this module only validates their textual forms.

use std::fmt;

/// A local review run identifier in lowercase hyphenated UUID form.
///
/// The value identifies retained run state; it does not locate or download it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(String);

impl RunId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for RunId {
    type Error = InvalidRunId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let groups: Vec<&str> = value.split('-').collect();
        let lengths = [8, 4, 4, 4, 12];
        let valid = groups.len() == lengths.len()
            && groups.iter().zip(lengths).all(|(group, length)| {
                group.len() == length
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !valid {
            return Err(InvalidRunId);
        }
        Ok(Self(value))
    }
}

/// The input is not a lowercase hyphenated UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("run ID must be a lowercase hyphenated UUID")]
pub struct InvalidRunId;

/// A reference to exact retained bytes: `sha256:` followed by 64 lowercase hex digits.
///
/// Equal references identify equal bytes. No canonical serialization is implied.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentRef(String);

impl ContentRef {
    const PREFIX: &'static str = "sha256:";

    /// Build a reference from a lowercase SHA-256 hex digest.
    pub fn from_hex(hex: &str) -> Result<Self, InvalidContentRef> {
        Self::try_from(format!("{}{hex}", Self::PREFIX))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 64-digit hex digest without the algorithm prefix.
    pub fn hex(&self) -> &str {
        &self.0[Self::PREFIX.len()..]
    }
}

impl fmt::Display for ContentRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ContentRef {
    type Error = InvalidContentRef;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.strip_prefix(Self::PREFIX).is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if !valid {
            return Err(InvalidContentRef);
        }
        Ok(Self(value))
    }
}

/// The input is not `sha256:` followed by 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("content reference must be sha256: followed by 64 lowercase hex digits")]
pub struct InvalidContentRef;

/// Hashes of the four immutable files captured when a run starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotFiles {
    pub context: ContentRef,
    pub source: ContentRef,
    pub diff: ContentRef,
    pub conversation: ContentRef,
}

impl SnapshotFiles {
    pub const CONTEXT: &'static str = "review-context.json";
    pub const SOURCE: &'static str = "reviewed-source.json";
    pub const DIFF: &'static str = "merge-request.diff";
    pub const CONVERSATION: &'static str = "review-conversation.json";
}

/// A verified publication outcome retained by the shared journal for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettledOutcome {
    Published,
    NoChanges,
    Abandoned,
}

/// How a completed run ended locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunCompletion {
    Published,
    NoChanges,
    LocalOnly,
    Abandoned,
}

impl From<SettledOutcome> for RunCompletion {
    fn from(outcome: SettledOutcome) -> Self {
        match outcome {
            SettledOutcome::Published => Self::Published,
            SettledOutcome::NoChanges => Self::NoChanges,
            SettledOutcome::Abandoned => Self::Abandoned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentRef, InvalidContentRef, InvalidRunId, RunId};

    #[test]
    fn run_ids_accept_only_lowercase_hyphenated_uuids() {
        assert!(RunId::try_from("00000000-0000-4000-8000-000000000001".to_owned()).is_ok());
        for raw in [
            "",
            "00000000-0000-4000-8000-00000000000",
            "00000000-0000-4000-8000-0000000000001",
            "00000000000040008000000000000001",
            "00000000-0000-4000-8000-00000000000A",
            "../00000000-0000-4000-8000-000000000001",
        ] {
            assert_eq!(RunId::try_from(raw.to_owned()), Err(InvalidRunId), "{raw}");
        }
    }

    #[test]
    fn content_refs_require_the_sha256_prefix_and_lowercase_digest() -> Result<(), InvalidContentRef>
    {
        let hex = "a".repeat(64);
        let reference = ContentRef::from_hex(&hex)?;
        assert_eq!(reference.as_str(), format!("sha256:{hex}"));
        assert_eq!(reference.hex(), hex);
        for raw in [
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "a".repeat(63)),
            format!("sha1:{hex}"),
            hex.clone(),
        ] {
            assert_eq!(ContentRef::try_from(raw), Err(InvalidContentRef));
        }
        Ok(())
    }
}
