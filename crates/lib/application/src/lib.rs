//! Turn agent-authored reviews into checked assessments and verified communication.
//!
//! Use cases gather existing review context, prepare unfilled drafts, check the
//! completed record, and coordinate permitted publication effects. Review
//! decisions remain the agent's input; successful publication describes what was
//! communicated, independently of whether the change is ready.
//!
//! Calls are synchronous. A host schedules this layer on a worker when its runtime
//! cannot block. Publication binds a provider session and caller-owned progress.

pub mod conversation;
mod observation;
mod policy_declarations;
pub mod ports;
pub mod publication;
pub mod review_run;
pub mod summary;
pub mod use_cases;
