use review_kit_application::use_cases::checkpoints::Accepted;
use review_kit_filesystem_adapter::WorkingFile;
use review_kit_json_adapter::checkpoints::{
    AssessmentDocument, AssessmentRevisionInputs, RevisionError, RevisionRequest, revise_assessment,
};
use serde_json::json;

use crate::{
    commands::{
        CommandOptions, CommandOutput,
        runs::{
            chain::{DRAFT_FILE, accepted_bytes, draft},
            lifecycle::begin,
        },
    },
    error::CliError,
};

/// Archive exact current bytes before replacing the draft with its successor.
/// Malformed drafts remain unchanged unless regeneration is requested.
#[review_kit_observation::operations]
#[operation(name = "cli.revise", input = crate::logging::command_input("cli.revise", options).flag("regenerate", regenerate), output = crate::logging::command_result)]
pub fn execute(
    options: &CommandOptions,
    reason: &str,
    regenerate: bool,
) -> Result<CommandOutput, CliError> {
    let mutation = begin(options)?;
    let run = &mutation.run;
    let writer = mutation
        .lock
        .run(run.run_id())
        .map_err(|error| CliError::storage("open review run", error))?;
    let current = draft(&run.dir)?;
    let accepted = match &run.manifest.review_assessment {
        Some(reference) => Some(Accepted {
            reference: reference.clone(),
            content: review_kit_json_adapter::decode::<AssessmentDocument>(&accepted_bytes(
                &run.dir, reference,
            )?)?,
        }),
        None => None,
    };
    let revised = match revise_assessment(
        current.as_deref(),
        &AssessmentRevisionInputs {
            run_id: run.run_id(),
            context: &run.context,
            accepted: accepted.as_ref(),
        },
        RevisionRequest { reason, regenerate },
    ) {
        Ok(revised) => revised,
        Err(error) => {
            let diagnostics = error.diagnostics();
            let code = match error {
                RevisionError::MalformedDraft(_) => "malformed_draft",
                RevisionError::BlankReason => "blank_reason",
                RevisionError::Context(_) | RevisionError::Candidate(_) => "revision_failed",
            };
            return Ok(CommandOutput {
                value: json!({
                    "version": 1, "command": "revise", "run_id": run.run_id().as_str(),
                    "status": code, "diagnostics": diagnostics, "draft": DRAFT_FILE,
                    "next": if code == "malformed_draft" {
                        "Fix the draft JSON, or run rkc revise --reason TEXT --regenerate to rebuild from accepted state; exact current bytes are archived first."
                    } else { "Supply a nonblank --reason and revise again." },
                }),
                exit: 2,
            });
        }
    };
    let bytes = review_kit_json_adapter::encode(&revised.document)?;
    let archived = current
        .as_deref()
        .map(|bytes| writer.archive_draft(bytes))
        .transpose()
        .map_err(|error| CliError::storage("archive current draft", error))?;
    fault_point!("revise.after_archive");
    writer
        .replace(WorkingFile::Draft, &bytes)
        .map_err(|error| CliError::storage("write successor draft", error))?;
    super::previews(&writer, None)?;
    Ok(CommandOutput::success(json!({
        "version": 1, "command": "revise", "run_id": run.run_id().as_str(),
        "status": "prepared", "draft": DRAFT_FILE,
        "supersedes": run.manifest.review_assessment.as_ref().map(|reference| reference.as_str()),
        "archived_draft": archived.map(|stored| format!("draft-history/{}/{DRAFT_FILE}", stored.reference.hex())),
        "regenerated": regenerate, "diagnostics": revised.diagnostics,
        "preserved": ["policy_declarations", "excluded_and_unresolved_outcomes", "new_findings", "answers", "transitions", "finding_reassessments", "depth", "scope", "limitation", "local_context"],
        "reset": ["evaluated_policy_outcomes", "decision", "finding_validation_flags", "confirmations"],
        "next": "Complete review-assessment.draft.json, then run rkc accept.",
    })))
}
