use review_kit_application::{
    review_run::{ContentRef, RunCompletion, RunId, SnapshotFiles},
    use_cases::checkpoints::CONTEXT_FILE,
};
use review_kit_domain::{ReviewerId, Revision, Subject};
use serde::{Deserialize, Serialize, Serializer, ser::Error as _};

use super::wire::{InvalidDocument, content_ref, nonblank, nullable, optional_ref, run_id};
use crate::domain::{RevisionData, SubjectData};

checkpoint_kind!(RunKind, ReviewRun, "review_run");

/// A run's local lifecycle, written as `status`, `completion` and `reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLifecycle {
    Open,
    /// An abandoned completion requires a reason; other outcomes may record one.
    Completed {
        completion: RunCompletion,
        reason: Option<String>,
    },
    Stopped {
        reason: String,
    },
}

impl RunLifecycle {
    /// Whether the variant satisfies the schema's status, completion and reason rules.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Open => true,
            Self::Completed { completion, reason } => match reason {
                Some(reason) => nonblank(reason),
                None => *completion != RunCompletion::Abandoned,
            },
            Self::Stopped { reason } => nonblank(reason),
        }
    }
}

/// An RFC 3339 UTC timestamp such as `2026-10-13T12:00:00Z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtcTimestamp(String);

impl UtcTimestamp {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UtcTimestamp {
    type Error = InvalidDocument;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let bytes = value.as_bytes();
        let digits = |range: std::ops::Range<usize>, max: u32| {
            bytes.get(range).is_some_and(|part| {
                part.iter().all(u8::is_ascii_digit)
                    && part
                        .iter()
                        .fold(0, |total, digit| total * 10 + u32::from(digit - b'0'))
                        <= max
            })
        };
        let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]
            .iter()
            .all(|(index, separator)| bytes.get(*index) == Some(separator));
        let fraction = match bytes.get(19..bytes.len().saturating_sub(1)) {
            Some([]) => true,
            Some([b'.', rest @ ..]) => !rest.is_empty() && rest.iter().all(u8::is_ascii_digit),
            _ => false,
        };
        let valid = separators
            && digits(0..4, 9999)
            && digits(5..7, 12)
            && digits(8..10, 31)
            && digits(11..13, 23)
            && digits(14..16, 59)
            && digits(17..19, 60)
            && bytes.get(5..7) != Some(b"00")
            && bytes.get(8..10) != Some(b"00")
            && fraction
            && bytes.last() == Some(&b'Z');
        if valid {
            Ok(Self(value))
        } else {
            Err(InvalidDocument::Timestamp)
        }
    }
}

/// `review-run.json`: a run's identity, retained-file hashes, accepted checkpoints and lifecycle.
///
/// Decoding enforces the schema's lifecycle combinations; encoding refuses an invalid lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RunWire")]
pub struct ReviewRunDocument {
    pub run_id: RunId,
    pub subject: Subject,
    pub reviewer: ReviewerId,
    pub revision: Revision,
    pub created_at: UtcTimestamp,
    pub lifecycle: RunLifecycle,
    pub snapshot_files: SnapshotFiles,
    pub review_assessment: Option<ContentRef>,
    /// Set before the first publication binding or dispatch and never cleared.
    pub publication_attempted: bool,
}

