#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Workspace locks are owned by the open file description. A child process
// started by a concurrent test holds a copy of every descriptor until it
// executes its program, so these tests run in their own binary, one at a time.

#[path = "runs/harness.rs"]
#[allow(dead_code, reason = "the harness is shared with the run storage tests")]
mod harness;

#[path = "runs/locking.rs"]
mod locking;
