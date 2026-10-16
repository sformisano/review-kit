use std::error::Error;

use review_kit_application::{
    ports::PortError,
    publication::{Effect, Effects, Write},
};
use review_kit_observation::{Completion, ErrorInfo, Fields, Outcome};

use crate::{FindingConflict, GitLabError};

pub(crate) fn error(error: &GitLabError) -> ErrorInfo {
    let variant = match error {
        GitLabError::Config => "configuration",
        GitLabError::Identity => "identity",
        GitLabError::Protocol => "protocol",
        GitLabError::Marker => "marker",
        GitLabError::State => "state",
        GitLabError::UnsupportedStateVersion(_) => "unsupported_state_version",
        GitLabError::UnsupportedSource => "unsupported_source",
        GitLabError::UnsupportedPosition => "unsupported_position",
        GitLabError::UnsafeBody => "unsafe_body",
        GitLabError::CarriageReturnBody => "carriage_return_body",
        GitLabError::RevisionChanged => "revision_changed",
        GitLabError::Limit => "limit",
        GitLabError::Transport => "transport",
        GitLabError::Redirect => "redirect",
        GitLabError::Authentication => "authentication",
        GitLabError::Denied => "denied",
        GitLabError::NotFound => "not_found",
        GitLabError::RateLimited => "rate_limited",
        GitLabError::Rejected(_) => "rejected",
        GitLabError::Uncertain => "uncertain",
        GitLabError::Conflict => "conflict",
        GitLabError::FindingHistory { .. } => "finding_history",
    };
    let info = ErrorInfo::new("gitlab", variant);
    match error {
        GitLabError::Rejected(status) => {
            info.with_fields(Fields::new().number("http_status", u64::from(*status)))
        }
        GitLabError::FindingHistory { finding, conflict } => info.with_fields(
            Fields::new()
                .number("finding", finding.get())
                .label("conflict", conflict_label(*conflict)),
        ),
        _ => info,
    }
}

fn conflict_label(conflict: FindingConflict) -> &'static str {
    match conflict {
        FindingConflict::MissingRoot => "missing_root",
        FindingConflict::DuplicateRoot => "duplicate_root",
        FindingConflict::Inconsistent => "inconsistent",
    }
}

pub(crate) fn port_error(error: &PortError) -> ErrorInfo {
    let info = ErrorInfo::new("port", "gitlab");
    match error
        .source()
        .and_then(|cause| cause.downcast_ref::<GitLabError>())
    {
        Some(cause) => info.caused_by(self::error(cause)),
        None => info.caused_by(ErrorInfo::opaque("port_source")),
    }
}

pub(crate) fn result<T>(
    result: &Result<T, GitLabError>,
    output: impl FnOnce(&T) -> Fields,
) -> Completion {
    Completion::result(result, output, error)
}

pub(crate) fn port_result<T>(
    result: &Result<T, PortError>,
    output: impl FnOnce(&T) -> Fields,
) -> Completion {
    Completion::result(result, output, port_error)
}

pub(crate) fn write_input(write: &Write) -> Fields {
    let kind = match write.effect() {
        Effect::CreateSummary => "create_summary",
        Effect::UpdateSummary => "update_summary",
        Effect::Finding => "create_finding",
        Effect::Reply => "reply",
        Effect::Resolve => "resolve",
        Effect::Reopen => "reopen",
    };
    Fields::new()
        .label("kind", kind)
        .secret("write", "provider write")
}

pub(crate) fn effects(effects: &Effects) -> Fields {
    Fields::new().labels(
        "allowed",
        [
            (Effect::CreateSummary, "create_summary"),
            (Effect::UpdateSummary, "update_summary"),
            (Effect::Finding, "finding"),
            (Effect::Reply, "reply"),
            (Effect::Resolve, "resolve"),
            (Effect::Reopen, "reopen"),
        ]
        .into_iter()
        .filter_map(|(effect, name)| effects.contains(effect).then_some(name)),
    )
}

pub(crate) fn mutation_result(result: &Result<(), GitLabError>) -> Completion {
    match result {
        Ok(()) => Completion::success(Fields::new().label("effect", "acknowledged")),
        Err(GitLabError::Uncertain) => Completion::new(
            Outcome::Uncertain,
            Fields::new().label("effect", "uncertain"),
        )
        .with_error(error(&GitLabError::Uncertain)),
        Err(cause) => Completion::new(Outcome::Rejected, Fields::new().label("effect", "rejected"))
            .with_error(error(cause)),
    }
}
