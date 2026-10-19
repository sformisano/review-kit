//! Conversation freshness on a real server: activity by another person and a
//! project access token bot, copied Review Kit markers and title and
//! description edits after capture do not stop publication, which writes only
//! its planned effects and leaves that activity untouched. A reply in the
//! reviewer's finding thread or a resolution change there still stops it
//! before any write.

use super::super::fixture::{Api, authored_notes};
use super::super::{
    assessments,
    fixture::{CliOptions, Fixture},
    source::{self, AUDIT_ORIGINAL, AUDIT_REPLACEMENT, QUANTITY_DERIVE},
    workflow::{Run, first_code},
};
use super::{plan, publish};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

/// The merge request's API prefix.
fn prefix(fixture: &Fixture, mr: &Value) -> String {
    format!(
        "projects/{}/merge_requests/{}",
        fixture.manifest["project_id"], mr["iid"]
    )
}

/// A text position on an added line of the current head.
fn position(fixture: &Fixture, mr: &Value, path: &str, line: usize) -> Result<Value> {
    let refs = fixture.author.get(&prefix(fixture, mr))?["diff_refs"].clone();
    Ok(
        json!({"position_type": "text", "base_sha": refs["base_sha"],
        "start_sha": refs["start_sha"], "head_sha": refs["head_sha"],
        "old_path": path, "new_path": path, "new_line": line}),
    )
}

/// Every non-system note whose author is not the reviewer, without positions.
fn foreign(fixture: &Fixture, observed: &Value) -> Vec<Value> {
    let reviewer = &fixture.manifest["reviewer"]["id"];
    authored_notes(observed)
        .into_iter()
        .filter(|note| note["author"]["id"] != *reviewer)
        .collect()
}

/// Every non-system note of the reviewer, without positions.
fn reviewer_notes(fixture: &Fixture, observed: &Value) -> Vec<Value> {
    let reviewer = &fixture.manifest["reviewer"]["id"];
    authored_notes(observed)
        .into_iter()
        .filter(|note| note["author"]["id"] == *reviewer)
        .collect()
}

/// The observed discussion with `id`.
fn discussion<'a>(observed: &'a Value, id: &Value) -> Result<&'a Value> {
    observed["discussions"]
        .as_array()
        .context("observed discussions")?
        .iter()
        .find(|discussion| discussion["id"] == *id)
        .context("observed discussion")
}

/// Post a reply in a discussion and return its note ID.
fn reply(api: &Api, prefix: &str, discussion: &Value, body: &str) -> Result<Value> {
    let id = discussion.as_str().context("discussion ID")?;
    Ok(api.post(
        &format!("{prefix}/discussions/{id}/notes"),
        &json!({"body": body}),
    )?["id"]
        .clone())
}

/// Rejected publication of `run`: exit 3, `conversation_changed` listing
/// exactly `changes`, no provider write and no recorded attempt.
fn refused(
    fixture: &mut Fixture,
    label: &str,
    run: &Run,
    mr: &Value,
    changes: Value,
) -> Result<()> {
    let before = fixture.observe(&format!("{label}-before"), mr)?;
    for (step, arguments) in [
        ("plan", vec!["publish", "--effects", "review", "--plan"]),
        ("publish", vec!["publish", "--effects", "review"]),
    ] {
        let value = fixture.rkc(
            &format!("{label}-{step}"),
            &run.worktree,
            &arguments,
            CliOptions {
                expected: 3,
                ..Default::default()
            },
        )?;
        ensure!(
            first_code(&value) == "conversation_changed",
            "{label}-{step}: {value}"
        );
        ensure!(
            value["diagnostics"][0]["changes"] == changes,
            "{label}-{step} lists {} instead of {changes}",
            value["diagnostics"][0]["changes"]
        );
    }
    ensure!(run.manifest()?["publication_attempted"] == false);
    let after = fixture.observe(&format!("{label}-after"), mr)?;
    ensure!(
        authored_notes(&after) == authored_notes(&before),
        "{label}: a refused publication changed provider notes"
    );
    let stopped = fixture.rkc(
        &format!("{label}-stop"),
        &run.worktree,
        &[
            "run",
            "stop",
            "--reason",
            "The reviewer's own thread changed after capture.",
        ],
        CliOptions::default(),
    )?;
    ensure!(stopped["status"] == "stopped", "{label}: {stopped}");
    Ok(())
}

/// Prepare a run that repeats `previous` and answers `questions` as
/// `(finding, note)` pairs, then accept it.
fn answering(
    fixture: &mut Fixture,
    label: &str,
    worktree: &Path,
    mr: &Value,
    previous: &Run,
    questions: &[(u64, &Value)],
) -> Result<Run> {
    let (run, draft) = fixture.prepare(label, worktree, mr, CliOptions::default())?;
    let mut value = assessments::equivalent(&previous.accepted()?, draft);
    value["answers"] = json!(
        questions
            .iter()
            .map(
                |(finding, note)| json!({"finding": finding, "in_reply_to": note.to_string(),
                "body": "The current revision still has this defect; the finding stays open."})
            )
            .collect::<Vec<_>>()
    );
    run.write_draft(&value)?;
    fixture.accept(&format!("{label}-assessment"), &run, CliOptions::default())?;
    Ok(run)
}

