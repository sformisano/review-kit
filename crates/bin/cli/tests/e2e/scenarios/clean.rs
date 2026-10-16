use super::super::fixture::{authored_discussions, authored_notes};
use super::super::{
    assessments, controls,
    fixture::{CliOptions, Fixture, read_json, summary_state},
};
use super::{assert_round, check, plan, publish, repeat};
use anyhow::Result;
use serde_json::{Value, json};

pub fn clean_blocked_permissions_and_controls(mut fixture: Fixture) -> Result<()> {
    let clean = fixture.manifest["mrs"]["clean"].clone();
    let worktree = fixture.worktree("clean")?;
    let (run, draft) = fixture.prepare("clean", &worktree, &clean, CliOptions::default())?;
    run.write_draft(&assessments::ready(draft))?;
    check(&mut fixture, "clean-check", &run, None)?;
    fixture.accept("clean-assessment", &run, CliOptions::default())?;
    plan(&mut fixture, "clean-plan", &run, CliOptions::default())?;
    publish(
        &mut fixture,
        "clean-publish",
        &run,
        "completed",
        CliOptions::default(),
    )?;
    let (summaries, roots) = fixture.owned(&fixture.observe("clean", &clean)?);
    assert_eq!(summaries.len(), 1);
    assert!(roots.is_empty());
    assert_round(&summaries[0], 1, &[]);
    repeat(
        &mut fixture,
        "clean-repeat",
        &run,
        &clean,
        1,
        CliOptions::default(),
    )?;
    controls::authority_and_abandonment(&mut fixture, &run.dir)?;

    let mr = fixture.extra_mr("blocked")?;
    let worktree = fixture.worktree("blocked")?;
    let (blocked, draft) = fixture.prepare("blocked", &worktree, &mr, CliOptions::default())?;
    blocked.write_draft(&assessments::blocked(draft))?;
    check(&mut fixture, "blocked-check", &blocked, None)?;
    fixture.accept("blocked-assessment", &blocked, CliOptions::default())?;
    let (value, result) = publish(
        &mut fixture,
        "blocked-publish",
        &blocked,
        "blocked",
        CliOptions::default(),
    )?;
    assert_eq!(result["round"], 1);
    assert_eq!(value["settlement"]["outcome"], "published");
    let (summaries, roots) = fixture.owned(&fixture.observe("blocked", &mr)?);
    assert_eq!(summaries.len(), 1);
    assert!(roots.is_empty());
    assert_eq!(summary_state(&summaries[0])["current"]["status"], "blocked");
    let summary_id = summaries[0]["id"].clone();
    repeat(
        &mut fixture,
        "blocked-repeat",
        &blocked,
        &mr,
        1,
        CliOptions::default(),
    )?;
    // The evidence becomes available: a new run completes the blocked round.
    let (unblocked, draft) = fixture.prepare("unblocked", &worktree, &mr, CliOptions::default())?;
    unblocked.write_draft(&assessments::ready(draft))?;
    check(&mut fixture, "unblocked-check", &unblocked, None)?;
    fixture.accept("unblocked-assessment", &unblocked, CliOptions::default())?;
    let (_, result) = publish(
        &mut fixture,
        "unblocked-publish",
        &unblocked,
        "completed",
        CliOptions::default(),
    )?;
    assert_eq!(result["round"], 1);
    let (summaries, roots) = fixture.owned(&fixture.observe("unblocked", &mr)?);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["id"], summary_id);
    assert!(roots.is_empty());
    assert_round(&summaries[0], 1, &[]);

    let mr = fixture.extra_mr("read-api")?;
    let token = fixture.read_only_token()?;
    let read_only = CliOptions {
        token: Some(&token),
        ..Default::default()
    };
    let worktree = fixture.worktree("read-api")?;
    let (run, draft) = fixture.prepare("read-api", &worktree, &mr, read_only)?;
    run.write_draft(&assessments::ready(draft))?;
    fixture.accept("read-api-assessment", &run, read_only)?;
    let before = fixture.observe("read-api-before", &mr)?;
    plan(&mut fixture, "read-api-plan", &run, read_only)?;
    let (value, result) = publish(
        &mut fixture,
        "read-api-publish",
        &run,
        "not_started",
        CliOptions {
            expected: 7,
            ..read_only
        },
    )?;
    assert_eq!(value["status"], "unfinished");
    assert_eq!(result["verified"], json!([]));
    assert_eq!(
        result["stop"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["code"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["write_rejected"]
    );
    let journal = read_json(value["progress"].as_str().unwrap())?;
    assert_eq!(journal["unresolved"], Value::Null);
    // A definite rejection of the first write restores the earlier binding.
    assert_eq!(journal["coverage_binding"], Value::Null);
    assert_eq!(run.manifest()?["status"], "open");
    let after = fixture.observe("read-api-after", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    assert!(authored_notes(&after).is_empty());
    fixture.finish()
}
