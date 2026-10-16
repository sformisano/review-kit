//! Crash points for restart tests, compiled only with the `fault-injection` feature.
//!
//! `RKC_FAULT=<point>[:<n>]` aborts the process at the named point. Counted
//! publication points abort at their n-th occurrence in this process (default 1);
//! other points ignore `n`. Aborting runs no destructors, so stored state is
//! exactly what the process persisted before the point.

use review_kit_application::{
    ports::PortError,
    use_cases::{
        publication_run::{ExecutionEvent, ExecutionEvidence},
        publishing::WritePhase,
    },
};
use review_kit_filesystem_adapter::FileEvidence;

const VARIABLE: &str = "RKC_FAULT";

/// The configured point and occurrence, if any.
#[review_kit_observation::operations]
#[helper]
fn configured() -> Option<(String, usize)> {
    let value = std::env::var(VARIABLE).ok()?;
    match value.split_once(':') {
        Some((point, count)) => Some((point.to_owned(), count.parse().ok()?)),
        None => Some((value, 1)),
    }
}

/// Abort when `point` is the configured uncounted point.
#[review_kit_observation::operations]
#[helper]
pub fn point(point: &str) {
    if configured().is_some_and(|(configured, _)| configured == point) {
        std::process::abort();
    }
}

/// Abort when `point` is configured for its `occurrence`-th event.
#[review_kit_observation::operations]
#[helper]
fn counted(point: &str, occurrence: usize) {
    if configured().is_some_and(|(configured, count)| configured == point && count == occurrence) {
        std::process::abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Previous {
    Other,
    BeforeWrite,
    Readback,
}

/// Execution evidence that aborts around persisted publication events.
///
/// A dispatch checkpoint is the `Progress` record with a dispatch-possible
/// write that immediately follows a `BeforeWrite` record. A verified
/// checkpoint is the `Progress` record that immediately follows a `Readback`.
pub struct FaultEvidence<'run> {
    inner: FileEvidence<'run>,
    previous: Previous,
    intents: usize,
    dispatches: usize,
    readbacks: usize,
    verified: usize,
}

#[review_kit_observation::operations]
impl<'run> FaultEvidence<'run> {
    #[helper]
    pub fn new(inner: FileEvidence<'run>) -> Self {
        Self {
            inner,
            previous: Previous::Other,
            intents: 0,
            dispatches: 0,
            readbacks: 0,
            verified: 0,
        }
    }
}

#[review_kit_observation::operations]
impl ExecutionEvidence for FaultEvidence<'_> {
    #[helper]
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        let (kind, dispatch) = match &event {
            ExecutionEvent::BeforeWrite(_) => (Previous::BeforeWrite, false),
            ExecutionEvent::Readback { .. } => (Previous::Readback, false),
            ExecutionEvent::Progress { progress, .. } => (
                Previous::Other,
                progress
                    .unresolved
                    .as_ref()
                    .is_some_and(|pending| pending.phase == WritePhase::DispatchPossible),
            ),
            _ => (Previous::Other, false),
        };
        let progress = matches!(event, ExecutionEvent::Progress { .. });
        if kind == Previous::Readback {
            self.readbacks += 1;
            counted("publish.before_readback_record", self.readbacks);
        }
        self.inner.record(event)?;
        let previous = std::mem::replace(&mut self.previous, kind);
        if kind == Previous::BeforeWrite {
            self.intents += 1;
            counted("publish.after_intent", self.intents);
        } else if progress && dispatch && previous == Previous::BeforeWrite {
            self.dispatches += 1;
            counted("publish.after_dispatch_flag", self.dispatches);
        } else if progress && previous == Previous::Readback {
            self.verified += 1;
            counted("publish.after_verified", self.verified);
        }
        Ok(())
    }
}
