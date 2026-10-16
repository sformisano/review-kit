use crate::observation::{path_fields, port_error};
use review_kit_observation::{Completion, Fields};

use std::path::PathBuf;

use review_kit_application::{
    ports::PortError,
    use_cases::initialization::{AssessmentDraftWriter, DraftCreation, InitializedAssessment},
};
use review_kit_json_adapter::DraftDocument;

use crate::Workspace;

pub struct FileDraftWriter {
    workspace: Workspace,
    path: PathBuf,
    context: String,
}

#[review_kit_observation::operations]
impl FileDraftWriter {
    #[helper]
    pub(crate) fn new(workspace: Workspace, path: PathBuf, context: String) -> Self {
        Self {
            workspace,
            path,
            context,
        }
    }
}

#[review_kit_observation::operations]
impl AssessmentDraftWriter for FileDraftWriter {
    #[operation(
        name = "filesystem.create_draft",
        input = path_fields(&self.path),
        output = |result| {
            Completion::result(
                result,
                |creation| {
                    Fields::new().flag("created", matches!(creation, DraftCreation::Created))
                },
                port_error,
            )
        },
    )]
    fn create_new(&mut self, draft: &InitializedAssessment) -> Result<DraftCreation, PortError> {
        let document = DraftDocument::from_initialized(draft, &self.context);
        document
            .validate_reference()
            .map_err(|error| PortError::new("create assessment draft", error))?;
        self.workspace
            .create_json(&self.path, &document)
            .map(|created| {
                if created {
                    DraftCreation::Created
                } else {
                    DraftCreation::Existing
                }
            })
            .map_err(|error| PortError::new("create assessment draft", error))
    }
}
