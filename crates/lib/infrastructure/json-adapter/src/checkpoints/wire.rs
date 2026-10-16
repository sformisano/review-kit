use review_kit_application::review_run::{ContentRef, RunId};
use serde::{Deserialize, Deserializer};

/// Decode a field that must be present and may be `null`.
///
/// Without this, serde treats an absent `Option` field as `null`, while the
/// checkpoint schemas list these fields as required.
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub(crate) fn run_id(value: String) -> Result<RunId, InvalidDocument> {
    RunId::try_from(value).map_err(|_| InvalidDocument::RunId)
}

pub(crate) fn content_ref(value: String) -> Result<ContentRef, InvalidDocument> {
    ContentRef::try_from(value).map_err(|_| InvalidDocument::Reference)
}

pub(crate) fn optional_ref(value: Option<String>) -> Result<Option<ContentRef>, InvalidDocument> {
    value.map(content_ref).transpose()
}

pub(crate) fn nonblank(value: &str) -> bool {
    !value.trim().is_empty()
}

/// A tool-owned checkpoint document violates its schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum InvalidDocument {
    /// unsupported document version {0}
    Version(u64),
    /// run ID must be a lowercase hyphenated UUID
    RunId,
    /// reference must be sha256: followed by 64 lowercase hex digits
    Reference,
    /// context must be review-context.json
    Context,
    /// manifest must be review-run.json
    Manifest,
    /// accepted path must match the review and reference
    Path,
    /// subject, reviewer or revision is invalid
    Identity,
    /// repository binding fields must not be blank
    Repository,
    /// created_at must be an RFC 3339 UTC timestamp
    Timestamp,
    /// status, completion and reason are inconsistent
    Lifecycle,
}
