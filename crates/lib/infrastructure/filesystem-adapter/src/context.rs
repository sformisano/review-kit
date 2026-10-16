use crate::observation::{file_error, port_error};
use review_kit_observation::{Completion, Fields};

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::Snapshot,
    use_cases::discovery_bundle::{ObservedChanges, ReviewChanges},
};
use review_kit_domain::{Revision, Subject};
use review_kit_json_adapter::{ContextDocument, JsonError};

use crate::FileError;

#[derive(Clone)]
pub struct SavedContext {
    snapshot: Snapshot,
    changes: ObservedChanges,
}

#[review_kit_observation::operations]
impl SavedContext {
    #[operation(
        name = "filesystem.restore_context",
        input = Fields::new().count("files", document.changes.files.len()),
        output = |result| Completion::result(result, |_| Fields::new(), file_error),
    )]
    pub fn new(document: &ContextDocument) -> Result<Self, FileError> {
        let bundle = document.bundle()?;
        Ok(Self {
            snapshot: bundle.snapshot().clone(),
            changes: bundle.changes().clone(),
        })
    }
}

#[review_kit_observation::operations]
impl ReviewReader for SavedContext {
    #[operation(
        name = "filesystem.discover_saved_context",
        input = Fields::new(),
        output = |result| {
            Completion::result(
                result,
                |snapshot| {
                    Fields::new()
                        .count("summaries", snapshot.summaries.len())
                        .count("findings", snapshot.findings.len())
                        .count("replies", snapshot.replies.len())
                },
                port_error,
            )
        },
    )]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
        if subject != &self.snapshot.subject {
            return Err(PortError::new(
                "read saved review context",
                JsonError::ContextBinding,
            ));
        }
        Ok(self.snapshot.clone())
    }
}

#[review_kit_observation::operations]
impl ReviewChanges for SavedContext {
    #[operation(
        name = "filesystem.read_saved_changes",
        input = Fields::new(),
        output = |result| {
            Completion::result(
                result,
                |changes| {
                    Fields::new()
                        .count("files", changes.files.len())
                        .count("diff_bytes", changes.unified_diff.len())
                },
                port_error,
            )
        },
    )]
    fn read_changes(
        &mut self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<ObservedChanges, PortError> {
        if subject != &self.changes.subject || revision != &self.changes.revision {
            return Err(PortError::new(
                "read saved review changes",
                JsonError::ContextBinding,
            ));
        }
        Ok(self.changes.clone())
    }
}
