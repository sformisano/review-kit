//! Synchronous, scoped operation logs with explicit redacted projections.

mod completion;
mod fields;
mod scope;

pub use completion::{Completion, Outcome};
pub use fields::{ErrorInfo, Fields};
pub use review_kit_observation_macros::{observed_port, operations};
pub use scope::with_observer;

/// Runtime support for generated operation declarations.
#[doc(hidden)]
pub mod __private {
    use crate::{Fields, Projection, isolate_logging, scope};

    pub use crate::observe as execute;

    pub fn project_input(project: impl FnOnce() -> Fields) -> Fields {
        let mut project = Projection(Some(project));
        if scope::current().is_none() {
            return Fields::new();
        }
        isolate_logging(|| project.0.take().map(|project| project()))
            .flatten()
            .unwrap_or_else(|| Fields::new().label("observation_input", "projection_failed"))
    }

    pub fn map_value<T, U>(value: T, map: impl FnOnce(T) -> U) -> U {
        map(value)
    }
}

use std::{
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::Instant,
};

use redactable::{Sensitive, slog::SlogRedactedExt};
use serde::Serialize;
use slog::Drain;
use uuid::Uuid;

fn isolate_logging<T>(work: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(value) => Some(value),
        Err(payload) => {
            // An arbitrary panic payload can panic again in Drop, including during unwinding.
            std::mem::forget(payload);
            None
        }
    }
}

struct Projection<F>(Option<F>);

impl<F> Drop for Projection<F> {
    fn drop(&mut self) {
        // Unused projection captures can run arbitrary destructors, even without a sink.
        let _ = isolate_logging(|| drop(self.0.take()));
    }
}

/// A synchronous JSON sink. I/O errors are discarded; no raw fallback is printed.
/// A writer must return from writes and flushes and must not log recursively.
#[derive(Clone)]
pub struct Observer {
    logger: Option<slog::Logger>,
    listener: Option<Arc<dyn Fn(OperationEvent) + Send + Sync>>,
}

/// Fixed operation metadata without inputs, outputs, identities or error payloads.
#[derive(Clone, Copy, Debug)]
pub enum OperationEvent {
    Started {
        operation: &'static str,
    },
    Completed {
        operation: &'static str,
        outcome: Outcome,
    },
}

impl Drop for Observer {
    fn drop(&mut self) {
        // The last logger owns the sink, whose destructor can also panic.
        let _ = isolate_logging(|| drop(self.logger.take()));
        let _ = isolate_logging(|| drop(self.listener.take()));
    }
}

impl Observer {
    pub fn json(writer: impl Write + Send + 'static) -> Self {
        let drain = slog_json::Json::new(writer)
            .add_default_keys()
            .set_newlines(true)
            .set_flush(true)
            .build();
        Self {
            logger: Some(slog::Logger::root(
                Mutex::new(drain).ignore_res(),
                slog::o!(),
            )),
            listener: None,
        }
    }

    /// Runs before sink I/O. The listener must return promptly and avoid recursive logging.
    /// Listener panics are contained and cannot replace operation results.
    pub fn with_listener(
        mut self,
        listener: impl Fn(OperationEvent) + Send + Sync + 'static,
    ) -> Self {
        let _ = isolate_logging(|| drop(self.listener.take()));
        self.listener = Some(Arc::new(listener));
        self
    }

    fn emit(&self, record: &Record) {
        if let Some(listener) = &self.listener {
            scope::emitting(|| {
                let _ = isolate_logging(|| {
                    listener(match record.outcome {
                        Some(outcome) => OperationEvent::Completed {
                            operation: record.operation,
                            outcome,
                        },
                        None => OperationEvent::Started {
                            operation: record.operation,
                        },
                    })
                });
            });
        }
        self.emit_projected(record, record.outcome);
    }

    fn emit_projected(&self, record: &impl redactable::ToRedacted, outcome: Option<Outcome>) {
        let Some(logger) = &self.logger else {
            return;
        };
        scope::emitting(|| {
            // Projection and writer failures cannot replace a business result.
            let _ = isolate_logging(|| {
                let value = record.slog_redacted_json();
                match outcome {
                    Some(Outcome::Failure | Outcome::Panicked | Outcome::Uncertain) => {
                        slog::error!(logger, "Operation completed"; "execution" => value);
                    }
                    Some(
                        Outcome::Partial
                        | Outcome::Rejected
                        | Outcome::Interrupted
                        | Outcome::Unclassified,
                    ) => {
                        slog::warn!(logger, "Operation completed"; "execution" => value);
                    }
                    _ => slog::info!(logger, "Operation"; "execution" => value),
                }
            });
        });
    }
}

#[cfg(test)]
mod tests;

#[derive(Clone, Serialize, Sensitive)]
struct Record {
    #[not_sensitive]
    version: u8,
    #[not_sensitive]
    operation: &'static str,
    #[not_sensitive]
    phase: &'static str,
    #[not_sensitive]
    execution_id: String,
    #[not_sensitive]
    parent_execution_id: Option<String>,
    input: Option<Fields>,
    output: Option<Fields>,
    error: Option<ErrorInfo>,
    outcome: Option<Outcome>,
    #[not_sensitive]
    duration_us: Option<u64>,
}

/// Observes one explicit boundary and returns the body's original result.
/// Unwinding restores ancestry and attempts a terminal record without a panic payload.
pub fn observe<T>(
    operation: &'static str,
    input: Fields,
    body: impl FnOnce() -> T,
    classify: impl FnOnce(&T) -> Completion,
) -> T {
    let mut classify = Projection(Some(classify));
    let Some((observer, parent)) = scope::current() else {
        return body();
    };
    let Some(id) = isolate_logging(Uuid::new_v4) else {
        return body();
    };
    let started = Instant::now();
    let record = Record {
        version: 1,
        operation,
        phase: "started",
        execution_id: id.to_string(),
        parent_execution_id: parent.map(|id| id.to_string()),
        input: Some(input),
        output: None,
        error: None,
        outcome: None,
        duration_us: None,
    };
    let _ancestry = scope::enter(id);
    let mut guard = Guard {
        observer,
        record,
        started,
        armed: true,
    };
    guard.observer.emit(&guard.record);
    let result = body();
    let completion = isolate_logging(|| classify.0.take().map(|classify| classify(&result)))
        .flatten()
        .unwrap_or_else(|| {
            Completion::new(Outcome::Unclassified, Fields::new())
                .with_error(ErrorInfo::new("observation", "projection_failed"))
        });
    guard.complete(completion);
    result
}

struct Guard {
    observer: Observer,
    record: Record,
    started: Instant,
    armed: bool,
}

impl Guard {
    fn complete(&mut self, completion: Completion) {
        self.armed = false;
        self.record.phase = "completed";
        self.record.input = None;
        self.record.output = completion.output;
        self.record.error = completion.error;
        self.record.outcome = Some(completion.outcome);
        self.record.duration_us =
            Some(self.started.elapsed().as_micros().min(u64::MAX as u128) as u64);
        self.observer.emit(&self.record);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.armed {
            self.complete(Completion::new(
                if std::thread::panicking() {
                    Outcome::Panicked
                } else {
                    Outcome::Interrupted
                },
                Fields::new(),
            ));
        }
    }
}
