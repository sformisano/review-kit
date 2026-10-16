//! Crash points of a `fault-injection` build, each followed by a fresh process.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "The shared HTTP fixture also exports adapter-only model helpers."
)]
mod support;

use support as server;

#[path = "runs/harness.rs"]
mod harness;

#[path = "faults/answers.rs"]
mod answers;
#[path = "faults/assessment.rs"]
mod assessment;
#[path = "faults/owner.rs"]
mod owner;
#[path = "faults/publication.rs"]
mod publication;
#[path = "faults/transfer.rs"]
mod transfer;

use std::os::unix::process::ExitStatusExt;

use harness::Harness;

/// Run `args` with `RKC_FAULT=fault` and require that the process aborted there.
fn crash(harness: &Harness, fault: &str, args: &[&str]) {
    let (output, value) = harness.rkc_env(&harness.repo, args, &[("RKC_FAULT", fault)]);
    assert_eq!(
        output.status.signal(),
        Some(6),
        "{fault} did not abort {args:?}: {value:#} {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