impl Serialize for ReviewRunDocument {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.lifecycle.is_valid() {
            return Err(S::Error::custom(InvalidDocument::Lifecycle));
        }
        RunWire::from(self).serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StatusData {
    Open,
    Completed,
    Stopped,
}

wire_enum!(
    CompletionData,
    RunCompletion,
    [Published, NoChanges, LocalOnly, Abandoned]
);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFilesData {
    #[serde(rename = "review-context.json")]
    context: String,
    #[serde(rename = "reviewed-source.json")]
    source: String,
    #[serde(rename = "merge-request.diff")]
    diff: String,
    #[serde(rename = "review-conversation.json")]
    conversation: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunWire {
    version: u64,
    kind: RunKind,
    run_id: String,
    subject: SubjectData,
    reviewer: String,
    revision: RevisionData,
    context: String,
    created_at: String,
    status: StatusData,
    #[serde(deserialize_with = "nullable")]
    completion: Option<CompletionData>,
    #[serde(deserialize_with = "nullable")]
    reason: Option<String>,
    snapshot_files: SnapshotFilesData,
    #[serde(deserialize_with = "nullable")]
    review_assessment: Option<String>,
    publication_attempted: bool,
}

impl TryFrom<RunWire> for ReviewRunDocument {
    type Error = InvalidDocument;

    fn try_from(wire: RunWire) -> Result<Self, Self::Error> {
        if wire.version != 1 {
            return Err(InvalidDocument::Version(wire.version));
        }
        if wire.context != CONTEXT_FILE {
            return Err(InvalidDocument::Context);
        }
        let lifecycle = match (wire.status, wire.completion, wire.reason) {
            (StatusData::Open, None, None) => RunLifecycle::Open,
            (StatusData::Completed, Some(completion), reason) => RunLifecycle::Completed {
                completion: completion.into(),
                reason,
            },
            (StatusData::Stopped, None, Some(reason)) => RunLifecycle::Stopped { reason },
            _ => return Err(InvalidDocument::Lifecycle),
        };
        if !lifecycle.is_valid() {
            return Err(InvalidDocument::Lifecycle);
        }
        Ok(Self {
            run_id: run_id(wire.run_id)?,
            subject: wire
                .subject
                .into_subject()
                .map_err(|_| InvalidDocument::Identity)?,
            reviewer: ReviewerId::try_from(wire.reviewer).map_err(|_| InvalidDocument::Identity)?,
            revision: wire
                .revision
                .into_revision()
                .map_err(|_| InvalidDocument::Identity)?,
            created_at: UtcTimestamp::try_from(wire.created_at)?,
            lifecycle,
            review_assessment: optional_ref(wire.review_assessment)?,
            snapshot_files: SnapshotFiles {
                context: content_ref(wire.snapshot_files.context)?,
                source: content_ref(wire.snapshot_files.source)?,
                diff: content_ref(wire.snapshot_files.diff)?,
                conversation: content_ref(wire.snapshot_files.conversation)?,
            },
            publication_attempted: wire.publication_attempted,
        })
    }
}

impl From<&ReviewRunDocument> for RunWire {
    fn from(document: &ReviewRunDocument) -> Self {
        let (status, completion, reason) = match &document.lifecycle {
            RunLifecycle::Open => (StatusData::Open, None, None),
            RunLifecycle::Completed { completion, reason } => (
                StatusData::Completed,
                Some((*completion).into()),
                reason.clone(),
            ),
            RunLifecycle::Stopped { reason } => (StatusData::Stopped, None, Some(reason.clone())),
        };
        let reference = |value: &Option<review_kit_application::review_run::ContentRef>| {
            value.as_ref().map(|value| value.as_str().to_owned())
        };
        Self {
            version: 1,
            kind: RunKind::ReviewRun,
            run_id: document.run_id.as_str().into(),
            subject: (&document.subject).into(),
            reviewer: document.reviewer.as_str().into(),
            revision: (&document.revision).into(),
            context: CONTEXT_FILE.into(),
            created_at: document.created_at.as_str().into(),
            status,
            completion,
            reason,
            snapshot_files: SnapshotFilesData {
                context: document.snapshot_files.context.as_str().into(),
                source: document.snapshot_files.source.as_str().into(),
                diff: document.snapshot_files.diff.as_str().into(),
                conversation: document.snapshot_files.conversation.as_str().into(),
            },
            review_assessment: reference(&document.review_assessment),
            publication_attempted: document.publication_attempted,
        }
    }
}
