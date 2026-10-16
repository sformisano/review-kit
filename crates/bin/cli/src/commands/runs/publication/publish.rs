use std::time::SystemTime;

use review_kit_application::{
    publication::Effects,
    use_cases::{
        publication_run::{RunMode, RunOutcome, RunPorts, run_bound},
        publishing::{
            BindingError, ManagedHistory, PlanError, PublicationProgress, PublicationStatus,
            RecoveryPolicy, RunPublication, SettledRun,
        },
    },
};
use review_kit_filesystem_adapter::{SystemClock, WorkingFile};
use review_kit_json_adapter::RunDocument;
use serde_json::{Value, json};

use super::{
    Role, Session, binding_request, check_offline, connect,
    effects::{self, Authority, RoundWrites},
    evidence, guidance, output, own_progress,
    preflight::{Freshness, admit_owner, discover_current, require_fresh},
    require_binding, settled, writer,
};
use crate::{
    args::UncertainRetry,
    commands::{
        CommandOutput, rendering,
        runs::{
            chain::{Drafts, accepted_review},
            journal::{self, pending_owner, settled_lifecycle, settlement_value},
            loading::require_open,
        },
        status::run_exit,
    },
    error::CliError,
};

/// Publish the selected run, or recover its unfinished publication.
///
/// Durable order: journal established, run marked as attempted, engine binding and
/// intent before dispatch, journal settlement, and only then local completion.
#[review_kit_observation::operations]
#[helper]
pub fn execute(
    session: &mut Session,
    role: Role,
    effects: Effects,
    retry: UncertainRetry,
) -> Result<CommandOutput, CliError> {
    let freshness = match &role {
        Role::Recovery {
            dispatched: true, ..
        } => Freshness::Skipped,
        _ => Freshness::Required,
    };
    let drafts = match &role {
        Role::Recovery { .. } => Drafts::Ignored,
        _ => {
            require_open(&session.run.manifest)?;
            Drafts::Clean
        }
    };
    let chain = accepted_review(
        &session.run,
        effects,
        drafts,
        session.config.review_workflow.as_deref(),
    )?;
    let mut renderer = rendering::load(&session.config)?;
    let profile = rendering::identity(renderer.identity());
    let dispatched = matches!(
        role,
        Role::Recovery {
            dispatched: true,
            ..
        }
    );
    rendering::verify_run_preview(&session.run.dir, &profile, !dispatched).map_err(|error| {
        match &role {
            Role::Recovery { .. } => error.with_detail("next", json!(rendering::RESTORE_PROFILE)),
            _ => error,
        }
    })?;
    let mut events = evidence(&session.lock, &session.run, &session.store, &renderer)?;
    check_offline(
        &session.run,
        &chain,
        &mut renderer,
        &own_progress(&session.reading, &role),
    )?;
    let gitlab = connect(&session.config, &session.run)?;
    let unsent = matches!(role, Role::Recovery { unsent: true, .. });
    let unsent_hint = |error: CliError| {
        if unsent {
            error.with_detail("next", json!(journal::UNSENT_GUIDANCE))
        } else {
            error
        }
    };
    let snapshot = discover_current(&gitlab, &session.run).map_err(&unsent_hint)?;
    if let Role::New = role {
        admit_owner(&session.reading, &session.run, &snapshot)?;
    }
    if freshness == Freshness::Required {
        require_fresh(&gitlab, &session.run, &snapshot).map_err(&unsent_hint)?;
    }
    let history = ManagedHistory::from(&snapshot);
    journal::require_empty_start(&session.reading, &session.store, &history)?;
    let opened = session
        .store
        .open_journal(&history)
        .map_err(|error| CliError::journal("open publication journal", error))?;
    fault_point!("publish.after_journal_load");
    let mut progress = opened.progress;
    match progress.run_publication(session.run.run_id()) {
        RunPublication::Settled(entry) => {
            let entry = entry.clone();
            drop(events);
            return settled(session, &entry, "publish");
        }
        RunPublication::BoundUnsettled(binding) | RunPublication::BoundUnsent(binding) => {
            require_binding(&session.run, binding)?
        }
        RunPublication::Unbound => require_open(&session.run.manifest)?,
    }
    progress.recovery_policy = match retry {
        UncertainRetry::Once => RecoveryPolicy::BoundedRetry,
        UncertainRetry::Never => RecoveryPolicy::Conservative,
    };
    let writer = writer(&session.lock, &session.run)?;
    if !session.run.manifest.publication_attempted {
        session.run.manifest.publication_attempted = true;
        writer
            .replace_json(WorkingFile::Manifest, &session.run.manifest)
            .map_err(|error| CliError::storage("record publication attempt", error))?;
    }
    fault_point!("publish.after_attempt_flag");
    let binding = binding_request(&session.run, &chain);
    let mut reader = gitlab.clone();
    let mut source = gitlab.clone();
    let mut host = gitlab.publication_host(session.config.authority());
    let mut clock = SystemClock;
    let report = run_bound(
        &chain.request,
        Some(&binding),
        RunMode::Publish,
        RunPorts {
            reader: &mut reader,
            host: &mut host,
            source: &mut source,
            renderer: &mut renderer,
            clock: &mut clock,
            evidence: &mut events,
            progress: &mut progress,
        },
    );
    fault_point!("publish.before_settlement");
    let settlement = match &report.outcome {
        Ok(RunOutcome::Published(result)) => {
            progress.settle_publication(&binding, result, SystemTime::now())
        }
        _ => Ok(None),
    };
    let effects = match &report.outcome {
        Ok(RunOutcome::Published(result)) => effects::report(
            &Authority {
                requested: &chain.request.effects,
                configured: &session.config.authority(),
                progress: &progress,
                snapshot: &snapshot,
                reviewer: &session.run.manifest.reviewer,
            },
            &RoundWrites {
                round: result.round,
                actions: &result.planned,
                unchanged: result.status == PublicationStatus::Unchanged,
                decision: result.decision,
            },
        ),
        _ => Value::Null,
    };
    let mut document = RunDocument::from(&report);
    document.renderer = Some(profile.to_string());
    let finished = finish(
        &session.store,
        &writer,
        &mut session.run.manifest,
        &progress,
        settlement,
        &document,
        run_exit(&report),
    )?;
    let mut value = output(session, "publish", &profile);
    value["status"] = json!(finished.status);
    value["provider_writes"] = json!(true);
    value["journal_origin"] = json!(format!("{:?}", opened.origin).to_lowercase());
    value["recovery"] = json!(matches!(role, Role::Recovery { .. }));
    value["settlement"] = finished.settlement;
    value["publication"] = publication_summary(&report.outcome);
    value["effects"] = effects;
    value["run"] = json!(document);
    value["storage_failures"] = json!(finished.storage_failures);
    value["next"] = json!(finished.next);
    let final_snapshot = match &report.outcome {
        Ok(RunOutcome::Published(result)) => &result.final_snapshot,
        _ => &snapshot,
    };
    let round = guidance::unfinished_round(final_snapshot, &session.run.manifest.reviewer);
    let stranded = guidance::stranded_creation(&progress, &final_snapshot.revision);
    if let Err(PlanError::Binding(BindingError::PendingOwner(owner))) = &report.outcome {
        let refused = match &stranded {
            Some(stranded) => pending_owner(owner, false).with_detail(
                "next",
                json!(guidance::stranded_guidance(stranded, Some(owner))),
            ),
            None => pending_owner(owner, false),
        };
        value["owner_run_id"] = json!(owner.as_str());
        value["diagnostics"] = refused.document()["diagnostics"].clone();
        value["next"] = value["diagnostics"][0]["next"].clone();
    } else if let Err(PlanError::ChangedRenderProfile) = &report.outcome {
        value["next"] = json!(rendering::RESTORE_PROFILE);
    } else if let Some(stranded) = &stranded
        && finished.status == "unfinished"
    {
        value["next"] = json!(guidance::stranded_guidance(stranded, None));
    } else if finished.status == "unfinished" && run_exit(&report) == 3 {
        let diagnostic = guidance::changed_revision(round);
        value["next"] = diagnostic["next"].clone();
        value["diagnostics"] = json!([diagnostic]);
    } else if let Err(error) = &report.outcome
        && finished.status == "unfinished"
        && matches!(role, Role::Recovery { .. })
        && guidance::stale_inputs(error)
    {
        value["next"] = json!(format!(
            "This run's accepted assessment can no longer complete its round: {error}. {}",
            guidance::close_round(round)
        ));
    }
    if finished.status == "unfinished"
        && progress
            .unsent_binding()
            .is_some_and(|binding| &binding.run_id == session.run.run_id())
    {
        value["provider_write_possible"] = json!(false);
        value["next"] = json!(journal::UNSENT_GUIDANCE);
    }
    Ok(CommandOutput {
        value,
        exit: finished.exit,
    })
}

