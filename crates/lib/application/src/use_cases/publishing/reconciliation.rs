use review_kit_domain::{Finding, FindingStatus, ReviewDecision, ReviewerId};

use super::planning::{PlanError, finding_effects_allowed, nonempty};
use crate::publication::*;

#[review_kit_observation::operations]
#[helper]
pub(super) fn finished_communication(round: &Round) -> bool {
    round.status == RoundStatus::Completed
        || (round.status == RoundStatus::Blocked
            && round.assessment.decision() == ReviewDecision::Blocked)
}

/// Structural finding changes become prior state only after their exact
/// transition reply is observed, including in completed rounds.
#[review_kit_observation::operations]
#[helper]
pub(crate) fn previous_finding(
    snapshot: &Snapshot,
    root: &FindingRecord,
    reviewer: &ReviewerId,
) -> Result<Finding, PlanError> {
    previous_finding_through(snapshot, root, reviewer, None, |_, _| true)
}

/// Replay observed transitions through the retained round, admitting only the
/// verified replies included by the caller's publication snapshot.
#[review_kit_observation::operations]
#[helper]
pub(crate) fn previous_finding_through(
    snapshot: &Snapshot,
    root: &FindingRecord,
    reviewer: &ReviewerId,
    cutoff: Option<u64>,
    mut accept_transition: impl FnMut(&Round, &ReplyRecord) -> bool,
) -> Result<Finding, PlanError> {
    let mut previous = root.finding.clone();
    let summary = snapshot.summaries.iter().find(|s| &s.author == reviewer);
    for round in summary
        .into_iter()
        .flat_map(|s| s.history.iter().chain(std::iter::once(&s.current)))
        .filter(|round| cutoff.is_none_or(|number| round.number <= number))
    {
        if round.number < root.created_in {
            continue;
        }
        let Some(desired) = round
            .assessment
            .findings()
            .iter()
            .find(|f| f.id() == root.finding.id())
        else {
            continue;
        };
        if finished_communication(round) {
            validate_planned_actions(round)?;
        }
        let eligible = round
            .abandonment
            .as_ref()
            .filter(|_| round.status == RoundStatus::Abandoned)
            .map_or(round.actions.as_slice(), |receipt| {
                receipt.verified_actions.as_slice()
            });
        for action in eligible {
            if let PlannedAction::Reply {
                finding,
                kind: ReplyKind::Transition(status),
                body,
            } = action
                && *finding == root.finding.id()
            {
                let id = ActionId {
                    round: round.number,
                    finding: *finding,
                    kind: ReplyKind::Transition(*status),
                };
                let mut replies = snapshot
                    .replies
                    .iter()
                    .filter(|r| &r.author == reviewer && r.action.as_ref() == Some(&id))
                    .filter(|reply| accept_transition(round, reply));
                match (replies.next(), replies.next()) {
                    (None, None) => {}
                    (Some(reply), None)
                        if reply.thread == root.id
                            && &reply.body == body
                            && *status == desired.status() =>
                    {
                        if !round.transitions.iter().any(|transition| {
                            transition.finding == *finding
                                && transition.from == previous.status()
                                && transition.to == *status
                        }) {
                            return Err(PlanError::InconsistentRound);
                        }
                        previous = desired.clone();
                    }
                    _ => return Err(PlanError::ConflictingReply(*finding)),
                }
            }
        }
        if finished_communication(round)
            && finding_effects_allowed(&round.assessment, desired)
            && !super::planning::same_finding(&previous, desired)
        {
            return Err(PlanError::InconsistentRound);
        }
    }
    Ok(previous)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn already_answered(
    snapshot: &Snapshot,
    root: &FindingRecord,
    answer: &Answer,
) -> Result<bool, PlanError> {
    let Some(summary) = snapshot
        .summaries
        .iter()
        .find(|summary| summary.author == root.author)
    else {
        return Ok(false);
    };
    for round in summary
        .history
        .iter()
        .chain(std::iter::once(&summary.current))
        .filter(|round| round.answers.contains(answer))
    {
        let kind = ReplyKind::Answer(answer.in_reply_to.clone());
        for action in &round.actions {
            if let PlannedAction::Reply {
                finding,
                kind: intended,
                body,
            } = action
                && *finding == answer.finding
                && intended == &kind
            {
                let id = ActionId {
                    round: round.number,
                    finding: *finding,
                    kind: kind.clone(),
                };
                let mut matches = snapshot.replies.iter().filter(|reply| {
                    reply.author == root.author && reply.action.as_ref() == Some(&id)
                });
                match (matches.next(), matches.next()) {
                    (None, None) => {}
                    (Some(reply), None) if reply.thread == root.id && &reply.body == body => {
                        return Ok(true);
                    }
                    _ => return Err(PlanError::ConflictingReply(answer.finding)),
                }
            }
        }
    }
    Ok(false)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn communicated_transition(
    snapshot: &Snapshot,
    root: &FindingRecord,
    desired: &Finding,
    reply: &TransitionReply,
) -> Result<bool, PlanError> {
    if !super::planning::same_finding(&previous_finding(snapshot, root, &root.author)?, desired) {
        return Ok(false);
    }
    for round in snapshot
        .summaries
        .iter()
        .filter(|summary| summary.author == root.author)
        .flat_map(|summary| std::iter::once(&summary.current).chain(summary.history.iter().rev()))
    {
        let Some(finding) = round
            .assessment
            .findings()
            .iter()
            .find(|finding| finding.id() == desired.id())
        else {
            continue;
        };
        let eligible = round
            .abandonment
            .as_ref()
            .filter(|_| round.status == RoundStatus::Abandoned)
            .map_or(round.actions.as_slice(), |receipt| {
                receipt.verified_actions.as_slice()
            });
        for action in eligible.iter().rev() {
            if let PlannedAction::Reply {
                finding: id,
                kind: ReplyKind::Transition(status),
                body,
            } = action
                && *id == desired.id()
            {
                let action_id = ActionId {
                    round: round.number,
                    finding: *id,
                    kind: ReplyKind::Transition(*status),
                };
                if snapshot.replies.iter().any(|record| {
                    record.author == root.author
                        && record.thread == root.id
                        && record.action.as_ref() == Some(&action_id)
                        && record.body == *body
                }) {
                    return Ok(super::planning::same_finding(finding, desired)
                        && round.transitions.contains(reply));
                }
            }
        }
    }
    Ok(false)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn unfinished_resolution(
    snapshot: &Snapshot,
    root: &FindingRecord,
    desired: &Finding,
) -> Result<Option<bool>, PlanError> {
    let reviewer = &root.author;
    let communicated = previous_finding(snapshot, root, reviewer)?;
    if communicated.status() != desired.status() {
        return Ok(None);
    }
    let summary = snapshot
        .summaries
        .iter()
        .find(|summary| &summary.author == reviewer);
    for round in summary
        .into_iter()
        .flat_map(|summary| std::iter::once(&summary.current).chain(summary.history.iter().rev()))
    {
        let Some(retained) = round
            .assessment
            .findings()
            .iter()
            .find(|finding| finding.id() == desired.id())
        else {
            continue;
        };
        if !finding_effects_allowed(&round.assessment, retained) {
            continue;
        }
        if finished_communication(round) {
            // Finished communication preserves later manual changes to thread state.
            return Ok(None);
        }
        if round.status == RoundStatus::Abandoned {
            return Ok(None);
        }
        if !super::planning::same_finding(retained, &communicated) {
            continue;
        }
        if let Some(resolved) = round.actions.iter().find_map(|action| match action {
            PlannedAction::SetResolved { finding, resolved } if *finding == desired.id() => {
                Some(*resolved)
            }
            _ => None,
        }) {
            if resolved
                != matches!(
                    retained.status(),
                    FindingStatus::Fixed | FindingStatus::Withdrawn
                )
            {
                return Err(PlanError::InconsistentRound);
            }
            return Ok(Some(resolved));
        }
    }
    Ok(None)
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_historical_roots(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<(), PlanError> {
    validate_abandonment(round, snapshot)?;
    for finding in round
        .assessment
        .findings()
        .iter()
        .filter(|finding| finding_effects_allowed(&round.assessment, finding))
    {
        // An unfinished create can be absent. Carried findings already have roots.
        let created_in_round = round.actions.iter().any(|action| matches!(
            action, PlannedAction::Finding { finding: planned, .. } if planned.id() == finding.id()
        ));
        if (finished_communication(round) || !created_in_round)
            && !snapshot.findings.iter().any(|root| {
                &root.author == round.assessment.reviewer() && root.finding.id() == finding.id()
            })
        {
            return Err(PlanError::MissingPublishedFinding(finding.id()));
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn remaining_actions(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<Vec<PlannedAction>, PlanError> {
    validate_retained_intent(round, snapshot)?;
    observed_remaining_actions(round, snapshot)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn observed_remaining_actions(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<Vec<PlannedAction>, PlanError> {
    validate_retained_structure(round, snapshot)?;
    let reviewer = round.assessment.reviewer();
    let mut remaining = Vec::new();
    for action in &round.actions {
        let finding_id = match action {
            PlannedAction::Finding { finding, .. } => finding.id(),
            PlannedAction::Reply { finding, .. } | PlannedAction::SetResolved { finding, .. } => {
                *finding
            }
        };
        let root = snapshot
            .findings
            .iter()
            .find(|r| &r.author == reviewer && r.finding.id() == finding_id);
        let satisfied = match action {
            PlannedAction::Finding { finding, body } => {
                nonempty(body.clone())?;
                match root {
                    None => false,
                    Some(root)
                        if root.finding == **finding
                            && root.body == *body
                            && root.created_in == round.number
                            && &root.revision == round.assessment.revision() =>
                    {
                        true
                    }
                    Some(_) => return Err(PlanError::ConflictingFinding(finding_id)),
                }
            }
            PlannedAction::Reply {
                finding,
                kind,
                body,
            } => {
                nonempty(body.clone())?;
                let root = root.ok_or(PlanError::MissingPublishedFinding(*finding))?;
                let id = ActionId {
                    round: round.number,
                    finding: *finding,
                    kind: kind.clone(),
                };
                let replies: Vec<_> = snapshot
                    .replies
                    .iter()
                    .filter(|r| &r.author == reviewer && r.action.as_ref() == Some(&id))
                    .collect();
                match replies.as_slice() {
                    [] => false,
                    [reply] if reply.thread == root.id && reply.body == *body => true,
                    _ => return Err(PlanError::ConflictingReply(*finding)),
                }
            }
            PlannedAction::SetResolved { resolved, .. } => {
                root.ok_or(PlanError::MissingPublishedFinding(finding_id))?
                    .resolved
                    == *resolved
            }
        };
        if !satisfied {
            remaining.push(action.clone());
        }
    }
    Ok(remaining)
}

#[review_kit_observation::operations]
#[helper]
fn validate_abandonment(round: &Round, snapshot: &Snapshot) -> Result<(), PlanError> {
    match (round.status, &round.abandonment) {
        (RoundStatus::Abandoned, Some(receipt)) => {
            if receipt.reason.trim().is_empty()
                || crate::ports::contains_reserved_marker(&receipt.reason)
                || receipt
                    .verified_actions
                    .iter()
                    .enumerate()
                    .any(|(index, action)| {
                        !round.actions.contains(action)
                            || receipt.verified_actions[..index]
                                .iter()
                                .any(|previous| same_action_identity(previous, action))
                    })
            {
                return Err(PlanError::InconsistentRound);
            }
            let mut verified = round.clone();
            verified.actions = receipt.verified_actions.clone();
            validate_completed_actions(&verified, snapshot)
        }
        (RoundStatus::Abandoned, None) | (_, Some(_)) => Err(PlanError::InconsistentRound),
        _ => Ok(()),
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_retained_intent(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<(), PlanError> {
    validate_retained_structure(round, snapshot)?;
    if let Some(answer) = changed_answer_targets(round, snapshot).first() {
        return Err(PlanError::InvalidAnswer(answer.finding));
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn changed_answer_targets(round: &Round, snapshot: &Snapshot) -> Vec<Answer> {
    round
        .answers
        .iter()
        .filter(|answer| {
            !round
                .answer_targets
                .iter()
                .any(|target| target.id == answer.in_reply_to && snapshot.replies.contains(target))
        })
        .cloned()
        .collect()
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_retained_structure(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<(), PlanError> {
    crate::publication::validate_policy_declarations(
        &round.policy_declarations,
        &round.assessment,
        &round.policy_names,
    )?;
    let reviewer = round.assessment.reviewer();
    validate_planned_actions(round)?;
    for finding in round.assessment.findings() {
        if !finding_effects_allowed(&round.assessment, finding) {
            continue;
        }
        let root = snapshot
            .findings
            .iter()
            .find(|r| &r.author == reviewer && r.finding.id() == finding.id());
        if root.is_none() && !round.actions.iter().any(|action| matches!(action, PlannedAction::Finding { finding: planned, .. } if planned.as_ref() == finding)) { return Err(PlanError::InconsistentRound); }
        if let Some(root) = root {
            let previous = previous_finding(snapshot, root, reviewer)?;
            if !super::planning::same_finding(&previous, finding) && !round.actions.iter().any(|action| matches!(action, PlannedAction::Reply { finding: id, kind: ReplyKind::Transition(status), .. } if *id == finding.id() && *status == finding.status())) { return Err(PlanError::InconsistentRound); }
        }
    }
    if round.answer_targets.len() != round.answers.len() {
        return Err(PlanError::InconsistentRound);
    }
    for answer in &round.answers {
        if !round.assessment.findings().iter().any(|finding| {
            finding.id() == answer.finding && finding_effects_allowed(&round.assessment, finding)
        }) {
            continue;
        }
        let root = snapshot
            .findings
            .iter()
            .find(|r| &r.author == reviewer && r.finding.id() == answer.finding)
            .ok_or(PlanError::InvalidAnswer(answer.finding))?;
        let expected = round
            .answer_targets
            .iter()
            .find(|r| r.id == answer.in_reply_to && r.thread == root.id)
            .ok_or(PlanError::InvalidAnswer(answer.finding))?;
        let _ = expected;
        let has_planned = round.actions.iter().any(|a| matches!(a, PlannedAction::Reply { finding, kind: ReplyKind::Answer(target), .. } if *finding == answer.finding && *target == answer.in_reply_to));
        let has_existing = already_answered(snapshot, root, answer)?;
        if !has_planned && !has_existing {
            return Err(PlanError::InconsistentRound);
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn validate_planned_actions(round: &Round) -> Result<(), PlanError> {
    for reply in &round.transitions {
        if reply.body.trim().is_empty()
            || !round
                .assessment
                .findings()
                .iter()
                .any(|f| f.id() == reply.finding && f.status() == reply.to)
        {
            return Err(PlanError::InvalidTransition(reply.finding));
        }
    }
    for (index, action) in round.actions.iter().enumerate() {
        if round.actions[..index]
            .iter()
            .any(|prior| same_action_identity(prior, action))
        {
            return Err(PlanError::InconsistentRound);
        }
        let id = match action {
            PlannedAction::Finding { finding, .. } => finding.id(),
            PlannedAction::Reply { finding, .. } | PlannedAction::SetResolved { finding, .. } => {
                *finding
            }
        };
        let desired = round
            .assessment
            .findings()
            .iter()
            .find(|f| f.id() == id)
            .ok_or(PlanError::InconsistentRound)?;
        if !finding_effects_allowed(&round.assessment, desired) {
            return Err(PlanError::InconsistentRound);
        }
        match action {
            PlannedAction::Finding { finding, body } => {
                nonempty(body.clone())?;
                if finding.as_ref() != desired || desired.status() != FindingStatus::Open {
                    return Err(PlanError::InconsistentRound);
                }
            }
            PlannedAction::Reply { kind, body, .. } => {
                nonempty(body.clone())?;
                match kind {
                    ReplyKind::Transition(status) if *status != desired.status() => {
                        return Err(PlanError::InconsistentRound);
                    }
                    ReplyKind::Transition(status)
                        if !round
                            .transitions
                            .iter()
                            .any(|r| r.finding == id && r.to == *status) =>
                    {
                        return Err(PlanError::InconsistentRound);
                    }
                    ReplyKind::Answer(target)
                        if !round
                            .answers
                            .iter()
                            .any(|a| a.finding == id && &a.in_reply_to == target) =>
                    {
                        return Err(PlanError::InconsistentRound);
                    }
                    _ => {}
                }
            }
            PlannedAction::SetResolved { resolved, .. } => {
                if *resolved
                    != matches!(
                        desired.status(),
                        FindingStatus::Fixed | FindingStatus::Withdrawn
                    )
                {
                    return Err(PlanError::InconsistentRound);
                }
            }
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn same_action_identity(left: &PlannedAction, right: &PlannedAction) -> bool {
    match (left, right) {
        (PlannedAction::Finding { finding: a, .. }, PlannedAction::Finding { finding: b, .. }) => {
            a.id() == b.id()
        }
        (
            PlannedAction::Reply {
                finding: a,
                kind: ak,
                ..
            },
            PlannedAction::Reply {
                finding: b,
                kind: bk,
                ..
            },
        ) => a == b && ak == bk,
        (
            PlannedAction::SetResolved { finding: a, .. },
            PlannedAction::SetResolved { finding: b, .. },
        ) => a == b,
        _ => false,
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_completed_actions(
    round: &Round,
    snapshot: &Snapshot,
) -> Result<(), PlanError> {
    validate_planned_actions(round)?;
    let reviewer = round.assessment.reviewer();
    for action in &round.actions {
        match action {
            PlannedAction::Finding { finding, body } => {
                let root = snapshot
                    .findings
                    .iter()
                    .find(|r| &r.author == reviewer && r.finding.id() == finding.id())
                    .ok_or(PlanError::MissingPublishedFinding(finding.id()))?;
                if root.finding != **finding
                    || root.body != *body
                    || root.created_in != round.number
                    || &root.revision != round.assessment.revision()
                {
                    return Err(PlanError::ConflictingFinding(finding.id()));
                }
            }
            PlannedAction::Reply {
                finding,
                kind,
                body,
            } => {
                let root = snapshot
                    .findings
                    .iter()
                    .find(|r| &r.author == reviewer && r.finding.id() == *finding)
                    .ok_or(PlanError::MissingPublishedFinding(*finding))?;
                let id = ActionId {
                    round: round.number,
                    finding: *finding,
                    kind: kind.clone(),
                };
                let replies: Vec<_> = snapshot
                    .replies
                    .iter()
                    .filter(|r| &r.author == reviewer && r.action.as_ref() == Some(&id))
                    .collect();
                if !matches!(replies.as_slice(), [reply] if reply.thread == root.id && reply.body == *body)
                {
                    return Err(PlanError::ConflictingReply(*finding));
                }
            }
            PlannedAction::SetResolved { .. } => {}
        }
    }
    Ok(())
}
