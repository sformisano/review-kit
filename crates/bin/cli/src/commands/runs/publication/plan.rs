use review_kit_application::{
    publication::Effects,
    review_run::RunId,
    use_cases::{
        publication_run::{RunMode, RunOutcome, RunPorts, run_bound},
        publishing::{ManagedHistory, PlanError, PublicationProgress},
    },
};
use review_kit_filesystem_adapter::{SystemClock, WorkingFile};
use review_kit_gitlab_adapter::validate_plan_bodies;
use review_kit_json_adapter::RunDocument;
use serde_json::{Value, json};

use super::{
    Role, Session, check_offline, connect,
    effects::{self, Authority, RoundWrites},
    evidence, output,
    preflight::{Freshness, admit_owner, discover_current, require_fresh},
    writer,
};
use crate::{
    commands::{
        CommandOutput, rendering,
        runs::{
            chain::{Drafts, accepted_review},
            journal::{self, JournalReading},
            loading::require_open,
        },
        shared::PREVIEW_NOTE,
        status::run_exit,
    },
    error::CliError,
};

/// Plan the selected run's accepted assessment without provider writes.
///
/// Planning leaves attempt and dispatch flags and journal bytes unchanged;
/// an absent key directory plans with empty progress when publication could
/// start an empty journal, and refuses as publication would otherwise.
#[review_kit_observation::operations]
#[helper]
pub fn execute(
    session: &mut Session,
    role: Role,
    effects: Effects,
) -> Result<CommandOutput, CliError> {
    let (drafts, freshness) = match &role {
        Role::Recovery { dispatched, .. } => (
            Drafts::Ignored,
            if *dispatched {
                Freshness::Skipped
            } else {
                Freshness::Required
            },
        ),
        _ => {
            require_open(&session.run.manifest)?;
            (Drafts::Clean, Freshness::Required)
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
    let mut events = evidence(&session.lock, &session.run, &session.store, &renderer)?;
    let admission = admission_view(&session.reading, &role, session.run.run_id());
    let mut offline = admission.journal.clone().unwrap_or_default();
    if !matches!(role, Role::Recovery { .. }) {
        offline.unresolved = None;
    }
    check_offline(&session.run, &chain, &mut renderer, &offline)?;
    let gitlab = connect(&session.config, &session.run)?;
    let snapshot = discover_current(&gitlab, &session.run)?;
    let owner_settlement = match role {
        Role::New => admit_owner(&session.reading, &session.run, &snapshot)?
            .map(|owner| owner.as_str().to_owned()),
        Role::Recovery { .. } | Role::Settled(_) => None,
    };
    if freshness == Freshness::Required {
        require_fresh(&gitlab, &session.run, &snapshot)?;
    }
    journal::require_empty_start(
        &session.reading,
        &session.store,
        &ManagedHistory::from(&snapshot),
    )?;
    let mut progress = admission.journal.unwrap_or_default();
    let mut reader = gitlab.clone();
    let mut source = gitlab.clone();
    let mut host = gitlab.publication_host(session.config.authority());
    let mut clock = SystemClock;
    let report = run_bound(
        &chain.request,
        None,
        RunMode::Plan,
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
    if let Ok(RunOutcome::Planned(plan)) = &report.outcome {
        validate_plan_bodies(plan).map_err(|error| {
            CliError::adapter("validate GitLab plan bodies", "gitlab_body", 2, error)
        })?;
    }
    let effects = match &report.outcome {
        Ok(RunOutcome::Planned(plan)) => effects::report(
            &Authority {
                requested: &chain.request.effects,
                configured: &session.config.authority(),
                progress: &progress,
                snapshot: &snapshot,
                reviewer: &session.run.manifest.reviewer,
            },
            &RoundWrites {
                round: plan.round.number,
                actions: &plan.actions,
                unchanged: plan.unchanged,
                decision: chain.request.assessment.decision(),
            },
        ),
        _ => Value::Null,
    };
    let mut document = RunDocument::from(&report);
    document.renderer = Some(profile.to_string());
    let mut exit = run_exit(&report);
    let writer = writer(&session.lock, &session.run)?;
    let mut storage_failures = Vec::new();
    if let Err(error) = writer.replace_json(WorkingFile::PublicationResult, &document) {
        storage_failures.push(json!({"code": "result_storage", "message": error.to_string()}));
        exit = 7;
    }
    if exit == 0 {
        rendering::save_run_preview(&writer, &profile)?;
    }
    let mut value = output(session, "plan", &profile);
    value["status"] = json!(if exit == 0 { "planned" } else { "failed" });
    value["provider_writes"] = json!(false);
    value["journal"] = json!(session.reading.label());
    value["recovery"] = json!(matches!(role, Role::Recovery { .. }));
    value["publication_owner_settled_by_provider"] = json!(owner_settlement);
    value["publication_owner_released"] = json!(admission.released.as_ref().map(RunId::as_str));
    value["preview_note"] = json!(PREVIEW_NOTE);
    value["effects"] = effects;
    value["run"] = json!(document);
    value["storage_failures"] = json!(storage_failures);
    value["next"] = json!(if exit == 0 {
        "Review the planned effects, then run rkc publish with the same effects."
    } else if let Err(PlanError::ChangedRenderProfile) = &report.outcome {
        rendering::RESTORE_PROFILE
    } else {
        "Resolve the reported problem before publishing."
    });
    Ok(CommandOutput { value, exit })
}

/// The journal that planning reads, and the owner whose binding it releases.
struct Admission {
    journal: Option<PublicationProgress>,
    released: Option<RunId>,
}

/// The journal as this run's publication would admit it, without persisting anything.
///
/// For a new run, another run's unsettled binding that sent nothing is released
/// exactly as admission releases it: its prepared intent is dropped and the
/// grant before it restored. Recovery plans the owner's own journal unchanged.
#[review_kit_observation::operations]
#[helper]
fn admission_view(reading: &JournalReading, role: &Role, run_id: &RunId) -> Admission {
    let journal = match reading {
        JournalReading::Journal(journal) => journal,
        JournalReading::Absent | JournalReading::Unavailable(_) => {
            return Admission {
                journal: None,
                released: None,
            };
        }
    };
    if let (Role::New, JournalReading::Journal(_)) = (role, reading)
        && let Some(view) = journal.released_for_admission(run_id)
    {
        return Admission {
            journal: Some(view),
            released: journal
                .coverage_binding
                .as_ref()
                .map(|binding| binding.run_id.clone()),
        };
    }
    Admission {
        journal: Some(journal.as_ref().clone()),
        released: None,
    }
}
