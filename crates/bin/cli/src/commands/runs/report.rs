use review_kit_application::{
    publication::Write,
    use_cases::publishing::{PublicationProgress, RunPublication, WritePhase},
};
use review_kit_json_adapter::checkpoints::{ReviewRunDocument, RunLifecycle};
use serde_json::{Value, json};

use super::{
    chain::{AssessmentState, DRAFT_FILE, assessment_state},
    environment::{self, run_directory},
    journal::{self, JournalReading, settlement_value},
    loading::{StoredRun, damaged_retained, lifecycle_label, load_manifest, verify_retained},
};
use crate::{
    commands::{CommandOptions, CommandOutput},
    error::CliError,
};

/// Report the selected run without repairing or changing anything.
///
/// Only the manifest must be readable; missing, undecodable or mismatched
/// retained files are reported under `retained_files`.
#[review_kit_observation::operations]
#[operation(name = "cli.status", input = crate::logging::command_input("cli.status", options), output = crate::logging::command_result)]
pub fn execute(options: &CommandOptions) -> Result<CommandOutput, CliError> {
    let store = environment::locate()?;
    let (workspace, run_id) = environment::selected(&store)?;
    let stored = load_manifest(&store, Some(&workspace.repository), &run_id)?;
    let retained = retained_state(&stored);
    let StoredRun { dir, manifest } = &stored;
    let state = assessment_state(dir, manifest.review_assessment.as_ref())?;
    let reading = match options.load() {
        Ok(config) => journal::read(&config.run_store(&manifest.subject, &manifest.reviewer)),
        Err(error) => JournalReading::Unavailable(error),
    };
    let (status, completion, reason) = lifecycle_label(&manifest.lifecycle);
    Ok(CommandOutput::success(json!({
        "version": 1,
        "command": "status",
        "run_id": run_id.as_str(),
        "run_directory": run_directory(&run_id),
        "subject": review_kit_json_adapter::domain::SubjectData::from(&manifest.subject),
        "reviewer": manifest.reviewer.as_str(),
        "revision": review_kit_json_adapter::domain::RevisionData::from(&manifest.revision),
        "status": status,
        "completion": completion,
        "reason": reason,
        "publication_attempted": manifest.publication_attempted,
        "assessment": {"state": state.label(), "draft": DRAFT_FILE, "accepted": manifest.review_assessment.as_ref().map(|reference| reference.as_str())},
        "retained_files": retained,
        "journal": journal_value(manifest, &reading),
        "next": next_action(manifest, state, &reading, &retained, journal::attempted(dir, manifest)),
    })))
}

/// `intact`, `mismatch` (bytes differ from the recorded hashes but still decode
/// for this run), `newer` (every problem is a file written by a newer rkc) or
/// `damaged` (files missing, undecodable or describing another run), with the
/// affected file names.
#[review_kit_observation::operations]
#[helper]
fn retained_state(stored: &StoredRun) -> Value {
    let damaged = damaged_retained(stored);
    if !damaged.is_empty() {
        let newer = damaged.iter().all(|(_, error)| {
            matches!(
                error,
                CliError::State {
                    code: "newer_run_document",
                    ..
                }
            )
        });
        return json!({
            "state": if newer { "newer" } else { "damaged" },
            "files": damaged.iter().map(|(file, _)| file.name()).collect::<Vec<_>>(),
            "problems": damaged
                .iter()
                .map(|(file, error)| {
                    let diagnostic = &error.document()["diagnostics"][0];
                    json!({"file": file.name(), "code": diagnostic["code"], "message": diagnostic["message"]})
                })
                .collect::<Vec<_>>(),
        });
    }
    match verify_retained(&stored.dir, &stored.manifest) {
        Ok(()) => json!({"state": "intact"}),
        Err(error) => {
            let mut diagnostic = error.document()["diagnostics"][0].clone();
            diagnostic["state"] = json!("mismatch");
            diagnostic
        }
    }
}

