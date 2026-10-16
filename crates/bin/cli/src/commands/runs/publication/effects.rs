//! The effect set publication may use, and which planned writes fall outside it.
//!
//! Publication sends a summary write that starts the round, then each planned
//! action, then a summary write that completes the round. A blocked round with
//! no finding effects finishes with its starting summary; an unchanged plan
//! sends nothing. Each write needs
//! its effect in the requested set, the trusted configured authority and the
//! provider's permissions. A round that continues on the provider is also held
//! to the ceiling retained when its first write was authorized; without a
//! retained ceiling for such a round, nothing is permitted. The first write of
//! a new round records its ceiling. These rules mirror the publication engine,
//! which re-checks every write and decides the actual outcome.

use review_kit_application::{
    publication::{Effect, Effects, PlannedAction, RoundStatus, Snapshot},
    use_cases::publishing::PublicationProgress,
};
use review_kit_domain::{ReviewDecision, ReviewerId};
use serde_json::{Value, json};

use crate::effects::{effect_name, effect_names};

/// The authority inputs, read before planning or after publication.
pub struct Authority<'a> {
    pub requested: &'a Effects,
    pub configured: &'a Effects,
    pub progress: &'a PublicationProgress,
    /// The provider snapshot read before the first write.
    pub snapshot: &'a Snapshot,
    pub reviewer: &'a ReviewerId,
}

/// The writes of one round, in the engine's order.
pub struct RoundWrites<'a> {
    pub round: u64,
    pub actions: &'a [PlannedAction],
    pub unchanged: bool,
    pub decision: ReviewDecision,
}

/// A round's retained effect ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ceiling {
    /// The round starts with this publication; its first authorized write records the ceiling.
    New,
    Retained(Effects),
    /// The provider shows the round unfinished, but the journal retains no ceiling for it.
    Unknown,
}

/// The `effects` output object: the effective set and each planned write's place in it.
///
/// `actions[].index` is the action's position in the plan's `actions`
/// (publication: `planned`).
#[review_kit_observation::operations]
#[helper]
pub fn report(authority: &Authority<'_>, writes: &RoundWrites<'_>) -> Value {
    let reachable = authority
        .requested
        .intersection(authority.configured)
        .intersection(&authority.snapshot.permissions);
    let ceiling = ceiling(authority, writes.round);
    let (effective, round_ceiling) = match &ceiling {
        Ceiling::New => (reachable, Value::Null),
        Ceiling::Retained(effects) => (
            reachable.intersection(effects),
            json!({"round": writes.round, "effects": effect_names(effects), "retained": true}),
        ),
        Ceiling::Unknown => (
            Effects::none(),
            json!({"round": writes.round, "effects": [], "retained": false}),
        ),
    };
    let summary = summary_writes(authority, writes);
    let mut outside = Vec::new();
    let summary_writes: Vec<Value> = summary
        .iter()
        .map(|(write, effect)| {
            let within = effective.contains(*effect);
            if !within {
                outside.push(*effect);
            }
            json!({"write": write, "effect": effect_name(*effect), "within": within})
        })
        .collect();
    let actions: Vec<Value> = writes
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let effect = action.effect();
            let within = effective.contains(effect);
            if !within {
                outside.push(effect);
            }
            json!({
                "index": index,
                "finding": finding(action),
                "effect": effect_name(effect),
                "within": within,
            })
        })
        .collect();
    json!({
        "requested": effect_names(authority.requested),
        "configured_authority": effect_names(authority.configured),
        "provider_permissions": effect_names(&authority.snapshot.permissions),
        "round_ceiling": round_ceiling,
        "effective": effect_names(&effective),
        "summary_writes": summary_writes,
        "actions": actions,
        "outside": effect_names(&Effects::new(outside)),
    })
}

/// The ceiling publication applies to `round`.
#[review_kit_observation::operations]
#[helper]
fn ceiling(authority: &Authority<'_>, round: u64) -> Ceiling {
    match &authority.progress.authorization {
        Some(grant) if grant.round == round => Ceiling::Retained(grant.effects.clone()),
        _ if authority.snapshot.summaries.iter().any(|summary| {
            &summary.author == authority.reviewer
                && summary.current.number == round
                && matches!(
                    summary.current.status,
                    RoundStatus::Pending | RoundStatus::Partial
                )
        }) =>
        {
            Ceiling::Unknown
        }
        _ => Ceiling::New,
    }
}

/// The round's summary writes: a creation when the reviewer has no summary yet,
/// otherwise an update, then the completing update for unfinished finding effects.
#[review_kit_observation::operations]
#[helper]
fn summary_writes(
    authority: &Authority<'_>,
    writes: &RoundWrites<'_>,
) -> Vec<(&'static str, Effect)> {
    if writes.unchanged {
        return Vec::new();
    }
    let exists = authority
        .snapshot
        .summaries
        .iter()
        .any(|summary| &summary.author == authority.reviewer);
    let mut summary = vec![(
        "start",
        if exists {
            Effect::UpdateSummary
        } else {
            Effect::CreateSummary
        },
    )];
    let unfinished = authority.snapshot.summaries.iter().any(|summary| {
        &summary.author == authority.reviewer
            && summary.current.number == writes.round
            && matches!(
                summary.current.status,
                RoundStatus::Pending | RoundStatus::Partial
            )
    });
    if writes.decision != ReviewDecision::Blocked || !writes.actions.is_empty() || unfinished {
        summary.push(("final", Effect::UpdateSummary));
    }
    summary
}

/// The finding a planned action concerns.
#[review_kit_observation::operations]
#[helper]
fn finding(action: &PlannedAction) -> u64 {
    match action {
        PlannedAction::Finding { finding, .. } => finding.id().get(),
        PlannedAction::Reply { finding, .. } | PlannedAction::SetResolved { finding, .. } => {
            finding.get()
        }
    }
}
