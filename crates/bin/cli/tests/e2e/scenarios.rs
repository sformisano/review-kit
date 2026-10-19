use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, path::Path};

use super::{
    assessments::{self, PRIVATE_CONTEXT},
    fixture::{CliOptions, Fixture, authored_discussions, authored_notes, summary_state},
    workflow::Run,
};

#[path = "scenarios/clean.rs"]
mod clean;
#[path = "scenarios/freshness.rs"]
mod freshness;
#[path = "scenarios/initial.rs"]
mod initial;
#[path = "scenarios/invalid.rs"]
mod invalid;
#[path = "scenarios/outdated.rs"]
mod outdated;
#[path = "scenarios/render_profiles.rs"]
mod render_profiles;
pub use clean::clean_blocked_permissions_and_controls;
pub use freshness::conversation_freshness_scope;
pub use initial::initial_fixed_and_recurrent_review;
pub use invalid::offline_invalid_inputs_and_private_bodies;
pub use outdated::outdated_position_followup;
pub use render_profiles::custom_profile_review;

/// Checks the run's assessment draft without a credential and verifies that
/// the rendered previews contain neither the private local context nor a token.
fn check(fixture: &mut Fixture, label: &str, run: &Run, config: Option<&Path>) -> Result<Value> {
    let checked = fixture.rkc(
        label,
        &run.worktree,
        &["check"],
        CliOptions {
            config,
            offline: true,
            ..Default::default()
        },
    )?;
    assert_eq!(checked["status"], "valid", "{checked}");
    assert_eq!(checked["check"]["valid"], true);
    for entry in fs::read_dir(run.dir.join("rendered"))? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "md")
        {
            let body = fs::read_to_string(entry.path())?;
            assert!(!body.contains(PRIVATE_CONTEXT));
            for token in &fixture.tokens {
                assert!(!body.contains(token));
            }
        }
    }
    Ok(checked)
}

/// Plans the selected run's accepted chain with every review effect.
fn plan(fixture: &mut Fixture, label: &str, run: &Run, options: CliOptions<'_>) -> Result<Value> {
    let planned = fixture.rkc(
        label,
        &run.worktree,
        &["publish", "--effects", "review", "--plan"],
        options,
    )?;
    assert_eq!(planned["status"], "planned", "{planned}");
    assert_eq!(planned["run"]["outcome"]["kind"], "planned");
    assert_eq!(planned["provider_writes"], false);
    Ok(planned)
}

fn publish(
    fixture: &mut Fixture,
    label: &str,
    run: &Run,
    status: &str,
    options: CliOptions<'_>,
) -> Result<(Value, Value)> {
    let value = fixture.rkc(
        label,
        &run.worktree,
        &["publish", "--effects", "review"],
        options,
    )?;
    assert_eq!(value["run_id"], run.id.as_str());
    assert_eq!(value["run"]["outcome"]["kind"], "published", "{value}");
    let result = value["run"]["outcome"]["result"].clone();
    assert_eq!(result["status"], status, "{value}");
    Ok((value, result))
}

/// Repeats the latest review of `mr` in a new run of `worktree`. The same
/// coverage, decision and carried findings at the same head communicate
/// nothing, keep the round and settle the run as `no_changes`; publishing the
/// settled run again reports `already_settled` without reopening it.
fn repeat(
    fixture: &mut Fixture,
    label: &str,
    previous: &Run,
    mr: &Value,
    round: u64,
    options: CliOptions<'_>,
) -> Result<Run> {
    let before = fixture.observe(&format!("{label}-before"), mr)?;
    let (run, draft) = fixture.prepare(label, &previous.worktree, mr, options)?;
    run.write_draft(&assessments::equivalent(&previous.accepted()?, draft))?;
    fixture.accept(&format!("{label}-assessment"), &run, options)?;
    let (value, result) = publish(fixture, label, &run, "unchanged", options)?;
    assert_eq!(value["settlement"]["outcome"], "no_changes");
    assert_eq!(result["round"], round);
    assert_eq!(result["attempted"], json!([]));
    assert_eq!(result["verified"], json!([]));
    let settled = fixture.rkc(
        &format!("{label}-settled"),
        &run.worktree,
        &["publish", "--effects", "review"],
        options,
    )?;
    assert_eq!(settled["status"], "already_settled");
    assert_eq!(settled["completion"], "no_changes");
    let after = fixture.observe(&format!("{label}-after"), mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    Ok(run)
}

fn assert_round(note: &Value, number: u64, history: &[u64]) {
    let state = summary_state(note);
    assert_eq!(state["current"]["number"], number);
    assert_eq!(state["current"]["status"], "completed");
    let rows = state["history"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        history
    );
    assert!(rows.iter().all(|row| row["status"] == "completed"));
}

fn assert_counts(note: &Value, open: u64, fixed: u64) {
    let mut recap = format!("Round {}", summary_state(note)["current"]["number"]);
    for (count, state) in [(open, "open"), (fixed, "fixed")] {
        if count > 0 {
            recap.push_str(&format!(" · {count} {state}"));
        }
    }
    let body = note["body"].as_str().unwrap();
    assert!(body.lines().any(|line| line == recap), "{recap}: {body}");
}
