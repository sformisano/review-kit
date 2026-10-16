use review_kit_application::{
    publication::Effects,
    review_run::ContentRef,
    use_cases::{checkpoints::Successor, publishing::PublishRequest},
};
use review_kit_filesystem_adapter::{RunDir, RunStorageError, WorkingFile, content_ref};
use review_kit_json_adapter::{
    Diagnostic,
    checkpoints::{AssessmentDocument, AssessmentInputs, check_assessment},
};
use serde_json::json;

use super::loading::LoadedRun;
use crate::error::CliError;

pub const DRAFT_FILE: &str = "review-assessment.draft.json";

#[review_kit_observation::operations]
#[helper]
pub fn draft(dir: &RunDir) -> Result<Option<Vec<u8>>, CliError> {
    dir.read_optional_bytes(WorkingFile::Draft.into())
        .map_err(|error| CliError::storage("read assessment draft", error))
}

#[review_kit_observation::operations]
#[helper]
pub fn reference(bytes: &[u8]) -> Result<ContentRef, CliError> {
    content_ref(bytes).map_err(|error| CliError::adapter("hash draft", "filesystem", 7, error))
}

/// Accepted bytes are usable only when they match the manifest's digest.
#[review_kit_observation::operations]
#[helper]
pub fn accepted_bytes(dir: &RunDir, reference: &ContentRef) -> Result<Vec<u8>, CliError> {
    dir.read_accepted(reference).map_err(|error| match error {
        RunStorageError::ContentMissing { .. } => CliError::state(
            "accepted_snapshot_missing",
            2,
            "the accepted assessment is missing; restore the run's accepted directory",
        ),
        RunStorageError::ContentCorrupt { .. } => CliError::state(
            "hash_mismatch",
            2,
            "the accepted assessment no longer matches its manifest reference",
        ),
        error => CliError::storage("read accepted assessment", error),
    })
}

pub struct AcceptedReview {
    pub assessment: ContentRef,
    pub request: PublishRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drafts {
    /// New publication and local completion require the exact accepted draft.
    Clean,
    /// Recovery uses the owner's frozen bytes despite unaccepted edits.
    Ignored,
}

/// Checking and publication use the same declared review and workflow identity.
#[review_kit_observation::operations]
#[helper]
pub fn publication_request(
    bytes: &[u8],
    inputs: &AssessmentInputs<'_>,
    effects: Effects,
    review_workflow: Option<&str>,
) -> Result<PublishRequest, Vec<Diagnostic>> {
    let mut request = check_assessment(bytes, inputs, effects, false)?;
    request.review_workflow = review_workflow.map(str::to_owned);
    Ok(request)
}

#[review_kit_observation::operations]
#[helper]
pub fn accepted_review(
    run: &LoadedRun,
    effects: Effects,
    drafts: Drafts,
    review_workflow: Option<&str>,
) -> Result<AcceptedReview, CliError> {
    let assessment = run.manifest.review_assessment.clone().ok_or_else(|| {
        CliError::state(
            "assessment_not_accepted",
            2,
            "accept the complete review before planning, publication or local completion",
        )
    })?;
    let bytes = accepted_bytes(&run.dir, &assessment)?;
    let request = publication_request(
        &bytes,
        &AssessmentInputs {
            run_id: run.run_id(),
            context: &run.context,
            successor: Successor::Retained,
        },
        effects,
        review_workflow,
    )
    .map_err(|diagnostics| {
        CliError::state(
            "invalid_accepted_snapshot",
            2,
            "the accepted assessment no longer passes validation",
        )
        .with_details(json!({"checks": diagnostics}))
    })?;
    if drafts == Drafts::Clean {
        require_clean_draft(run)?;
    }
    Ok(AcceptedReview {
        assessment,
        request,
    })
}

#[review_kit_observation::operations]
#[helper]
pub fn require_clean_draft(run: &LoadedRun) -> Result<(), CliError> {
    let Some(accepted) = &run.manifest.review_assessment else {
        return Ok(());
    };
    let current = draft(&run.dir)?.as_deref().map(reference).transpose()?;
    if current.as_ref() == Some(accepted) {
        return Ok(());
    }
    Err(CliError::state("dirty_draft", 2,
        "the draft differs from its accepted snapshot; revise and accept it, or restore accepted bytes, before new publication or local completion")
        .with_details(json!({"draft": DRAFT_FILE})))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentState {
    Missing,
    Draft,
    MissingSnapshot,
    DamagedSnapshot,
    Dirty,
    Accepted,
}

#[review_kit_observation::operations]
impl AssessmentState {
    #[helper]
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Draft => "draft",
            Self::MissingSnapshot => "missing_snapshot",
            Self::DamagedSnapshot => "damaged_snapshot",
            Self::Dirty => "dirty",
            Self::Accepted => "accepted",
        }
    }
}

/// Status inspects stored bytes without changing or accepting them.
#[review_kit_observation::operations]
#[helper]
pub fn assessment_state(
    dir: &RunDir,
    accepted: Option<&ContentRef>,
) -> Result<AssessmentState, CliError> {
    let current = draft(dir)?.as_deref().map(reference).transpose()?;
    let Some(accepted) = accepted else {
        return Ok(if current.is_some() {
            AssessmentState::Draft
        } else {
            AssessmentState::Missing
        });
    };
    match dir.read_accepted(accepted) {
        Err(RunStorageError::ContentMissing { .. }) => return Ok(AssessmentState::MissingSnapshot),
        Err(_) => return Ok(AssessmentState::DamagedSnapshot),
        Ok(bytes) => {
            if review_kit_json_adapter::decode::<AssessmentDocument>(&bytes)
                .map_or(true, |document| document.run_id != dir.run_id().as_str())
            {
                return Ok(AssessmentState::DamagedSnapshot);
            }
        }
    }
    Ok(if current.as_ref() == Some(accepted) {
        AssessmentState::Accepted
    } else {
        AssessmentState::Dirty
    })
}
