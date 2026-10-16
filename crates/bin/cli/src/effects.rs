use std::str::FromStr;

use review_kit_application::publication::{Effect, Effects};
use serde::{Deserialize, Serialize};

/// External effect names are decoded once before application authority checks.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectName {
    CreateSummary,
    UpdateSummary,
    Finding,
    Reply,
    Resolve,
    Reopen,
}

impl From<EffectName> for Effect {
    fn from(value: EffectName) -> Self {
        match value {
            EffectName::CreateSummary => Self::CreateSummary,
            EffectName::UpdateSummary => Self::UpdateSummary,
            EffectName::Finding => Self::Finding,
            EffectName::Reply => Self::Reply,
            EffectName::Resolve => Self::Resolve,
            EffectName::Reopen => Self::Reopen,
        }
    }
}

/// Every effect in the order command output lists them.
const ALL: [Effect; 6] = [
    Effect::CreateSummary,
    Effect::UpdateSummary,
    Effect::Finding,
    Effect::Reply,
    Effect::Resolve,
    Effect::Reopen,
];

/// The external name of one effect, as configuration and `--effects` spell it.
pub fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::CreateSummary => "create_summary",
        Effect::UpdateSummary => "update_summary",
        Effect::Finding => "finding",
        Effect::Reply => "reply",
        Effect::Resolve => "resolve",
        Effect::Reopen => "reopen",
    }
}

/// The external names of a set of effects, in `ALL` order.
pub fn effect_names(effects: &Effects) -> Vec<&'static str> {
    ALL.into_iter()
        .filter(|effect| effects.contains(*effect))
        .map(effect_name)
        .collect()
}

#[derive(Debug, Clone)]
pub struct EffectSelection(pub Effects);

/// effects must be review, none, or a comma-separated list of create_summary, update_summary, finding, reply, resolve, reopen
#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub struct InvalidEffects;

impl FromStr for EffectSelection {
    type Err = InvalidEffects;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let effects = match value {
            "review" => Effects::review(),
            "none" => Effects::none(),
            _ => {
                let mut parsed = Vec::new();
                for part in value.split(',') {
                    parsed.push(match part {
                        "create_summary" => Effect::CreateSummary,
                        "update_summary" => Effect::UpdateSummary,
                        "finding" => Effect::Finding,
                        "reply" => Effect::Reply,
                        "resolve" => Effect::Resolve,
                        "reopen" => Effect::Reopen,
                        _ => return Err(InvalidEffects),
                    });
                }
                Effects::new(parsed)
            }
        };
        Ok(Self(effects))
    }
}
