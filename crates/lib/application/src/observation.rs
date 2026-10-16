//! Disclosure policy for application operations and caller-supplied ports.

mod errors;
pub(crate) use errors::*;

use review_kit_domain::{Assessment, ReviewDecision, Subject};
pub(crate) use review_kit_observation::{Completion, ErrorInfo, Fields, Outcome};

use crate::{
    ports::{PortError, WriteOutcome},
    publication::{Effect, Effects, Plan, Snapshot, Write},
    use_cases::{
        publication_run::{ExecutionEvent, RunOutcome, RunReport},
        publishing::{PlanError, PublicationResult, PublicationStatus, PublishRequest},
    },
};

pub(crate) fn subject(value: &Subject) -> Fields {
    Fields::new()
        .secret("provider", value.provider())
        .secret("host", value.host())
        .secret("repository", value.repository())
        .secret("request", value.request().to_string())
}

pub(crate) fn assessment(value: &Assessment) -> Fields {
    Fields::new()
        .object("subject", subject(value.subject()))
        .secret("reviewer", value.reviewer().as_str())
        .secret("revision", value.revision().head())
        .label(
            "decision",
            match value.decision() {
                ReviewDecision::Ready => "ready",
                ReviewDecision::ChangesRequired => "changes_required",
                ReviewDecision::Blocked => "blocked",
            },
        )
        .count("findings", value.findings().len())
        .count("policies", value.selected_policies().len())
}

pub(crate) fn request(value: &PublishRequest) -> Fields {
    Fields::new()
        .object("assessment", assessment(&value.assessment))
        .object("effects", effects(&value.effects))
        .count("answers", value.answers.len())
        .count("transitions", value.transitions.len())
        .flag("allow_colocated", value.allow_colocated)
}

pub(crate) fn effect(value: Effect) -> &'static str {
    match value {
        Effect::CreateSummary => "create_summary",
        Effect::UpdateSummary => "update_summary",
        Effect::Finding => "create_finding",
        Effect::Reply => "create_reply",
        Effect::Resolve => "resolve",
        Effect::Reopen => "reopen",
    }
}

pub(crate) fn effects(value: &Effects) -> Fields {
    Fields::new().labels(
        "allowed",
        [
            Effect::CreateSummary,
            Effect::UpdateSummary,
            Effect::Finding,
            Effect::Reply,
            Effect::Resolve,
            Effect::Reopen,
        ]
        .into_iter()
        .filter(|item| value.contains(*item))
        .map(effect),
    )
}

pub(crate) fn snapshot(value: &Snapshot) -> Fields {
    Fields::new()
        .count("summaries", value.summaries.len())
        .count("findings", value.findings.len())
        .count("replies", value.replies.len())
        .object("permissions", effects(&value.permissions))
}

pub(crate) fn plan(value: &Plan) -> Fields {
    Fields::new()
        .number("round", value.round.number)
        .count("actions", value.actions.len())
        .flag("unchanged", value.unchanged)
        .count("unresolved_findings", value.unresolved_findings.len())
}

pub(crate) fn write(value: &Write) -> Fields {
    Fields::new().label("effect", effect(value.effect()))
}

pub(crate) fn publication(value: &PublicationResult) -> Completion {
    publication_with_fields(value, Fields::new())
}

fn publication_with_fields(value: &PublicationResult, fields: Fields) -> Completion {
    let fields = match &value.stop {
        Some(stop) => fields.error("stop", stop_error(stop)),
        None => fields.absent("stop"),
    };
    let fields = match &value.finalization_stop {
        Some(stop) => fields.error("finalization_error", stop_error(stop)),
        None => fields.absent("finalization_error"),
    };
    let (outcome, status) = match value.status {
        PublicationStatus::Unchanged => (Outcome::Success, "unchanged"),
        PublicationStatus::Completed => (Outcome::Success, "completed"),
        PublicationStatus::Blocked => (Outcome::Blocked, "blocked"),
        PublicationStatus::Partial => (Outcome::Partial, "partial"),
        PublicationStatus::Superseded => (Outcome::Partial, "superseded"),
        PublicationStatus::Uncertain => (Outcome::Uncertain, "uncertain"),
        PublicationStatus::Denied => (Outcome::Rejected, "denied"),
        PublicationStatus::NotStarted => (Outcome::Failure, "not_started"),
        PublicationStatus::Abandoned => (Outcome::Success, "abandoned"),
    };
    let mut completion = Completion::new(
        outcome,
        fields
            .label("status", status)
            .number("round", value.round)
            .count("planned", value.planned.len())
            .count("attempted", value.attempted.len())
            .count("readback_verified", value.verified.len())
            .count("skipped", value.skipped.len())
            .flag("finalization_failed", value.finalization_stop.is_some()),
    );
    if let Some(stop) = &value.stop {
        completion = completion.with_error(stop_error(stop));
    } else if let Some(stop) = &value.finalization_stop {
        completion = completion.with_error(stop_error(stop));
    }
    completion
}

pub(crate) fn publication_result(value: &Result<PublicationResult, PlanError>) -> Completion {
    match value {
        Ok(value) => publication(value),
        Err(error) => Completion::failure(plan_error(error)),
    }
}

pub(crate) fn run(value: &RunReport) -> Completion {
    let fields = Fields::new()
        .label(
            "mode",
            match value.mode {
                crate::use_cases::publication_run::RunMode::Plan => "plan",
                crate::use_cases::publication_run::RunMode::Publish => "publish",
            },
        )
        .flag("final_evidence_failed", value.evidence_failure.is_some());
    let fields = match &value.evidence_failure {
        Some(error) => fields.error("final_evidence_error", port_error(error)),
        None => fields.absent("final_evidence_error"),
    };
    match &value.outcome {
        Ok(RunOutcome::Planned(plan)) => {
            Completion::success(fields.object("plan", self::plan(plan)))
        }
        Ok(RunOutcome::Published(result)) => publication_with_fields(result, fields),
        Err(error) => Completion::new(Outcome::Failure, fields).with_error(plan_error(error)),
    }
}

pub(crate) fn write_outcome(value: &WriteOutcome) -> Completion {
    match value {
        WriteOutcome::Acknowledged => {
            Completion::success(Fields::new().label("effect_state", "acknowledged"))
        }
        WriteOutcome::Rejected(error) => Completion::new(
            Outcome::Rejected,
            Fields::new().label("effect_state", "rejected"),
        )
        .with_error(port_error(error)),
        WriteOutcome::Unknown(error) => Completion::new(
            Outcome::Uncertain,
            Fields::new().label("effect_state", "uncertain"),
        )
        .with_error(port_error(error)),
    }
}

pub(crate) fn evidence_stage(event: &ExecutionEvent<'_>) -> &'static str {
    match event {
        ExecutionEvent::Started { .. } => "started",
        ExecutionEvent::Observed(_) => "observed",
        ExecutionEvent::Planned(_) => "planned",
        ExecutionEvent::BeforeWrite(_) => "before_write",
        ExecutionEvent::DiscardedUnsent(_) => "discarded_unsent",
        ExecutionEvent::Readback { .. } => "readback",
        ExecutionEvent::Progress { .. } => "progress",
        ExecutionEvent::Finished(_) => "finished",
    }
}

pub(crate) fn rendered(result: &Result<String, PortError>) -> Completion {
    Completion::result(
        result,
        |body| Fields::new().secret("body", body.as_str()),
        port_error,
    )
}
