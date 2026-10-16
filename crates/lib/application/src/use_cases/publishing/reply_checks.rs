use crate::observation as log;
use std::collections::{BTreeMap, BTreeSet};

use review_kit_domain::FindingId;

use super::{
    planning::{
        PlanError, PublishRequest, cumulative_policy_names, nonempty, public_render_context,
        summary_context,
    },
    reconciliation::{already_answered, communicated_transition, previous_finding},
};
use crate::{
    ports::{
        AnswerRenderInput, FindingRenderInput, TransitionRenderInput, contains_reserved_marker,
    },
    publication::{ProviderId, ReplyRecord, Round, Snapshot},
};

pub(super) struct CheckedReplies {
    pub transitions: BTreeMap<FindingId, String>,
    pub answers: BTreeMap<(FindingId, ProviderId), String>,
    pub targets: Vec<ReplyRecord>,
}

#[review_kit_observation::operations]
#[operation(name = "application.validation.replies", input = log::Fields::new()
            .count("answers", request.answers.len())
            .count("transitions", request.transitions.len()), output = |result| {
            log::Completion::result(
                result,
                |value| {
                    log::Fields::new()
                        .count("answers", value.answers.len())
                        .count("transitions", value.transitions.len())
                },
                |errors| log::plan_errors(errors),
            )
        })]
/// Effect eligibility does not replace validation of authored replies.
pub(super) fn check_replies(
    request: &PublishRequest,
    snapshot: &Snapshot,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    _retained: Option<&Round>,
) -> Result<CheckedReplies, Vec<PlanError>> {
    let assessment = &request.assessment;
    let reviewer = assessment.reviewer();
    let mut errors = Vec::new();
    let (round, history) = summary_context(request, snapshot).map_err(|error| vec![error])?;
    let context = public_render_context(&round);
    let names = cumulative_policy_names(&round, &history);
    let mut transitions = BTreeMap::new();
    let mut answers = BTreeMap::new();
    let mut targets = Vec::new();
    let mut transition_ids = BTreeSet::new();
    for reply in &request.transitions {
        if !transition_ids.insert(reply.finding) {
            errors.push(PlanError::DuplicateTransition(reply.finding));
        }
        if reply.body.trim().is_empty() || contains_reserved_marker(&reply.body) {
            errors.push(PlanError::InvalidTransition(reply.finding));
        }
        let desired = assessment
            .findings()
            .iter()
            .find(|f| f.id() == reply.finding);
        let root = snapshot
            .findings
            .iter()
            .find(|root| &root.author == reviewer && root.finding.id() == reply.finding);
        match (desired, root) {
            (Some(desired), Some(root)) => {
                match previous_finding(snapshot, root, reviewer) {
                    Ok(previous) => {
                        let communicated =
                            match communicated_transition(snapshot, root, desired, reply) {
                                Ok(communicated) => communicated,
                                Err(error) => {
                                    errors.push(error);
                                    false
                                }
                            };
                        if reply.to != desired.status()
                            || (!communicated && reply.from != previous.status())
                            || (!communicated && reply.from == reply.to && previous == *desired)
                        {
                            errors.push(PlanError::InvalidTransition(reply.finding));
                        }
                    }
                    Err(error) => errors.push(error),
                }
                let rendered = names
                    .iter()
                    .find(|policy| &policy.id == desired.policy_id())
                    .ok_or_else(|| PlanError::MissingPolicyName(desired.policy_id().clone()))
                    .and_then(|policy| {
                        renderer
                            .transition_context(TransitionRenderInput {
                                finding: FindingRenderInput {
                                    finding: desired,
                                    policy_name: &policy.name,
                                    context: &context,
                                },
                                previous: reply.from,
                                explanation: &reply.body,
                            })
                            .map_err(PlanError::Render)
                    });
                match rendered.and_then(nonempty) {
                    Ok(body) => {
                        transitions.insert(reply.finding, body);
                    }
                    Err(error) => errors.push(error),
                }
            }
            _ => errors.push(PlanError::InvalidTransition(reply.finding)),
        }
    }
    let mut answer_ids = BTreeSet::new();
    for answer in &request.answers {
        let id = (answer.finding, answer.in_reply_to.clone());
        if !answer_ids.insert(answer.finding) {
            errors.push(PlanError::DuplicateAnswer);
        }
        if answer.body.trim().is_empty() || contains_reserved_marker(&answer.body) {
            errors.push(PlanError::InvalidAnswer(answer.finding));
        }
        let root = snapshot
            .findings
            .iter()
            .find(|root| &root.author == reviewer && root.finding.id() == answer.finding);
        let target = root.and_then(|root| {
            snapshot
                .replies
                .iter()
                .find(|reply| reply.id == answer.in_reply_to && reply.thread == root.id)
        });
        match target {
            Some(target) => {
                targets.push(target.clone());
                match root.map(|root| already_answered(snapshot, root, answer)) {
                    Some(Ok(true)) => {}
                    Some(Err(error)) => errors.push(error),
                    _ if !request.answer_targets.contains(target) => {
                        errors.push(PlanError::InvalidAnswer(answer.finding));
                    }
                    _ => {}
                }
            }
            None => errors.push(PlanError::InvalidAnswer(answer.finding)),
        }
        let rendered = match (root, target) {
            (Some(root), Some(target)) => names
                .iter()
                .find(|policy| &policy.id == root.finding.policy_id())
                .ok_or_else(|| PlanError::MissingPolicyName(root.finding.policy_id().clone()))
                .and_then(|policy| {
                    let current_finding = previous_finding(snapshot, root, reviewer)?;
                    renderer
                        .answer_context(AnswerRenderInput {
                            answer,
                            finding: root,
                            current_finding: &current_finding,
                            policy_name: &policy.name,
                            target,
                            context: &context,
                        })
                        .map_err(PlanError::Render)
                }),
            _ => Err(PlanError::InvalidAnswer(answer.finding)),
        };
        match rendered.and_then(nonempty) {
            Ok(body) => {
                answers.insert(id, body);
            }
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(CheckedReplies {
            transitions,
            answers,
            targets,
        })
    } else {
        Err(errors)
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn reply_error(mut errors: Vec<PlanError>) -> PlanError {
    if errors.len() == 1 {
        errors.remove(0)
    } else {
        PlanError::ReplyChecks(errors)
    }
}
