use redactable::NotSensitive;
use serde::Serialize;

use crate::{ErrorInfo, Fields};

#[derive(Debug, Clone, Copy, Serialize, NotSensitive)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Failure,
    Rejected,
    Uncertain,
    Partial,
    Blocked,
    Panicked,
    Interrupted,
    /// The operation returned, but its logging projection failed.
    Unclassified,
}

/// Observation facts borrow no business value and never determine its return value.
pub struct Completion {
    pub(crate) outcome: Outcome,
    pub(crate) output: Option<Fields>,
    pub(crate) error: Option<ErrorInfo>,
}

impl Completion {
    pub fn new(outcome: Outcome, output: Fields) -> Self {
        Self {
            outcome,
            output: Some(output),
            error: None,
        }
    }

    pub fn success(output: Fields) -> Self {
        Self::new(Outcome::Success, output)
    }

    pub fn failure(error: ErrorInfo) -> Self {
        Self {
            outcome: Outcome::Failure,
            output: None,
            error: Some(error),
        }
    }

    pub fn with_error(mut self, error: ErrorInfo) -> Self {
        self.error = Some(error);
        self
    }

    pub fn result<T, E>(
        result: &Result<T, E>,
        output: impl FnOnce(&T) -> Fields,
        error: impl FnOnce(&E) -> ErrorInfo,
    ) -> Self {
        match result {
            Ok(value) => Self::success(output(value)),
            Err(value) => Self::failure(error(value)),
        }
    }
}
