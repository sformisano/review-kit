use std::collections::BTreeMap;

use review_kit_application::publication::{
    ActionId, FindingRecord, PlannedAction, ProviderId, ReplyKind, ReplyRecord, Round,
    SummaryRecord,
};
use review_kit_domain::{FindingId, ReviewerId};

use super::records::{Discussion, Note};
use crate::{Binding, GitLabError, wire::Marker};

pub(super) fn provider_id(id: u64) -> Result<ProviderId, GitLabError> {
    if id == 0 {
        return Err(GitLabError::Identity);
    }
    ProviderId::new(id.to_string()).map_err(|_| GitLabError::Identity)
}

pub(super) fn retained_rounds<'a>(
    summaries: &'a [SummaryRecord],
    binding: &Binding,
) -> Result<BTreeMap<u64, &'a Round>, GitLabError> {
    let mut rounds = BTreeMap::new();
    for summary in summaries {
        for round in summary
            .history
            .iter()
            .chain(std::iter::once(&summary.current))
        {
            if round.assessment.subject() != &binding.subject
                || round.assessment.reviewer() != &binding.reviewer
            {
                return Err(GitLabError::Identity);
            }
            if let Some(previous) = rounds.insert(round.number, round)
                && previous != round
            {
                return Err(GitLabError::Conflict);
            }
        }
    }
    Ok(rounds)
}

pub(super) fn map_finding(
    discussion: &Discussion,
    root: &Note,
    body: String,
    marker: &Marker,
    rounds: &BTreeMap<u64, &Round>,
) -> Result<FindingRecord, GitLabError> {
    let Marker::Finding { finding, round } = marker else {
        return Err(GitLabError::Marker);
    };
    if root.system
        || discussion.individual_note
        || root.kind.as_deref() != Some("DiffNote")
        || root.position.is_none()
    {
        return Err(GitLabError::UnsupportedPosition);
    }
    let retained = rounds.get(round).ok_or(GitLabError::State)?;
    let mut intended = retained.actions.iter().filter_map(|action| match action {
        PlannedAction::Finding {
            finding: value,
            body,
        } if value.id().get() == *finding => Some((value, body)),
        _ => None,
    });
    let (finding, expected_body) = intended.next().ok_or(GitLabError::State)?;
    if intended.next().is_some() || expected_body != &body {
        return Err(GitLabError::Conflict);
    }
    Ok(FindingRecord {
        id: ProviderId::new(discussion.id.clone()).map_err(|_| GitLabError::Identity)?,
        root_note: provider_id(root.id)?,
        author: root.reviewer()?,
        created_in: *round,
        revision: retained.assessment.revision().clone(),
        finding: finding.as_ref().clone(),
        body,
        resolved: discussion.resolved()?,
    })
}

pub(super) fn map_reply(
    note: &Note,
    root: &FindingRecord,
    reviewer: &ReviewerId,
    marked: Option<(String, Marker)>,
    rounds: &BTreeMap<u64, &Round>,
) -> Result<ReplyRecord, GitLabError> {
    let (body, action) = match marked {
        None => (note.body.clone(), None),
        Some((body, marker)) => {
            if !note.owned_by(reviewer) || note.system {
                return Err(GitLabError::Identity);
            }
            crate::body::validate_no_suggestions(&body)?;
            if note
                .suggestions
                .as_ref()
                .is_some_and(|suggestions| !suggestions.is_empty())
            {
                return Err(GitLabError::Conflict);
            }
            let (number, finding, expected_kind) = match marker {
                Marker::Transition { finding, round } => (round, finding, None),
                Marker::Answer {
                    finding,
                    round,
                    note,
                } => {
                    let target = ProviderId::new(note).map_err(|_| GitLabError::Marker)?;
                    (round, finding, Some(ReplyKind::Answer(target)))
                }
                _ => return Err(GitLabError::Marker),
            };
            let id = FindingId::try_from(finding).map_err(|_| GitLabError::Marker)?;
            if id != root.finding.id() {
                return Err(GitLabError::Conflict);
            }
            let retained = rounds.get(&number).ok_or(GitLabError::State)?;
            let mut matches = retained.actions.iter().filter_map(|action| match action {
                PlannedAction::Reply {
                    finding,
                    kind,
                    body,
                } if *finding == id => {
                    let same_kind = match (&expected_kind, kind) {
                        (None, ReplyKind::Transition(_)) => true,
                        (Some(expected), observed) => expected == observed,
                        _ => false,
                    };
                    same_kind.then_some((kind, body))
                }
                _ => None,
            });
            let (kind, expected_body) = matches.next().ok_or(GitLabError::State)?;
            if matches.next().is_some() || expected_body != &body {
                return Err(GitLabError::Conflict);
            }
            (
                body,
                Some(ActionId {
                    round: number,
                    finding: id,
                    kind: kind.clone(),
                }),
            )
        }
    };
    Ok(ReplyRecord {
        id: provider_id(note.id)?,
        author: note.reviewer()?,
        thread: root.id.clone(),
        action,
        body,
    })
}
