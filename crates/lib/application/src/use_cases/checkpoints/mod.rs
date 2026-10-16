//! Agent-declared policy coverage for one revision-bound review checkpoint.

mod confirmations;
mod declarations;

pub use confirmations::{
    Confirmation, ConfirmationIssue, ConfirmationResult, admit_findings, requires_confirmation,
    reusable_confirmations,
};
pub use declarations::{
    ApplicabilityStatus, PolicyOutcome, ReviewBinding, ReviewDeclarations, ReviewDeclarationsDraft,
    ReviewIssue, ReviewPolicy, ReviewPolicyDraft, validate_declarations,
};

use crate::review_run::ContentRef;

pub const CONTEXT_FILE: &str = crate::review_run::SnapshotFiles::CONTEXT;

/// Accepted review content together with the manifest reference whose bytes it decodes.
///
/// The caller verifies that the bytes hash to `reference`; validators trust that pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted<T> {
    pub reference: ContentRef,
    pub content: T,
}

/// How a review document's `supersedes` reference is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Successor<'a> {
    /// A draft names the review's current accepted reference, or none before its first acceptance.
    Current(Option<&'a ContentRef>),
    /// An accepted snapshot keeps the reference it was accepted with.
    /// An exact acceptance retry is such a snapshot: its bytes hash to the current reference.
    Retained,
}

/// Result of applying a [`Successor`] rule to a declared `supersedes` value.
pub(crate) enum SuccessorCheck {
    Valid,
    Stale(Option<ContentRef>),
    Invalid,
}

#[review_kit_observation::operations]
impl Successor<'_> {
    #[helper]
    pub(crate) fn check(self, actual: Option<&str>) -> SuccessorCheck {
        match self {
            Self::Current(expected) if actual == expected.map(ContentRef::as_str) => {
                SuccessorCheck::Valid
            }
            Self::Current(expected) => SuccessorCheck::Stale(expected.cloned()),
            Self::Retained => match actual.map(|value| ContentRef::try_from(value.to_owned())) {
                None => SuccessorCheck::Valid,
                Some(Ok(_)) => SuccessorCheck::Valid,
                Some(Err(_)) => SuccessorCheck::Invalid,
            },
        }
    }
}
