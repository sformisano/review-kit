use super::super::fixture::{authored_discussions, authored_notes};
use super::super::{
    assessments,
    fixture::{CliOptions, Fixture, private_file, write_json},
    workflow::{Run, codes, first_code},
};
use super::{check, plan};
use anyhow::Result;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

/// Checks a complete assessment draft offline and expects a rejected result with `code`.
fn rejected(
    fixture: &mut Fixture,
    label: &str,
    run: &Run,
    arguments: &[&str],
    config: Option<&Path>,
) -> Result<Value> {
    let value = fixture.rkc(
        label,
        &run.worktree,
        arguments,
        CliOptions {
            config,
            offline: true,
            expected: 2,
            ..Default::default()
        },
    )?;
    assert_ne!(value["status"], "valid", "{value}");
    Ok(value)
}

pub fn offline_invalid_inputs_and_private_bodies(mut fixture: Fixture) -> Result<()> {
    let clean = fixture.manifest["mrs"]["clean"].clone();
    let config = fixture.directory.join("offline-config.json");
    write_json(&config, &json!({"version": 1}))?;
    let offline = CliOptions {
        config: Some(&config),
        offline: true,
        ..Default::default()
    };
    let worktree = fixture.worktree("offline")?;
    let run = fixture.start("offline-start", &worktree, &clean, CliOptions::default())?;
    let placeholder = rejected(
        &mut fixture,
        "placeholder-assessment-check",
        &run,
        &["check"],
        Some(&config),
    )?;
    assert_eq!(placeholder["status"], "invalid");
    run.write_draft(&assessments::declare_policies(run.draft()?))?;
    let draft = run.draft()?;
    let incomplete = rejected(
        &mut fixture,
        "incomplete-check",
        &run,
        &["check"],
        Some(&config),
    )?;
    let found: BTreeSet<_> = codes(&incomplete).into_iter().collect();
    assert!(
        [
            "missing_decision",
            "missing_depth",
            "missing_scope",
            "missing_policy_judgment"
        ]
        .iter()
        .all(|code| found.contains(*code)),
        "{incomplete}"
    );
    run.write_draft(&assessments::ready(draft))?;
    check(&mut fixture, "offline-check", &run, Some(&config))?;
    let valid = fs::read(run.draft_path())?;
    private_file(run.draft_path(), "{ broken")?;
    let malformed = rejected(
        &mut fixture,
        "malformed-check",
        &run,
        &["check"],
        Some(&config),
    )?;
    assert_eq!(first_code(&malformed), "malformed_draft");
    let unchanged = fixture.rkc(
        "malformed-revise",
        &worktree,
        &["revise", "--reason", "Repair the draft."],
        CliOptions {
            expected: 2,
            ..offline
        },
    )?;
    assert_eq!(unchanged["status"], "malformed_draft");
    assert_eq!(fs::read(run.draft_path())?, b"{ broken");
    let regenerated = fixture.rkc(
        "malformed-regenerate",
        &worktree,
        &[
            "revise",
            "--reason",
            "Rebuild the draft from accepted state.",
            "--regenerate",
        ],
        offline,
    )?;
    assert_eq!(regenerated["status"], "prepared");
    assert_eq!(
        fs::read(
            run.dir
                .join(regenerated["archived_draft"].as_str().unwrap())
        )?,
        b"{ broken"
    );
    let regenerated = run.draft()?;
    assert_eq!(regenerated["decision"], Value::Null);
    let mut repaired: Value = serde_json::from_slice(&valid)?;
    repaired["reason"] = regenerated["reason"].clone();
    run.write_draft(&repaired)?;
    check(&mut fixture, "regenerated-check", &run, Some(&config))?;

    // The review list may name any policy; rkc never consults a catalog.
    let worktree = fixture.worktree("caller-policy")?;
    let arbitrary = fixture.start(
        "caller-policy-start",
        &worktree,
        &clean,
        CliOptions::default(),
    )?;
    arbitrary.write_draft(&assessments::declare_policies_of(
        arbitrary.draft()?,
        &["caller-policy"],
    ))?;
    let draft = arbitrary.draft()?;
    assert_eq!(draft["policies"].as_array().unwrap().len(), 1);
    assert_eq!(draft["policies"][0]["policy_id"], "caller-policy");

    let no_token = fixture.worktree("no-token")?;
    let refused = fixture.rkc(
        "no-token",
        &no_token,
        &["run", "start", "--mr", clean["url"].as_str().unwrap()],
        CliOptions {
            offline: true,
            expected: 2,
            ..Default::default()
        },
    )?;
    assert_eq!(first_code(&refused), "configuration");

    let mr = fixture.manifest["mrs"]["review"].clone();
    let worktree = fixture.worktree("body")?;
    let (body, draft) = fixture.prepare("body", &worktree, &mr, CliOptions::default())?;
    // Retained bytes copied from another run fail their recorded hash, and an
    // assessment copied from another run fails its run binding.
    let retained = run.dir.join("reviewed-source.json");
    let original = fs::read(&retained)?;
    fs::copy(body.dir.join("reviewed-source.json"), &retained)?;
    let copied = rejected(
        &mut fixture,
        "copied-source-check",
        &run,
        &["check"],
        Some(&config),
    )?;
    assert_eq!(first_code(&copied), "hash_mismatch");
    private_file(&retained, &original)?;
    let own = fs::read(run.draft_path())?;
    fs::copy(body.draft_path(), run.draft_path())?;
    let foreign = rejected(
        &mut fixture,
        "copied-assessment-check",
        &run,
        &["check"],
        Some(&config),
    )?;
    assert_eq!(first_code(&foreign), "run_binding");
    private_file(run.draft_path(), &own)?;
    check(&mut fixture, "restored-check", &run, Some(&config))?;

    let valid = assessments::initial(draft);
    let before = fixture.observe("invalid-before", &mr)?;
    let mut bad = valid.clone();
    bad["new_findings"][0]["body"] = json!(format!(
        "{}\n\n<!-- review-kit:v1 {{\"kind\":\"summary\"}} -->",
        bad["new_findings"][0]["body"].as_str().unwrap()
    ));
    let mut wrong_original = valid.clone();
    wrong_original["new_findings"][1]["suggestion"]["original"] = json!(["wrong original source"]);
    for (label, invalid) in [("reserved-body", bad), ("bad-suggestion", wrong_original)] {
        body.write_draft(&invalid)?;
        let checked = rejected(
            &mut fixture,
            &format!("{label}-check"),
            &body,
            &["check"],
            None,
        )?;
        assert_eq!(checked["status"], "invalid");
        let refused = fixture.rkc(
            &format!("{label}-accept"),
            &body.worktree,
            &["accept"],
            CliOptions {
                expected: 2,
                ..Default::default()
            },
        )?;
        assert_eq!(refused["status"], "rejected");
        assert_eq!(body.manifest()?["review_assessment"], Value::Null);
        let unpublished = fixture.rkc(
            &format!("{label}-publish"),
            &body.worktree,
            &["publish", "--effects", "review"],
            CliOptions {
                expected: 2,
                ..Default::default()
            },
        )?;
        assert_eq!(first_code(&unpublished), "assessment_not_accepted");
    }
    body.write_draft(&valid)?;
    check(&mut fixture, "valid-body-check", &body, None)?;
    fixture.accept("valid-body-assessment", &body, CliOptions::default())?;
    plan(&mut fixture, "read-only-plan", &body, CliOptions::default())?;
    assert_eq!(body.manifest()?["publication_attempted"], false);
    let after = fixture.observe("invalid-after", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    fixture.finish()
}
