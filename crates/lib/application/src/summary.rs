use std::collections::{BTreeMap, BTreeSet};

use review_kit_domain::{Finding, FindingId, FindingStatus, PolicyId, PolicyJudgment};

use crate::{
    publication::*,
    use_cases::publishing::{PlanError, previous_finding},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishedAction {
    Created,
    Transition {
        from: FindingStatus,
        to: FindingStatus,
    },
    Answered {
        in_reply_to: ProviderId,
    },
    Resolved,
    Reopened,
    ObservedResolved,
    ObservedReopened,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryFinding {
    pub root: FindingRecord,
    /// Last communicated state, including verified replies in an unfinished round.
    pub finding: Finding,
    pub actions: Vec<PublishedAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalFinding {
    pub finding: Finding,
    pub root_note: ProviderId,
    pub thread: ProviderId,
    pub actions: Vec<PublishedAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FindingCounts {
    pub total: usize,
    pub open: usize,
    pub fixed: usize,
    pub withdrawn: usize,
    pub uncertain: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySummary {
    pub policy_id: PolicyId,
    pub name: Option<String>,
    pub applicable_rounds: Vec<u64>,
    /// Counts distinct published root notes under their original policy footer.
    pub comments: usize,
    pub declarations: Vec<PolicyDeclarationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDeclarationSummary {
    pub round: u64,
    pub declaration: PublicPolicyDeclaration,
    pub result: Option<PolicyJudgment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryView {
    pub round: Round,
    pub history: Vec<Round>,
    pub findings: Vec<SummaryFinding>,
    pub counts: FindingCounts,
    pub policies: Vec<PolicySummary>,
    pub unpublished_findings: Vec<FindingId>,
}

#[review_kit_observation::operations]
#[operation(name = "application.summary.project", input = crate::observation::Fields::new()
            .number("round", round.number)
            .count("history", history.len())
            .count("findings", snapshot.findings.len()), output = |result| {
            crate::observation::Completion::result(
                result,
                |value| {
                    crate::observation::Fields::new()
                        .count("findings", value.findings.len())
                        .count("policies", value.policies.len())
                },
                crate::observation::plan_error,
            )
        })]
/// Links, counts, and action labels use observed provider records. The round
/// retains intended judgments separately from verified communication.
pub fn summary_view(
    round: &Round,
    history: &[Round],
    snapshot: &Snapshot,
) -> Result<SummaryView, PlanError> {
    let reviewer = round.assessment.reviewer();
    crate::use_cases::discovery::validate_snapshot(snapshot, round.assessment.subject(), reviewer)
        .map_err(|error| {
            PlanError::Discovery(crate::use_cases::discovery::DiscoveryError::Conflict(error))
        })?;
    let mut findings = Vec::new();
    let mut counts = FindingCounts::default();
    let mut policy_rows: BTreeMap<PolicyId, (BTreeSet<u64>, BTreeSet<ProviderId>)> =
        BTreeMap::new();
    let mut policy_names = BTreeMap::new();
    let mut declarations: BTreeMap<PolicyId, Vec<PolicyDeclarationSummary>> = BTreeMap::new();
    for applicable in history.iter().chain(std::iter::once(round)) {
        for declaration in &applicable.policy_declarations {
            let policy = &declaration.policy;
            let row = policy_rows.entry(policy.id.clone()).or_default();
            policy_names.insert(policy.id.clone(), policy.name.clone());
            if declaration.status == crate::use_cases::checkpoints::ApplicabilityStatus::Applicable
            {
                row.0.insert(applicable.number);
            }
            let result = applicable
                .assessment
                .evaluations()
                .iter()
                .find(|evaluation| evaluation.policy_id() == &policy.id)
                .map(|evaluation| evaluation.judgment().clone());
            declarations
                .entry(policy.id.clone())
                .or_default()
                .push(PolicyDeclarationSummary {
                    round: applicable.number,
                    declaration: declaration.clone(),
                    result,
                });
        }
    }
    for root in snapshot.findings.iter().filter(|r| &r.author == reviewer) {
        policy_rows
            .entry(root.finding.policy_id().clone())
            .or_default()
            .1
            .insert(root.root_note.clone());
        let finding = previous_finding(snapshot, root, reviewer)?;
        let mut actions = Vec::new();
        let eligible = round
            .abandonment
            .as_ref()
            .filter(|_| round.status == RoundStatus::Abandoned)
            .map_or(round.actions.as_slice(), |receipt| {
                receipt.verified_actions.as_slice()
            });
        if creation_is_visible(round, root, eligible) {
            actions.push(PublishedAction::Created);
        }
        for action in eligible {
            match action {
                PlannedAction::Reply {
                    finding: id,
                    kind,
                    body,
                } if *id == finding.id() => {
                    let action_id = ActionId {
                        round: round.number,
                        finding: *id,
                        kind: kind.clone(),
                    };
                    if snapshot.replies.iter().any(|reply| {
                        &reply.author == reviewer
                            && reply.thread == root.id
                            && reply.action.as_ref() == Some(&action_id)
                            && &reply.body == body
                    }) {
                        match kind {
                            ReplyKind::Transition(to) => {
                                let reply = round
                                    .transitions
                                    .iter()
                                    .find(|reply| reply.finding == *id && reply.to == *to)
                                    .ok_or(PlanError::InconsistentRound)?;
                                actions.push(PublishedAction::Transition {
                                    from: reply.from,
                                    to: *to,
                                });
                            }
                            ReplyKind::Answer(target) => actions.push(PublishedAction::Answered {
                                in_reply_to: target.clone(),
                            }),
                        }
                    }
                }
                PlannedAction::SetResolved {
                    finding: id,
                    resolved,
                } if *id == finding.id() && resolution_is_recorded(round, *id, *resolved) => {
                    let provenance = round
                        .verified_resolutions
                        .iter()
                        .find(|receipt| receipt.thread == root.id && receipt.resolved == *resolved)
                        .map_or(ResolutionProvenance::Acknowledged, |receipt| {
                            receipt.provenance
                        });
                    actions.push(resolution_action(*resolved, provenance));
                }
                _ => {}
            }
        }
        counts.total += 1;
        match finding.status() {
            FindingStatus::Open => counts.open += 1,
            FindingStatus::Fixed => counts.fixed += 1,
            FindingStatus::Withdrawn => counts.withdrawn += 1,
            FindingStatus::Uncertain => counts.uncertain += 1,
        }
        findings.push(SummaryFinding {
            root: root.clone(),
            finding,
            actions,
        });
    }
    findings.sort_by_key(|row| row.finding.id());
    let mut unpublished_findings: Vec<_> = round
        .assessment
        .findings()
        .iter()
        .filter(|f| !findings.iter().any(|row| row.finding.id() == f.id()))
        .filter(|f| crate::use_cases::publishing::finding_effects_allowed(&round.assessment, f))
        .map(Finding::id)
        .collect();
    unpublished_findings.sort();
    let policies = policy_rows
        .into_iter()
        .map(|(policy_id, (rounds, roots))| PolicySummary {
            name: policy_names.get(&policy_id).cloned(),
            declarations: declarations.remove(&policy_id).unwrap_or_default(),
            policy_id,
            applicable_rounds: rounds.into_iter().collect(),
            comments: roots.len(),
        })
        .collect();
    Ok(SummaryView {
        round: round.clone(),
        history: history.to_vec(),
        findings,
        counts,
        policies,
        unpublished_findings,
    })
}

#[review_kit_observation::operations]
#[helper]
fn creation_is_visible(round: &Round, root: &FindingRecord, eligible: &[PlannedAction]) -> bool {
    root.created_in == round.number
        && (round.status != RoundStatus::Abandoned
            || eligible.iter().any(|action| match action {
                PlannedAction::Finding { finding, .. } => finding.id() == root.finding.id(),
                _ => false,
            }))
}

#[review_kit_observation::operations]
#[helper]
fn resolution_action(resolved: bool, provenance: ResolutionProvenance) -> PublishedAction {
    match (resolved, provenance) {
        (true, ResolutionProvenance::Acknowledged) => PublishedAction::Resolved,
        (false, ResolutionProvenance::Acknowledged) => PublishedAction::Reopened,
        (true, ResolutionProvenance::Observed) => PublishedAction::ObservedResolved,
        (false, ResolutionProvenance::Observed) => PublishedAction::ObservedReopened,
    }
}

#[review_kit_observation::operations]
#[helper]
fn resolution_is_recorded(round: &Round, finding: FindingId, resolved: bool) -> bool {
    let acknowledged = resolution_action(resolved, ResolutionProvenance::Acknowledged);
    let observed = resolution_action(resolved, ResolutionProvenance::Observed);
    round.communicated_findings.as_ref().is_some_and(|rows| {
        rows.iter().any(|row| {
            row.finding.id() == finding
                && (row.actions.contains(&acknowledged) || row.actions.contains(&observed))
        })
    })
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn retain_verified_resolutions(round: &mut Round) {
    let Some(rows) = &mut round.communicated_findings else {
        return;
    };
    for receipt in &round.verified_resolutions {
        if let Some(row) = rows.iter_mut().find(|row| row.thread == receipt.thread) {
            let action = resolution_action(receipt.resolved, receipt.provenance);
            if !row.actions.contains(&action) {
                row.actions.push(action);
            }
        }
    }
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_retained_snapshots(
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
) -> Result<(), crate::use_cases::discovery::DiscoveryConflict> {
    use crate::use_cases::discovery::DiscoveryConflict;
    let Some(summary) = snapshot
        .summaries
        .iter()
        .find(|summary| &summary.author == reviewer)
    else {
        return Ok(());
    };
    let rounds: Vec<_> = summary
        .history
        .iter()
        .chain(std::iter::once(&summary.current))
        .collect();
    for round in &rounds {
        let mut policies = BTreeSet::new();
        if round
            .review_workflow
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
            || round
                .policy_set
                .as_ref()
                .is_some_and(|name| name.trim().is_empty())
            || round
                .policy_names
                .iter()
                .any(|policy| policy.name.trim().is_empty() || !policies.insert(&policy.id))
            || round.renderer.as_ref().is_some_and(|identity| {
                identity.id.trim().is_empty()
                    || identity.input_contract.trim().is_empty()
                    || identity.fingerprint.trim().is_empty()
            })
        {
            return Err(DiscoveryConflict::InvalidRound);
        }
        for (index, receipt) in round.verified_resolutions.iter().enumerate() {
            if &receipt.subject != round.assessment.subject()
                || &receipt.reviewer != reviewer
                || receipt.round != round.number
                || round.verified_resolutions[..index]
                    .iter()
                    .any(|prior| prior.thread == receipt.thread)
            {
                return Err(DiscoveryConflict::InvalidRound);
            }
            if let Some(root) = snapshot
                .findings
                .iter()
                .find(|root| &root.author == reviewer && root.id == receipt.thread)
            {
                if !round.plans_resolution(root.finding.id(), receipt.resolved) {
                    return Err(DiscoveryConflict::InvalidRound);
                }
            } else if let Some(row) = round
                .communicated_findings
                .as_ref()
                .and_then(|rows| rows.iter().find(|row| row.thread == receipt.thread))
            {
                return Err(DiscoveryConflict::MissingPublishedFinding(row.finding.id()));
            } else {
                return Err(DiscoveryConflict::InvalidRound);
            }
        }
        let Some(rows) = &round.communicated_findings else {
            continue;
        };
        let mut findings = BTreeSet::new();
        for row in rows {
            let root = snapshot
                .findings
                .iter()
                .find(|root| &root.author == reviewer && root.finding.id() == row.finding.id())
                .ok_or(DiscoveryConflict::MissingPublishedFinding(row.finding.id()))?;
            if !findings.insert(row.finding.id())
                || row.thread != root.id
                || row.root_note != root.root_note
                || root.created_in > round.number
                || !(row.finding == root.finding
                    || rounds
                        .iter()
                        .filter(|prior| prior.number <= round.number)
                        .any(|prior| prior.assessment.findings().contains(&row.finding)))
            {
                return Err(DiscoveryConflict::InvalidFindingState(row.finding.id()));
            }
            let mut actions = Vec::new();
            for action in &row.actions {
                if actions.contains(action) {
                    return Err(DiscoveryConflict::InvalidRound);
                }
                actions.push(action.clone());
                validate_snapshot_action(round, row, root, action, snapshot, reviewer)?;
            }
            if !historical_communicated_state(row, round, root, snapshot, reviewer)
                .is_some_and(|communicated| communicated == row.finding)
            {
                return Err(DiscoveryConflict::InvalidFindingState(row.finding.id()));
            }
        }
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn validate_snapshot_action(
    round: &Round,
    row: &HistoricalFinding,
    root: &FindingRecord,
    action: &PublishedAction,
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
) -> Result<(), crate::use_cases::discovery::DiscoveryConflict> {
    use crate::use_cases::discovery::DiscoveryConflict;
    let permitted = match action {
        PublishedAction::Created => {
            root.created_in == round.number
                && round.actions.iter().any(|planned| match planned {
                    PlannedAction::Finding { finding, body } => {
                        finding.as_ref() == &root.finding && body == &root.body
                    }
                    _ => false,
                })
        }
        PublishedAction::Transition { from, to } => {
            require_snapshot_reply(round, row, &ReplyKind::Transition(*to), snapshot, reviewer)?;
            round.transitions.iter().any(|transition| {
                transition.finding == row.finding.id()
                    && transition.from == *from
                    && transition.to == *to
            })
        }
        PublishedAction::Answered { in_reply_to } => {
            require_snapshot_reply(
                round,
                row,
                &ReplyKind::Answer(in_reply_to.clone()),
                snapshot,
                reviewer,
            )?;
            true
        }
        PublishedAction::Resolved
        | PublishedAction::Reopened
        | PublishedAction::ObservedResolved
        | PublishedAction::ObservedReopened => {
            let resolved = matches!(
                action,
                PublishedAction::Resolved | PublishedAction::ObservedResolved
            );
            round.plans_resolution(row.finding.id(), resolved)
                && round.verified_resolutions.iter().any(|receipt| {
                    receipt.thread == row.thread
                        && receipt.resolved == resolved
                        && resolution_action(resolved, receipt.provenance) == *action
                })
        }
    };
    if permitted {
        Ok(())
    } else {
        Err(DiscoveryConflict::InvalidRound)
    }
}

#[review_kit_observation::operations]
#[helper]
fn require_snapshot_reply(
    round: &Round,
    row: &HistoricalFinding,
    kind: &ReplyKind,
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
) -> Result<(), crate::use_cases::discovery::DiscoveryConflict> {
    if verified_snapshot_reply(round, row, kind, snapshot, reviewer) {
        Ok(())
    } else {
        Err(crate::use_cases::discovery::DiscoveryConflict::ConflictingReply(row.finding.id()))
    }
}

#[review_kit_observation::operations]
#[helper]
fn historical_communicated_state(
    row: &HistoricalFinding,
    round: &Round,
    root: &FindingRecord,
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
) -> Option<Finding> {
    crate::use_cases::publishing::previous_finding_through(snapshot, root, reviewer, Some(round.number), |attempt, reply| {
        if attempt.number != round.number { return true; }
        let Some(ActionId { kind: ReplyKind::Transition(to), .. }) = &reply.action else { return false; };
        row.actions.iter().any(|action| matches!(action, PublishedAction::Transition { to: recorded, .. } if recorded == to))
    }).ok()
}

#[review_kit_observation::operations]
#[helper]
fn verified_snapshot_reply(
    round: &Round,
    row: &HistoricalFinding,
    kind: &ReplyKind,
    snapshot: &Snapshot,
    reviewer: &review_kit_domain::ReviewerId,
) -> bool {
    round.actions.iter().any(|action| match action {
        PlannedAction::Reply {
            finding,
            kind: planned,
            body,
        } if *finding == row.finding.id() && planned == kind => {
            let id = ActionId {
                round: round.number,
                finding: *finding,
                kind: kind.clone(),
            };
            snapshot.replies.iter().any(|reply| {
                &reply.author == reviewer
                    && reply.thread == row.thread
                    && reply.action.as_ref() == Some(&id)
                    && &reply.body == body
            })
        }
        _ => false,
    })
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn retain_communicated_findings(
    round: &mut Round,
    history: &[Round],
    snapshot: &Snapshot,
) -> Result<(), PlanError> {
    let view = summary_view(round, history, snapshot)?;
    round.communicated_findings = Some(
        view.findings
            .into_iter()
            .map(|row| HistoricalFinding {
                finding: row.finding,
                root_note: row.root.root_note,
                thread: row.root.id,
                actions: row.actions,
            })
            .collect(),
    );
    Ok(())
}
