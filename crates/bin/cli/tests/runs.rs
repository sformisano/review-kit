//! Review-run commands against real temporary Git worktrees and controlled HTTP GitLab.
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

#[path = "runs/flow.rs"]
mod flow;

#[path = "runs/assessment.rs"]
mod assessment;

#[path = "runs/confirmations.rs"]
mod confirmations;

#[path = "runs/environment.rs"]
mod environment;

#[path = "runs/journal.rs"]
mod journal;

#[path = "runs/fresh_state.rs"]
mod fresh_state;

#[path = "runs/proxy.rs"]
mod proxy;

#[path = "runs/requests.rs"]
mod requests;

#[path = "runs/codes.rs"]
mod codes;

#[path = "runs/selection.rs"]
mod selection;

#[path = "runs/settlement.rs"]
mod settlement;

#[path = "runs/damaged.rs"]
mod damaged;

#[path = "runs/result_inspection.rs"]
mod result_inspection;

#[path = "runs/history.rs"]
mod history;

#[path = "runs/effects.rs"]
mod effects;

#[path = "runs/hints.rs"]
mod hints;

#[path = "runs/freshness.rs"]
mod freshness;

#[path = "runs/conversation.rs"]
mod conversation;
