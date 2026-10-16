mod rounds;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use review_kit_application::publication::{PlannedAction, Round};
use serde::{Deserialize, Serialize};

use crate::GitLabError;

impl From<review_kit_json_adapter::JsonError> for GitLabError {
    fn from(error: review_kit_json_adapter::JsonError) -> Self {
        match error {
            review_kit_json_adapter::JsonError::Version(version) => {
                Self::UnsupportedStateVersion(version)
            }
            review_kit_json_adapter::JsonError::RetainedStateLimit => Self::Limit,
            _ => Self::State,
        }
    }
}

const MARKER_PREFIX: &str = "<!-- review-kit:v1 ";
const MARKER_SUFFIX: &str = " -->";
const DATA_PREFIX: &str = "\n\n<!-- review-kit:state:v3 ";
const DATA_SUFFIX: &str = " -->";
const DATA_TITLE: &str = "<summary>Review Kit data</summary>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Marker {
    Summary,
    Finding {
        finding: u64,
        round: u64,
    },
    Transition {
        finding: u64,
        round: u64,
    },
    Answer {
        finding: u64,
        round: u64,
        note: String,
    },
}

impl Marker {
    fn validate(&self) -> Result<(), GitLabError> {
        let (finding, round) = match self {
            Self::Summary => return Ok(()),
            Self::Finding { finding, round } | Self::Transition { finding, round } => {
                (*finding, *round)
            }
            Self::Answer {
                finding,
                round,
                note,
            } => {
                if note.parse::<u64>().ok().filter(|id| *id > 0).is_none() || note.starts_with('0')
                {
                    return Err(GitLabError::Marker);
                }
                (*finding, *round)
            }
        };
        if finding == 0 || round == 0 {
            return Err(GitLabError::Marker);
        }
        Ok(())
    }
}

pub(crate) fn append_marker(body: &str, marker: &Marker) -> Result<String, GitLabError> {
    validate_body(body)?;
    if !matches!(marker, Marker::Finding { .. }) {
        crate::body::validate_no_suggestions(body)?;
    }
    marker.validate()?;
    attach_marker(body, marker)
}

fn attach_marker(body: &str, marker: &Marker) -> Result<String, GitLabError> {
    Ok(format!(
        "{body}\n\n{MARKER_PREFIX}{}{MARKER_SUFFIX}",
        escaped_json(marker)?
    ))
}

pub(crate) fn parse_marker(body: &str) -> Result<Option<(String, Marker)>, GitLabError> {
    Ok(parse_records(body)?.map(|record| (record.original.to_owned(), record.marker)))
}

struct Records<'a> {
    original: &'a str,
    marker: Marker,
    state: Option<&'a str>,
}

fn parse_records(body: &str) -> Result<Option<Records<'_>>, GitLabError> {
    let records: Vec<_> = body
        .match_indices("<!--")
        .filter(|(offset, _)| body[offset + 4..].trim_start().starts_with("review-"))
        .map(|(offset, _)| offset)
        .collect();
    if records.is_empty() {
        if body.contains(DATA_TITLE) {
            return Err(GitLabError::State);
        }
        return Ok(None);
    }
    let (state_offset, offset) = match records.as_slice() {
        [offset] => (None, *offset),
        [state, offset] => (Some(*state), *offset),
        _ => return Err(GitLabError::Marker),
    };
    let record = &body[offset..];
    let payload = record
        .strip_prefix(MARKER_PREFIX)
        .and_then(|text| text.strip_suffix(MARKER_SUFFIX))
        .ok_or(GitLabError::Marker)?;
    if payload.contains('\n') || payload.contains('\r') || payload.contains("-->") {
        return Err(GitLabError::Marker);
    }
    let fields: serde_json::Value =
        serde_json::from_str(payload).map_err(|_| GitLabError::Marker)?;
    if fields.get("kind").and_then(serde_json::Value::as_str) == Some("summary")
        && fields.as_object().is_none_or(|object| object.len() != 1)
    {
        return Err(GitLabError::Marker);
    }
    let marker: Marker = serde_json::from_str(payload).map_err(|_| GitLabError::Marker)?;
    marker.validate()?;
    let (original, state) = match (state_offset, &marker) {
        (Some(state_offset), Marker::Summary) => {
            let record = body[state_offset..offset]
                .strip_suffix("\n\n")
                .ok_or(GitLabError::State)?;
            let (version, payload) = record
                .strip_prefix("<!-- review-kit:state:v")
                .and_then(|value| value.split_once(' '))
                .ok_or(GitLabError::State)?;
            let number = version.parse::<u64>().map_err(|_| GitLabError::State)?;
            if number.to_string() != version {
                return Err(GitLabError::State);
            }
            if number != rounds::State::VERSION {
                return Err(GitLabError::UnsupportedStateVersion(number));
            }
            let payload = payload
                .strip_suffix(DATA_SUFFIX)
                .ok_or(GitLabError::State)?;
            if !payload.starts_with('{') || payload.contains(['\n', '\r', '<', '>', '&', '`', '~'])
            {
                return Err(GitLabError::State);
            }
            if payload.len() > rounds::State::MAX_BYTES {
                return Err(GitLabError::Limit);
            }
            let original = body[..state_offset]
                .strip_suffix("\n\n")
                .ok_or(GitLabError::State)?;
            (original, Some(payload))
        }
        (None, Marker::Summary) | (Some(_), _) => return Err(GitLabError::State),
        (None, _) => (
            body[..offset]
                .strip_suffix("\n\n")
                .ok_or(GitLabError::Marker)?,
            None,
        ),
    };
    if original.contains(DATA_TITLE) {
        return Err(GitLabError::State);
    }
    if !fences_closed(original) {
        return Err(GitLabError::Marker);
    }
    Ok(Some(Records {
        original,
        marker,
        state,
    }))
}