pub fn conversation_freshness_scope(mut fixture: Fixture) -> Result<()> {
    let commenter = fixture.commenter()?;
    let bot = fixture.project_bot()?;
    let bot_user = fixture
        .author
        .get(&format!("users/{}", fixture.manifest["bot"]["id"]))?;
    println!(
        "Project bot user: bot={} username={}",
        bot_user["bot"], bot_user["username"]
    );
    let mr = fixture.manifest["mrs"]["review"].clone();
    let path = prefix(&fixture, &mr);
    let notes = format!("{path}/notes");
    let threads = format!("{path}/discussions");
    let audit_line = source::line_of(&source::storage_audit(), AUDIT_ORIGINAL);
    let quantity_line = source::line_of(&source::quantity_input(), QUANTITY_DERIVE);
    let worktree = fixture.worktree("freshness")?;

    // First round: everything below lands after capture and outside owned threads.
    let (initial, draft) =
        fixture.prepare("freshness-initial", &worktree, &mr, CliOptions::default())?;
    let mut value = assessments::initial(draft);
    value["new_findings"][1]["suggestion"]["original"] = json!([AUDIT_ORIGINAL, "    Ok(())"]);
    value["new_findings"][1]["suggestion"]["replacement"] =
        json!([AUDIT_REPLACEMENT, "    Ok(())"]);
    initial.write_draft(&value)?;
    fixture.accept(
        "freshness-initial-assessment",
        &initial,
        CliOptions::default(),
    )?;
    commenter.post(
        &notes,
        &json!({"body": "Could the optional audit failure be logged?"}),
    )?;
    let bot_note = bot.post(
        &notes,
        &json!({"body": "Automated review: no blocking issues detected."}),
    )?["id"]
        .clone();
    let foreign_thread = commenter.post(
        &threads,
        &json!({"body": "Please explain this added audit line.",
            "position": position(&fixture, &mr, "src/storage.rs", audit_line)?}),
    )?["id"]
        .clone();
    reply(
        &bot,
        &path,
        &foreign_thread,
        "Automated suggestion: document the best-effort audit.",
    )?;
    commenter.put(
        &format!("{threads}/{}", foreign_thread.as_str().unwrap()),
        &json!({"resolved": true}),
    )?;
    let copied = commenter.post(
        &threads,
        &json!({"body": "Copied review note.\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":1} -->",
            "position": position(&fixture, &mr, "src/quantity.rs", quantity_line)?}),
    )?["id"]
        .clone();
    fixture.author.put(
        &path,
        &json!({"title": "Add JSON quantity input and optional audit output (revised title)",
            "description": "Edited after the review run started."}),
    )?;
    let before = fixture.observe("freshness-initial-foreign", &mr)?;
    let planned = plan(
        &mut fixture,
        "freshness-initial-plan",
        &initial,
        CliOptions::default(),
    )?;
    let (published, result) = publish(
        &mut fixture,
        "freshness-initial-publish",
        &initial,
        "completed",
        CliOptions::default(),
    )?;
    ensure!(published["settlement"]["outcome"] == "published");
    ensure!(result["round"] == 1);
    ensure!(
        result["planned"] == planned["run"]["outcome"]["plan"]["actions"],
        "publication wrote other effects than its plan"
    );
    let after = fixture.observe("freshness-initial", &mr)?;
    ensure!(
        foreign(&fixture, &after) == foreign(&fixture, &before),
        "publication changed a foreign note"
    );
    ensure!(after["mr"]["title"] == before["mr"]["title"]);
    ensure!(after["mr"]["description"] == before["mr"]["description"]);
    let (summaries, roots) = fixture.owned(&after);
    ensure!(summaries.len() == 1);
    ensure!(roots.keys().copied().collect::<Vec<_>>() == [1, 2]);
    ensure!(
        reviewer_notes(&fixture, &after).len() == 3,
        "one summary and two finding roots"
    );
    ensure!(
        roots.values().all(|root| root["id"] != copied),
        "a copied finding marker is not an owned root"
    );
    let copied_thread = discussion(&after, &copied)?;
    ensure!(
        copied_thread["notes"]
            .as_array()
            .context("copied notes")?
            .len()
            == 1
    );
    ensure!(copied_thread["notes"][0]["author"]["id"] == fixture.manifest["commenter"]["id"]);
    let summary_id = summaries[0]["id"].clone();
    let finding_thread = roots[&1]["id"].clone();
    let audit_thread = roots[&2]["id"].clone();

    // Second round: owned threads exist; activity outside them still does not block.
    let question = reply(
        &fixture.author,
        &path,
        &finding_thread,
        "Does the revised decoding reject zero now?",
    )?;
    let answered = answering(
        &mut fixture,
        "freshness-answer",
        &worktree,
        &mr,
        &initial,
        &[(1, &question)],
    )?;
    ensure!(answered.started["carried_findings"] == 2);
    bot.post(
        &notes,
        &json!({"body": "Automated review: rerun after the description edit."}),
    )?;
    bot.put(
        &format!("{notes}/{bot_note}"),
        &json!({"body": "Automated review: no blocking issues detected (edited)."}),
    )?;
    reply(
        &commenter,
        &path,
        &foreign_thread,
        "Reopening: the comment is still missing.",
    )?;
    commenter.put(
        &format!("{threads}/{}", foreign_thread.as_str().unwrap()),
        &json!({"resolved": false}),
    )?;
    commenter.post(
        &notes,
        &json!({"body": "Copied summary.\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->"}),
    )?;
    fixture.author.put(
        &path,
        &json!({"description": "Edited again after the follow-up run started."}),
    )?;
    let before = fixture.observe("freshness-answer-foreign", &mr)?;
    let planned = plan(
        &mut fixture,
        "freshness-answer-plan",
        &answered,
        CliOptions::default(),
    )?;
    let (_, result) = publish(
        &mut fixture,
        "freshness-answer-publish",
        &answered,
        "completed",
        CliOptions::default(),
    )?;
    ensure!(result["round"] == 2);
    ensure!(
        result["planned"] == planned["run"]["outcome"]["plan"]["actions"],
        "publication wrote other effects than its plan"
    );
    let after = fixture.observe("freshness-answer", &mr)?;
    ensure!(
        foreign(&fixture, &after) == foreign(&fixture, &before),
        "publication changed a foreign note"
    );
    ensure!(after["mr"]["description"] == before["mr"]["description"]);
    let (summaries, _) = fixture.owned(&after);
    ensure!(summaries.len() == 1 && summaries[0]["id"] == summary_id);
    let mine_before = reviewer_notes(&fixture, &before);
    let mine_after = reviewer_notes(&fixture, &after);
    ensure!(
        mine_after.len() == mine_before.len() + 1,
        "the answer is the only new reviewer note"
    );
    let answers: Vec<_> = discussion(&after, &finding_thread)?["notes"]
        .as_array()
        .context("finding notes")?
        .iter()
        .filter(|note| note["author"]["id"] == fixture.manifest["reviewer"]["id"])
        .filter_map(|note| super::super::fixture::marker(note["body"].as_str().unwrap()))
        .collect();
    ensure!(
        answers.contains(
            &json!({"kind": "answer", "finding": 1, "round": 2, "note": question.to_string()})
        ),
        "the answer reply is in the finding thread"
    );

    // A reply by the author inside the reviewer's finding thread blocks.
    let second = reply(
        &commenter,
        &path,
        &audit_thread,
        "Should the audit failure at least be counted?",
    )?;
    let owned_reply = answering(
        &mut fixture,
        "freshness-owned-reply",
        &worktree,
        &mr,
        &answered,
        &[(2, &second)],
    )?;
    let late = reply(
        &fixture.author,
        &path,
        &finding_thread,
        "One more case: negative input.",
    )?;
    refused(
        &mut fixture,
        "freshness-owned-reply",
        &owned_reply,
        &mr,
        json!([{"kind": "note_added", "discussion": finding_thread, "note": late.to_string()}]),
    )?;

    // A resolution change on the reviewer's thread blocks.
    let owned_resolution = answering(
        &mut fixture,
        "freshness-owned-resolution",
        &worktree,
        &mr,
        &answered,
        &[(2, &second)],
    )?;
    fixture.author.put(
        &format!("{threads}/{}", audit_thread.as_str().unwrap()),
        &json!({"resolved": true}),
    )?;
    refused(
        &mut fixture,
        "freshness-owned-resolution",
        &owned_resolution,
        &mr,
        json!([{"kind": "resolution_changed", "discussion": audit_thread}]),
    )?;
    fixture.author.put(
        &format!("{threads}/{}", audit_thread.as_str().unwrap()),
        &json!({"resolved": false}),
    )?;

    // A fresh run assesses the current threads and publishes.
    let current = answering(
        &mut fixture,
        "freshness-current",
        &worktree,
        &mr,
        &answered,
        &[(1, &late), (2, &second)],
    )?;
    let before = fixture.observe("freshness-current-foreign", &mr)?;
    let (_, result) = publish(
        &mut fixture,
        "freshness-current-publish",
        &current,
        "completed",
        CliOptions::default(),
    )?;
    ensure!(result["round"] == 3);
    let after = fixture.observe("freshness-current", &mr)?;
    ensure!(
        foreign(&fixture, &after) == foreign(&fixture, &before),
        "publication changed a foreign note"
    );
    ensure!(
        reviewer_notes(&fixture, &after).len() == reviewer_notes(&fixture, &before).len() + 2,
        "two answers are the only new reviewer notes"
    );
    fixture.finish()
}