#[review_kit_observation::operations]
#[helper]
fn journal_value(manifest: &ReviewRunDocument, reading: &JournalReading) -> Value {
    let mut value = json!({
        "state": reading.label(),
        "publication_owner": reading.owner().map(|owner| owner.as_str()),
        "settlement": null,
        "publication_status": null,
        "unresolved_effect": null,
        "dispatch_may_have_started": null,
    });
    if let JournalReading::Unavailable(error) = reading {
        value["cause"] = error.document()["diagnostics"][0].clone();
    }
    if let JournalReading::Journal(progress) = reading
        && reading.owner().is_some()
    {
        value["owner_provider_write_possible"] = json!(progress.unsent_binding().is_none());
    }
    match reading.publication(manifest) {
        Some(RunPublication::Settled(entry)) => {
            value["settlement"] = settlement_value(entry);
        }
        Some(RunPublication::BoundUnsent(binding)) => {
            value["publication_status"] = json!("bound_unsent");
            value["provider_write_possible"] = json!(false);
            value["round"] = json!(binding.round);
            if let JournalReading::Journal(progress) = reading {
                value["dispatch_may_have_started"] = json!(progress.dispatch_may_have_started);
                value["unresolved_effect"] = unresolved(progress);
            }
        }
        Some(RunPublication::BoundUnsettled(binding)) => {
            value["publication_status"] = json!("pending");
            value["provider_write_possible"] = json!(true);
            value["round"] = json!(binding.round);
            if let JournalReading::Journal(progress) = reading {
                value["dispatch_may_have_started"] = json!(progress.dispatch_may_have_started);
                value["unresolved_effect"] = unresolved(progress);
            }
        }
        _ => {}
    }

    value
}

#[review_kit_observation::operations]
#[helper]
fn unresolved(progress: &PublicationProgress) -> Value {
    let Some(pending) = &progress.unresolved else {
        return Value::Null;
    };
    let phase = match pending.phase {
        WritePhase::Prepared => "prepared",
        WritePhase::DispatchPossible => "dispatch_possible",
        WritePhase::LegacyAmbiguous => "legacy_ambiguous",
    };
    let (kind, finding) = match &pending.write {
        Write::Summary { expected, .. } => (
            if expected.is_some() {
                "update_summary"
            } else {
                "create_summary"
            },
            None,
        ),
        Write::Finding { finding, .. } => ("finding", Some(finding.id().get())),
        Write::Reply { action, .. } => ("reply", Some(action.finding.get())),
        Write::SetResolved { resolved, .. } => (if *resolved { "resolve" } else { "reopen" }, None),
    };
    json!({"kind": kind, "finding": finding, "phase": phase})
}

/// Guidance for a run whose retained files a newer rkc wrote.
const NEWER_GUIDANCE: &str = "A newer rkc wrote this run's retained files; continue the run with that rkc version or a later one, or stop it with rkc run stop --reason TEXT.";