/// Final persistence of one publication or abandonment.
pub struct Finished {
    pub status: &'static str,
    pub settlement: Value,
    pub storage_failures: Vec<Value>,
    pub exit: u8,
    pub next: &'static str,
}

/// Save the journal, then the manifest when the journal holds this run's settlement, then the result.
///
/// A journal that cannot be saved leaves the run open, so the settlement is
/// recorded again by the next recovery instead of being lost.
#[review_kit_observation::operations]
#[helper]
pub fn finish(
    store: &review_kit_filesystem_adapter::FileProgressStore,
    writer: &review_kit_filesystem_adapter::RunWriter<'_>,
    manifest: &mut review_kit_json_adapter::checkpoints::ReviewRunDocument,
    progress: &PublicationProgress,
    settlement: Result<Option<SettledRun>, BindingError>,
    document: &impl serde::Serialize,
    exit: u8,
) -> Result<Finished, CliError> {
    let mut storage_failures = Vec::new();
    let saved = store.save_journal(progress);
    if let Err(error) = &saved {
        storage_failures.push(json!({"code": "progress_storage", "message": error.to_string()}));
    }
    let (status, settlement_value_, exit, next) = match settlement {
        Ok(Some(entry)) if saved.is_ok() => {
            fault_point!("publish.after_settlement");
            manifest.lifecycle = settled_lifecycle(&entry);
            writer
                .replace_json(WorkingFile::Manifest, manifest)
                .map_err(|error| CliError::storage("complete run manifest", error))?;
            (
                "settled",
                settlement_value(&entry),
                exit,
                "Publication settled; start a fresh review with rkc run start for later changes.",
            )
        }
        Ok(_) => (
            "unfinished",
            Value::Null,
            exit,
            "Publication is unfinished; run rkc status, then rkc publish again with the original effects to recover it.",
        ),
        Err(error) => (
            "settlement_rejected",
            json!({"error": journal::binding_code(&error), "message": error.to_string()}),
            5,
            "The journal rejected this run's settlement; inspect it with rkc status before retrying.",
        ),
    };
    if let Err(error) = writer.replace_json(WorkingFile::PublicationResult, document) {
        storage_failures.push(json!({"code": "result_storage", "message": error.to_string()}));
    }
    Ok(Finished {
        status,
        settlement: settlement_value_,
        exit: if storage_failures.is_empty() { exit } else { 7 },
        storage_failures,
        next,
    })
}

/// Compact publication status for command output; the full report is under `run`.
#[review_kit_observation::operations]
#[helper]
fn publication_summary(outcome: &Result<RunOutcome, PlanError>) -> Value {
    match outcome {
        Ok(RunOutcome::Published(result)) => json!({
            "status": review_kit_json_adapter::PublicationState::from(result.status),
            "round": result.round,
            "attempted": result.attempted.len(),
            "verified": result.verified.len(),
            "retried_creations": result.retried_creations.len(),
        }),
        Ok(RunOutcome::Planned(_)) => json!({"status": "planned"}),
        Err(error) => json!({"status": "rejected", "error": error.to_string()}),
    }
}
