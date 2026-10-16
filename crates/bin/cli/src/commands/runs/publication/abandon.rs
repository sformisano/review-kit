use std::time::SystemTime;

use review_kit_application::{
    publication::Effects,
    use_cases::publishing::{
        AbandonRequest, ManagedHistory, PlanError, RunPublication, abandon_bound,
    },
};
use review_kit_filesystem_adapter::{SystemClock, WorkingFile};
use review_kit_json_adapter::AbandonDocument;
use serde_json::json;

use super::{
    Role, Session, connect, evidence, output, preflight::discover_current, publish::finish,
    require_binding, settled, writer,
};
use crate::{
    commands::{
        CommandOutput, rendering,
        status::{plan_exit, publication_exit},
    },
    error::CliError,
};

/// Close the round of this run's unfinished publication with a public reason.
///
/// Existing abandonment rules apply: uncertain writes must settle first and the
/// summary update needs current authority. The run settles as `abandoned`.
#[review_kit_observation::operations]
#[helper]
pub fn execute(
    session: &mut Session,
    role: Role,
    effects: Effects,
    reason: &str,
) -> Result<CommandOutput, CliError> {
    if let Role::Recovery { unsent: true, .. } = role {
        return Err(CliError::state(
            "no_unfinished_publication",
            5,
            format!(
                "there is no provider round to abandon. {}",
                crate::commands::runs::journal::UNSENT_GUIDANCE
            ),
        )
        .with_details(json!({"provider_write_possible": false})));
    }
    let Role::Recovery { binding, .. } = role else {
        return Err(CliError::state(
            "no_unfinished_publication",
            5,
            "the selected run owns no unfinished publication to abandon",
        ));
    };
    let mut renderer = rendering::load(&session.config)?;
    let profile = rendering::identity(renderer.identity());
    let mut events = evidence(&session.lock, &session.run, &session.store, &renderer)?;
    let gitlab = connect(&session.config, &session.run)?;
    let snapshot = discover_current(&gitlab, &session.run)?;
    let opened = session
        .store
        .open_journal(&ManagedHistory::from(&snapshot))
        .map_err(|error| CliError::journal("open publication journal", error))?;
    fault_point!("publish.after_journal_load");
    let mut progress = opened.progress;
    match progress.run_publication(session.run.run_id()) {
        RunPublication::Settled(entry) => {
            let entry = entry.clone();
            drop(events);
            return settled(session, &entry, "abandon");
        }
        RunPublication::BoundUnsettled(current) | RunPublication::BoundUnsent(current) => {
            require_binding(&session.run, current)?
        }
        RunPublication::Unbound => {
            return Err(CliError::state(
                "no_unfinished_publication",
                5,
                "the selected run owns no unfinished publication to abandon",
            ));
        }
    }
    let writer = writer(&session.lock, &session.run)?;
    if !session.run.manifest.publication_attempted {
        session.run.manifest.publication_attempted = true;
        writer
            .replace_json(WorkingFile::Manifest, &session.run.manifest)
            .map_err(|error| CliError::storage("record publication attempt", error))?;
    }
    fault_point!("publish.after_attempt_flag");
    let request = review_kit_application::use_cases::publishing::BindingRequest {
        run_id: binding.run_id.clone(),
        review_assessment: binding.review_assessment.clone(),
        snapshot_files: binding.snapshot_files.clone(),
    };
    let mut host = gitlab.publication_host(session.config.authority());
    let mut clock = SystemClock;
    let result = abandon_bound(
        &AbandonRequest {
            subject: session.run.manifest.subject.clone(),
            reviewer: session.run.manifest.reviewer.clone(),
            round: binding.round,
            reason: reason.to_owned(),
            effects,
        },
        Some(&request),
        &mut host,
        &mut renderer,
        &mut progress,
        &mut events,
        &mut clock,
    );
    fault_point!("publish.before_settlement");
    let settlement = match &result {
        Ok(published) => progress.settle_publication(&request, published, SystemTime::now()),
        Err(_) => Ok(None),
    };
    let exit = match &result {
        Ok(result) => publication_exit(result),
        Err(error) => plan_exit(error),
    };
    let current = match &result {
        Ok(result) => &result.final_snapshot.revision,
        Err(_) => &snapshot.revision,
    };
    let stranded = super::guidance::stranded_creation(&progress, current);
    let document = AbandonDocument::from_result(&result);
    let finished = finish(
        &session.store,
        &writer,
        &mut session.run.manifest,
        &progress,
        settlement,
        &document,
        exit,
    )?;
    let mut value = output(session, "abandon", &profile);
    value["status"] = json!(finished.status);
    value["round"] = json!(binding.round);
    value["settlement"] = finished.settlement;
    value["abandon"] = json!(document);
    value["storage_failures"] = json!(finished.storage_failures);
    value["next"] = match (&result, &stranded) {
        (Err(PlanError::ChangedRenderProfile), _) => json!(rendering::RESTORE_PROFILE),
        (_, Some(stranded)) if finished.status == "unfinished" => {
            json!(super::guidance::stranded_guidance(stranded, None))
        }
        _ => json!(finished.next),
    };
    Ok(CommandOutput {
        value,
        exit: finished.exit,
    })
}