pub(crate) fn encode_summary(
    body: &str,
    round: &Round,
    history: &[Round],
) -> Result<String, GitLabError> {
    validate_body(body)?;
    crate::body::validate_no_suggestions(body)?;
    rounds::validate_history(round, history)?;
    validate_actions(round, history)?;
    let state = rounds::State::from_rounds(round, history)?;
    state.clone().into_rounds()?;
    let payload = escaped_json(&state)?;
    if payload.len() > rounds::State::MAX_BYTES {
        return Err(GitLabError::Limit);
    }
    let content = format!("{body}{DATA_PREFIX}{payload}{DATA_SUFFIX}");
    attach_marker(&content, &Marker::Summary)
}

pub(crate) fn decode_summary(
    body: &str,
) -> Result<Option<(String, Round, Vec<Round>)>, GitLabError> {
    let Some(record) = parse_records(body)? else {
        return Ok(None);
    };
    if record.marker != Marker::Summary {
        return Ok(None);
    }
    validate_body(record.original)?;
    crate::body::validate_no_suggestions(record.original)?;
    let payload = record.state.ok_or(GitLabError::State)?;
    let state = rounds::State::decode(payload.as_bytes())?;
    let (round, history) = state.into_rounds()?;
    rounds::validate_history(&round, &history)?;
    validate_actions(&round, &history)?;
    Ok(Some((record.original.to_owned(), round, history)))
}

pub(crate) fn validate_actions(current: &Round, history: &[Round]) -> Result<(), GitLabError> {
    for round in history.iter().chain(std::iter::once(current)) {
        for action in round.actions.iter().chain(
            round
                .abandonment
                .iter()
                .flat_map(|record| &record.verified_actions),
        ) {
            validate_action(action)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_action(action: &PlannedAction) -> Result<(), GitLabError> {
    match action {
        PlannedAction::Finding { finding, body } => {
            validate_body(body)?;
            crate::body::validate_suggestion(body, finding)?;
        }
        PlannedAction::Reply { body, .. } => {
            validate_body(body)?;
            crate::body::validate_no_suggestions(body)?;
        }
        PlannedAction::SetResolved { .. } => {}
    }
    Ok(())
}

fn escaped_json(value: &impl Serialize) -> Result<String, GitLabError> {
    let json = serde_json::to_string(value).map_err(|_| GitLabError::State)?;
    // HTML and code delimiters in retained strings cannot terminate the state comment.
    Ok(json
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('`', "\\u0060")
        .replace('~', "\\u007e"))
}

pub(crate) fn validate_body(body: &str) -> Result<(), GitLabError> {
    crate::body::validate_quick_actions(body)?;
    // GitLab removes CR bytes before storing notes; exact retained bodies must survive readback.
    if body.contains('\r') {
        return Err(GitLabError::CarriageReturnBody);
    }
    if body
        .split("<!--")
        .skip(1)
        .any(|comment| comment.trim_start().starts_with("review-"))
        || body.contains(DATA_TITLE)
        || !fences_closed(body)
    {
        return Err(GitLabError::Marker);
    }
    Ok(())
}

fn fences_closed(body: &str) -> bool {
    let mut fence = None;
    for (event, range) in Parser::new(body).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))) => {
                let opening = &body[range.clone()];
                let delimiter = opening.as_bytes()[0];
                let length = opening
                    .bytes()
                    .take_while(|byte| *byte == delimiter)
                    .count();
                let content_end = opening
                    .find('\n')
                    .map_or(range.end, |newline| range.start + newline + 1);
                fence = Some((delimiter, length, content_end));
            }
            Event::Text(_) => {
                if let Some((_, _, end)) = &mut fence {
                    *end = range.end;
                }
            }
            Event::End(TagEnd::CodeBlock)
                if fence.take().is_some_and(|(delimiter, length, end)| {
                    // Container prefixes can follow the content of an implicitly closed block.
                    let closing = body[end..range.end].trim_end_matches([' ', '\t', '\n']);
                    closing
                        .bytes()
                        .rev()
                        .take_while(|byte| *byte == delimiter)
                        .count()
                        < length
                }) =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests;
