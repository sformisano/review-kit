use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

use super::fixture::{
    CliOptions, Fixture, authored_discussions, authored_notes, private_file, read_json,
    summary_state, write_json,
};
use super::workflow::{Run, first_code};

/// Reuse a completed clean assessment's policy rows and judgments at the same head.
fn reuse(
    _fixture: &mut Fixture,
    _label: &str,
    prior: &Run,
    run: &Run,
    _options: CliOptions<'_>,
) -> Result<()> {
    let assessment = prior.accepted()?;
    let mut draft = run.draft()?;
    for field in [
        "policy_set",
        "policies",
        "decision",
        "depth",
        "scope",
        "limitation",
        "local_context",
    ] {
        draft[field] = assessment[field].clone();
    }
    run.write_draft(&draft)?;
    Ok(())
}

/// Authority, ownership and abandonment controls on a separate merge request
/// at the clean review's committed head, reusing the judgments accepted in
/// the run stored at `prior`.
pub fn authority_and_abandonment(fixture: &mut Fixture, prior: &Path) -> Result<()> {
    let prior = Run::load(prior)?;
    let assessment = prior.accepted()?;
    let clean = &fixture.manifest["mrs"]["clean"];
    ensure!(
        assessment["revision"]["head"] == clean["head"]
            && prior.context()?["snapshot"]["subject"]["request"] == clean["iid"]
            && assessment["decision"] == "ready"
            && assessment["new_findings"]
                .as_array()
                .context("new findings")?
                .is_empty()
            && assessment["findings"]
                .as_array()
                .context("findings")?
                .is_empty(),
        "controls require the completed clean fixture review"
    );
    let mr = fixture.extra_mr("authority")?;
    let directory = fixture.directory.join("authority");
    let mut config = read_json(fixture.manifest["config"].as_str().context("config path")?)?;
    let full = directory.join("full-config.json");
    write_json(&full, &config)?;
    let empty = directory.join("empty-config.json");
    config["gitlab"]["authority"] = json!([]);
    write_json(&empty, &config)?;
    let create = directory.join("create-config.json");
    config["gitlab"]["authority"] = json!(["create_summary"]);
    write_json(&create, &config)?;
    let update = directory.join("update-config.json");
    config["gitlab"]["authority"] = json!(["update_summary"]);
    write_json(&update, &config)?;
    let with = |config| CliOptions {
        config: Some(config),
        ..Default::default()
    };
    let worktree = fixture.worktree("authority")?;
    let run = fixture.start("control-start", &worktree, &mr, with(&empty))?;
    reuse(fixture, "control", &prior, &run, with(&empty))?;
    let checked = fixture.rkc("control-check", &worktree, &["check"], with(&empty))?;
    assert_eq!(checked["status"], "valid");
    fixture.accept("control-assessment", &run, with(&empty))?;
    let review = ["publish", "--effects", "review"];
    let plan = ["publish", "--effects", "review", "--plan"];
    let planned = fixture.rkc(
        "control-plan-without-authority",
        &worktree,
        &plan,
        with(&empty),
    )?;
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["run"]["outcome"]["kind"], "planned");
    assert!(authored_notes(&fixture.observe("control-plan", &mr)?).is_empty());
    let denied = fixture.rkc(
        "control-denied",
        &worktree,
        &review,
        CliOptions {
            expected: 4,
            ..with(&empty)
        },
    )?;
    assert_eq!(denied["run"]["outcome"]["result"]["status"], "denied");
    assert_eq!(denied["run"]["outcome"]["result"]["attempted"], json!([]));
    assert_eq!(denied["status"], "unfinished");
    assert_eq!(
        read_json(denied["progress"].as_str().context("journal path")?)?["coverage_binding"],
        Value::Null
    );
    assert!(authored_notes(&fixture.observe("control-denied", &mr)?).is_empty());
    let author_token =
        fs::read_to_string(fixture.manifest["author"]["token_file"].as_str().unwrap())?;
    let wrong = fixture.rkc(
        "control-wrong-reviewer",
        &worktree,
        &plan,
        CliOptions {
            token: Some(&author_token),
            expected: 4,
            ..with(&full)
        },
    )?;
    assert_eq!(first_code(&wrong), "wrong_reviewer");
    assert!(authored_notes(&fixture.observe("control-wrong-reviewer", &mr)?).is_empty());
    let partial = fixture.rkc(
        "control-partial",
        &worktree,
        &review,
        CliOptions {
            expected: 4,
            ..with(&create)
        },
    )?;
    let result = &partial["run"]["outcome"]["result"];
    assert_eq!(result["status"], "partial");
    assert_eq!(partial["status"], "unfinished");
    assert_eq!(result["verified"].as_array().unwrap().len(), 1);
    assert_eq!(result["attempted"].as_array().unwrap().len(), 1);
    let observed = fixture.observe("control-partial", &mr)?;
    let notes = authored_notes(&observed);
    assert_eq!(notes.len(), 1);
    let pending_note = notes[0]["id"].clone();
    let journal_path = Path::new(partial["progress"].as_str().context("journal path")?);
    let retained = fs::read(journal_path)?;
    let mut journal = read_json(journal_path)?;
    assert_eq!(journal["coverage_binding"]["run_id"], run.id.as_str());
    assert_eq!(journal["unresolved"], Value::Null);
    let summary = &result["final_snapshot"]["summaries"][0];
    let unobserved_body = format!(
        "{}\n\nUnobserved summary update.",
        summary["body"].as_str().context("summary body")?
    );
    journal["unresolved"] = json!({
        "subject": journal["subject"],
        "reviewer": journal["reviewer"],
        "renderer": partial["render_profile"],
        "phase": "legacy_ambiguous",
        "write": {
            "kind": "summary",
            "expected": summary,
            "round": summary["current"],
            "history": summary["history"],
            "body": unobserved_body,
        },
    });
    write_json(journal_path, &journal)?;
    // Another worktree's run of the same merge request cannot publish over,
    // or abandon, the owner's unknown write.
    let other = fixture.worktree("authority-followup")?;
    let followup = fixture.start("control-followup-start", &other, &mr, with(&full))?;
    reuse(fixture, "control-followup", &prior, &followup, with(&full))?;
    fixture.accept("control-followup-assessment", &followup, with(&full))?;
    let before = fixture.observe("control-unresolved-before", &mr)?;
    let pending = fixture.rkc(
        "control-unresolved-other-run",
        &other,
        &review,
        CliOptions {
            expected: 5,
            ..with(&full)
        },
    );
    let not_owner = fixture.rkc(
        "control-unresolved-other-abandon",
        &other,
        &[
            "publish",
            "--abandon",
            "--reason",
            "Exercise retained intent.",
            "--effects",
            "update_summary",
        ],
        CliOptions {
            expected: 5,
            ..with(&update)
        },
    );
    let uncertain = fixture.rkc(
        "control-unresolved-owner-abandon",
        &worktree,
        &[
            "publish",
            "--abandon",
            "--reason",
            "Exercise retained intent.",
            "--effects",
            "update_summary",
        ],
        CliOptions {
            expected: 6,
            ..with(&update)
        },
    );
    let observed_journal = read_json(journal_path);
    // Restore only the synthetic unresolved update before ordinary abandonment.
    private_file(journal_path, &retained)?;
    let pending = pending?;
    assert_eq!(
        pending["diagnostics"][0]["code"],
        "publication_pending_owner"
    );
    assert_eq!(pending["diagnostics"][0]["owner_run_id"], run.id.as_str());
    assert_eq!(first_code(&not_owner?), "no_unfinished_publication");
    let uncertain = uncertain?;
    assert_eq!(observed_journal?["unresolved"], journal["unresolved"]);
    assert_eq!(uncertain["abandon"]["result"]["status"], "uncertain");
    assert_eq!(uncertain["abandon"]["result"]["attempted"], json!([]));
    let after = fixture.observe("control-unresolved-after", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    let reason = "Close the authority-limited test round.";
    let abandoned = fixture.rkc(
        "control-abandoned",
        &worktree,
        &[
            "publish",
            "--abandon",
            "--reason",
            reason,
            "--effects",
            "update_summary",
        ],
        with(&update),
    )?;
    assert_eq!(abandoned["abandon"]["result"]["status"], "abandoned");
    assert_eq!(abandoned["settlement"]["outcome"], "abandoned");
    assert_eq!(run.manifest()?["completion"], "abandoned");
    let observed = fixture.observe("control-abandoned", &mr)?;
    let notes = authored_notes(&observed);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], pending_note);
    let state = summary_state(&notes[0]);
    assert_eq!(state["current"]["number"], 1);
    assert_eq!(state["current"]["status"], "abandoned");
    assert_eq!(state["current"]["abandonment"]["reason"], reason);
    assert_eq!(state["history"], json!([]));
    let round = fixture.start("control-new-round-start", &worktree, &mr, with(&full))?;
    reuse(fixture, "control-new-round", &prior, &round, with(&full))?;
    fixture.accept("control-new-round-assessment", &round, with(&full))?;
    let finished = fixture.rkc("control-new-round", &worktree, &review, with(&full))?;
    let result = &finished["run"]["outcome"]["result"];
    assert_eq!(result["status"], "completed");
    assert_eq!(result["round"], 2);
    assert_eq!(
        result["final_snapshot"]["summaries"][0]["history"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let before = fixture.observe("control-new-round", &mr)?;
    let notes = authored_notes(&before);
    assert_eq!(notes.len(), 1);
    let state = summary_state(&notes[0]);
    assert_eq!(state["current"]["number"], 2);
    assert_eq!(state["current"]["status"], "completed");
    assert_eq!(state["history"].as_array().unwrap().len(), 1);
    assert_eq!(state["history"][0]["number"], 1);
    assert_eq!(state["history"][0]["status"], "abandoned");
    assert_eq!(state["history"][0]["abandonment"]["reason"], reason);
    // The other worktree's run retained the conversation before the summary
    // changed. Against the current history its equivalent assessment plans no
    // write, so no write target is compared and it settles without writes; a
    // fresh equivalent run does the same.
    let stale = fixture.rkc("control-followup-stale", &other, &review, with(&full))?;
    let result = &stale["run"]["outcome"]["result"];
    assert_eq!(result["status"], "unchanged");
    assert_eq!(result["attempted"], json!([]));
    assert_eq!(stale["settlement"]["outcome"], "no_changes");
    let after = fixture.observe("control-followup-stale", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    let unchanged = fixture.start("control-unchanged-start", &other, &mr, with(&full))?;
    reuse(
        fixture,
        "control-unchanged",
        &prior,
        &unchanged,
        with(&full),
    )?;
    fixture.accept("control-unchanged-assessment", &unchanged, with(&full))?;
    let repeated = fixture.rkc("control-unchanged", &other, &review, with(&full))?;
    let result = &repeated["run"]["outcome"]["result"];
    assert_eq!(result["status"], "unchanged");
    assert_eq!(result["attempted"], json!([]));
    assert_eq!(result["verified"], json!([]));
    assert_eq!(repeated["settlement"]["outcome"], "no_changes");
    let after = fixture.observe("control-unchanged", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    let notes = authored_notes(&after);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], pending_note);
    write_json(
        directory.join("result.json"),
        &json!({"status": "passed", "mr": mr["url"], "checks": ["read_only_plan", "empty_authority", "wrong_reviewer", "partial", "unresolved_other_worktree", "abandon", "new_round", "stale_capture_unchanged", "unchanged"]}),
    )?;
    Ok(())
}
