use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::super::{
    assessments,
    fixture::{CliOptions, Fixture, read_json, write_json},
};
use super::{check, publish, repeat};

pub fn custom_profile_review(mut fixture: Fixture) -> Result<()> {
    let directory = fixture.directory.join("render-profile");
    fixture.cli(
        "profile-export",
        &[
            "render-profile",
            "export",
            "--dir",
            directory.to_str().context("profile directory")?,
        ],
        CliOptions::default(),
    )?;
    let manifest = read_json(directory.join("profile.json"))?;
    for role in ["finding", "summary", "suggestion", "transition", "answer"] {
        let path = directory.join(
            manifest["artifacts"][role]["template"]
                .as_str()
                .context("template path")?,
        );
        let template = fs::read_to_string(&path)?;
        fs::write(path, format!("TEAM {}\n{template}", role.to_uppercase()))?;
    }
    let config_path = fixture.directory.join("render-profile-config.json");
    let mut config = read_json(fixture.manifest["config"].as_str().context("config path")?)?;
    config["render_profile"] = json!(directory.join("profile.json"));
    write_json(&config_path, &config)?;
    let options = CliOptions {
        config: Some(&config_path),
        ..Default::default()
    };
    let mr = fixture.manifest["mrs"]["review"].clone();
    let worktree = fixture.worktree("profile")?;
    let (run, draft) = fixture.prepare("profile-initial", &worktree, &mr, options)?;
    run.write_draft(&assessments::initial(draft))?;
    let checked = check(
        &mut fixture,
        "profile-initial-check",
        &run,
        Some(&config_path),
    )?;
    assert!(
        checked["check"]["plan"]["summary_body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM SUMMARY\n")
    );
    fixture.accept("profile-initial-assessment", &run, options)?;
    let (_, result) = publish(
        &mut fixture,
        "profile-initial-publish",
        &run,
        "completed",
        options,
    )?;
    assert_eq!(result["round"], 1);
    let observed = fixture.observe("profile-initial", &mr)?;
    let (summaries, roots) = fixture.owned(&observed);
    assert!(
        summaries[0]["body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM SUMMARY\n")
    );
    assert!(roots.values().all(|thread| {
        thread["notes"][0]["body"]
            .as_str()
            .unwrap()
            .starts_with("TEAM FINDING\n")
    }));
    assert!(
        roots[&2]["notes"][0]["body"]
            .as_str()
            .unwrap()
            .contains("TEAM SUGGESTION\n")
    );
    let original_roots = roots.clone();
    repeat(
        &mut fixture,
        "profile-initial-repeat",
        &run,
        &mr,
        1,
        options,
    )?;

    fixture.evolve(false)?;
    let mr = fixture.manifest["mrs"]["review"].clone();
    let observed = fixture.observe("profile-fixed-before", &mr)?;
    let (_, roots) = fixture.owned(&observed);
    let author = fixture.manifest["author"]["id"].clone();
    let replies: BTreeMap<u64, Vec<Value>> = roots
        .iter()
        .map(|(id, discussion)| {
            (
                *id,
                discussion["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .skip(1)
                    .filter(|note| note["author"]["id"] == author && note["system"] == false)
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    assert!(replies.values().all(|notes| !notes.is_empty()));
    let (fixed, draft) = fixture.prepare("profile-fixed", &worktree, &mr, options)?;
    fixed.write_draft(&assessments::fixed(draft, &replies))?;
    check(
        &mut fixture,
        "profile-fixed-check",
        &fixed,
        Some(&config_path),
    )?;
    fixture.accept("profile-fixed-assessment", &fixed, options)?;
    publish(
        &mut fixture,
        "profile-fixed-publish",
        &fixed,
        "completed",
        options,
    )?;
    let (summaries, roots) = fixture.owned(&fixture.observe("profile-fixed", &mr)?);
    let summary = summaries[0]["body"].as_str().unwrap();
    assert!(summary.contains("<summary>Round 1"));
    assert!(summary.contains("| ID | Finding | Severity | Status | This round |"));
    assert!(summary.contains("| ID | Finding | Severity | Status | That round |"));
    for (id, thread) in &roots {
        assert_eq!(thread["id"], original_roots[id]["id"]);
        assert_eq!(
            thread["notes"][0]["id"],
            original_roots[id]["notes"][0]["id"]
        );
        assert_eq!(thread["notes"][0]["resolved"], true);
        let notes = thread["notes"].as_array().unwrap();
        assert!(notes.iter().skip(1).any(|note| {
            note["body"]
                .as_str()
                .unwrap()
                .starts_with("TEAM TRANSITION\n")
        }));
        assert!(
            notes
                .iter()
                .skip(1)
                .any(|note| note["body"].as_str().unwrap().starts_with("TEAM ANSWER\n"))
        );
    }
    repeat(
        &mut fixture,
        "profile-fixed-repeat",
        &fixed,
        &mr,
        2,
        options,
    )?;
    fixture.finish()
}
