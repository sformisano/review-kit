//! A reviewer's conclusion for one revision and concerns retained across follow-ups.
//!
//! The agent declares coverage; assessments combine supplied policy judgments
//! and findings. Construction checks consistency without deciding review correctness.
//! Source observations and publication belong to callers.

mod entities;
mod values;

pub use entities::{Finding, FindingDraft, FindingError};
pub use values::{
    Assessment, AssessmentDraft, AssessmentError, BlankPolicyId, Disposition, FindingId,
    FindingIdExhausted, FindingStatus, FindingTransition, IdentityError, InvalidFindingId,
    InvalidLineNumber, LineNumber, PolicyEvaluation, PolicyEvaluationError, PolicyId,
    PolicyJudgment, ReviewDecision, ReviewerId, Revision, Severity, SourceError, SourceLocation,
    SourcePath, Subject, Suggestion,
};