/// The next command for the run, from journal state first and assessment state second.
///
/// `retained` is the `retained_files` value that `status` reports.
#[review_kit_observation::operations]
#[helper]
fn next_action(
    manifest: &ReviewRunDocument,
    state: AssessmentState,
    reading: &JournalReading,
    retained: &Value,
    attempted: bool,
) -> String {
    let retained_intact = retained["state"] == "intact";
    let newer = retained["state"] == "newer";
    match reading.publication(manifest) {
        Some(RunPublication::BoundUnsent(_)) if newer => {
            return "No write of this run's publication binding could have reached the provider, and a newer rkc wrote this run's retained files, so this rkc cannot resume it. Resume it with that rkc version or a later one, or start a fresh run with rkc run start --new and publish it; that publication releases this binding, after which this run can be stopped.".into();
        }
        Some(RunPublication::BoundUnsent(_)) => {
            return super::journal::UNSENT_GUIDANCE.into();
        }
        Some(RunPublication::BoundUnsettled(_)) => {
            return owner_recovery(manifest, state, retained);
        }
        Some(RunPublication::Settled(entry)) if manifest.lifecycle == RunLifecycle::Open => {
            if let Err(error) =
                journal::require_inputs(manifest, &entry.review_assessment, &entry.snapshot_files)
            {
                return error.to_string();
            }
            if newer {
                return "The journal records this run's settled publication, but a newer rkc wrote this run's retained files; run rkc publish --effects review with that rkc version or a later one to record it in the run manifest without provider writes.".into();
            }
            return "The journal records this run's settled publication; run rkc publish --effects review to record it in the run manifest without provider writes.".into();
        }
        _ => {}
    }
    if manifest.lifecycle != RunLifecycle::Open {
        return "This run is terminal and stays inspectable; start a fresh review with rkc run start.".into();
    }
    if attempted && matches!(reading, JournalReading::Unavailable(_)) {
        return format!(
            "This run attempted publication, but its publication journal is unavailable. Check the journal cause and configuration. {}",
            super::journal::MISSING_GUIDANCE
        );
    }
    if attempted && matches!(reading, JournalReading::Absent) {
        return format!(
            "This run attempted publication, but its publication journal is missing. {}",
            super::journal::MISSING_GUIDANCE
        );
    }
    if newer {
        return NEWER_GUIDANCE.into();
    }
    if !retained_intact {
        return "Retained run files are missing, damaged or changed; restore the run's exact files, or stop it with rkc run stop --reason TEXT and start a new run with rkc run start --new.".into();
    }
    match state {
        AssessmentState::Missing => return "Run rkc revise --reason TEXT --regenerate to prepare review-assessment.draft.json.".into(),
        AssessmentState::Draft => return "Complete review-assessment.draft.json, then run rkc accept.".into(),
        AssessmentState::MissingSnapshot | AssessmentState::DamagedSnapshot => return "The accepted assessment is missing or damaged; restore its exact accepted bytes, or stop this run and start a new run with rkc run start --new.".into(),
        AssessmentState::Dirty => return "The draft differs from its accepted snapshot; run rkc revise --reason TEXT, complete the successor and run rkc accept, or restore accepted bytes.".into(),
        AssessmentState::Accepted => {}
    }
    let unsent_owner =
        matches!(reading, JournalReading::Journal(progress) if progress.unsent_binding().is_some());
    match reading.owner() {
        Some(owner) if owner != &manifest.run_id && unsent_owner => format!(
            "Run {owner} holds a publication binding without any possible provider write; publishing this run releases it: run rkc publish --effects review --plan, then rkc publish --effects review."
        ),
        Some(owner) if owner != &manifest.run_id => format!(
            "Run {owner} owns unfinished publication for this merge request; recover it before publishing this run, or finish this run locally with rkc run finish."
        ),
        _ => "Run rkc publish --effects review --plan and then rkc publish --effects review, or rkc run finish for a local-only review.".into(),
    }
}

/// The next command for a run that owns unfinished publication.
///
/// Recovery and abandonment read the retained files, and recovery republishes
/// the exact accepted snapshots, so local damage to either is named before
/// any publication command.
#[review_kit_observation::operations]
#[helper]
fn owner_recovery(
    manifest: &ReviewRunDocument,
    state: AssessmentState,
    retained: &Value,
) -> String {
    const RECOVER: &str = "run rkc publish with the originally permitted effects to reconcile this run's unfinished publication";
    if retained["state"] == "newer" {
        return format!(
            "This run owns unfinished publication, but a newer rkc wrote its retained files. Install that rkc version or a later one, then {RECOVER}."
        );
    }
    if retained["state"] != "intact" {
        let files = retained["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|files| !files.is_empty())
            .unwrap_or_else(|| "its retained files".to_owned());
        return format!(
            "This run owns unfinished publication, but retained run files are missing, damaged or changed: {files}. Recovery and abandonment both read them. Restore the run's exact files from a copy of this run first, then {RECOVER}."
        );
    }
    let condition = match state {
        AssessmentState::MissingSnapshot => Some("missing"),
        AssessmentState::DamagedSnapshot => Some("damaged"),
        _ => None,
    };
    let snapshots =
        condition
            .zip(manifest.review_assessment.as_ref())
            .map(|(condition, reference)| {
                format!(
                    "the assessment snapshot {} at accepted/{}/ is {condition}",
                    reference.as_str(),
                    reference.hex()
                )
            });
    if snapshots.is_none() {
        return "Run rkc publish with the originally permitted effects to reconcile this run's unfinished publication, or rkc publish --abandon --reason TEXT after uncertain writes settle.".into();
    }
    format!(
        "This run owns unfinished publication, and recovery publishes from its exact accepted snapshots, but {}. Restore the accepted files in the run directory from a copy of this run first, then {RECOVER}. Only if the accepted files cannot be restored, close the round with rkc publish --abandon --reason TEXT once no write is uncertain; abandonment ends the round publicly with that reason on the merge request, and a fresh run with rkc run start --new reviews again.",
        snapshots.unwrap_or_default()
    )
}
