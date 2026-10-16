use super::super::fixture::{authored_discussions, authored_notes};
use super::super::{
    assessments,
    fixture::{CliOptions, Fixture, marker},
    source::{AUDIT_ORIGINAL, AUDIT_REPLACEMENT},
    workflow::first_code,
};
use super::{assert_counts, assert_round, check, plan, publish, repeat};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn initial_fixed_and_recurrent_review(mut fixture: Fixture) -> Result<()> {
    let mr = fixture.manifest["mrs"]["review"].clone();
    let worktree = fixture.worktree("review")?;
    let (run, draft) = fixture.prepare("initial", &worktree, &mr, CliOptions::default())?;
    assert_eq!(run.started["carried_findings"], 0);
    assert_eq!(run.started["next_finding_id"], 1);
    let mut value = assessments::initial(draft);
    value["new_findings"][1]["suggestion"]["original"] = json!([AUDIT_ORIGINAL, "    Ok(())"]);
    value["new_findings"][1]["suggestion"]["replacement"] =
        json!([AUDIT_REPLACEMENT, "    Ok(())"]);
    run.write_draft(&value)?;
    let source = fixture.committed_file(
        "src/storage.rs",
        value["revision"]["head"].as_str().unwrap(),
    )?;
    let content = STANDARD.decode(source["content"].as_str().unwrap().replace('\n', ""))?;
    assert!(
        std::str::from_utf8(&content)?
            .lines()
            .any(|line| line == AUDIT_ORIGINAL)
    );
    check(&mut fixture, "initial-check", &run, None)?;
    fixture.accept("initial-assessment", &run, CliOptions::default())?;
    let unpublished = fixture.observe("initial-before", &mr)?;
    plan(&mut fixture, "initial-plan", &run, CliOptions::default())?;
    assert!(authored_notes(&fixture.observe("initial-planned", &mr)?).is_empty());
    assert!(authored_notes(&unpublished).is_empty());
    let (published, result) = publish(
        &mut fixture,
        "initial-publish",
        &run,
        "completed",
        CliOptions::default(),
    )?;
    assert_eq!(result["round"], 1);
    assert_eq!(published["settlement"]["outcome"], "published");
    assert_eq!(run.manifest()?["completion"], "published");
    let observed = fixture.observe("initial", &mr)?;
    assert_eq!(
        observed["mr"]["diff_refs"]["head_sha"],
        value["revision"]["head"]
    );
    let (summaries, roots) = fixture.owned(&observed);
    assert_eq!(summaries.len(), 1);
    assert_eq!(roots.keys().copied().collect::<Vec<_>>(), [1, 2]);
    assert_round(&summaries[0], 1, &[]);
    assert_counts(&summaries[0], 2, 0);
    let summary_id = summaries[0]["id"].clone();
    let original_roots = roots.clone();
    for finding in value["new_findings"].as_array().unwrap() {
        let root = &roots[&finding["id"].as_u64().unwrap()]["notes"][0];
        assert_eq!(root["type"], "DiffNote");
        assert_eq!(root["author"]["id"], fixture.manifest["reviewer"]["id"]);
        assert_eq!(root["position"]["head_sha"], value["revision"]["head"]);
        assert_eq!(root["position"]["new_path"], finding["location"]["path"]);
        assert_eq!(root["position"]["new_line"], finding["location"]["line"]);
        assert!(
            root["body"]
                .as_str()
                .unwrap()
                .contains(finding["title"].as_str().unwrap())
        );
        assert!(
            root["body"]
                .as_str()
                .unwrap()
                .contains(finding["body"].as_str().unwrap())
        );
        assert!(
            summaries[0]["body"]
                .as_str()
                .unwrap()
                .contains(&format!("#note_{}", root["id"]))
        );
    }
    let suggestions = roots[&2]["notes"][0]["suggestions"].as_array().unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["from_content"],
        format!("{AUDIT_ORIGINAL}\n    Ok(())\n")
    );
    assert_eq!(
        suggestions[0]["to_content"],
        format!("{AUDIT_REPLACEMENT}\n    Ok(())\n")
    );
    assert_eq!(
        suggestions[0]["from_line"],
        value["new_findings"][1]["location"]["line"]
    );
    assert_eq!(
        suggestions[0]["to_line"],
        suggestions[0]["from_line"].as_u64().unwrap() + 1
    );
    let repeated = repeat(
        &mut fixture,
        "initial-repeat",
        &run,
        &mr,
        1,
        CliOptions::default(),
    )?;

    // A run accepted before the next push cannot publish the older revision.
    let (stale, draft) = fixture.prepare("stale", &worktree, &mr, CliOptions::default())?;
    stale.write_draft(&assessments::equivalent(&repeated.accepted()?, draft))?;
    fixture.accept("stale-assessment", &stale, CliOptions::default())?;
    fixture.evolve(false)?;
    let mr = fixture.manifest["mrs"]["review"].clone();
    let before = fixture.observe("fixed-before", &mr)?;
    let (_, previous_roots) = fixture.owned(&before);
    let author = &fixture.manifest["author"]["id"];
    let replies: BTreeMap<u64, Vec<Value>> = previous_roots
        .iter()
        .map(|(id, discussion)| {
            (
                *id,
                discussion["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .skip(1)
                    .filter(|note| note["author"]["id"] == *author && note["system"] == false)
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    assert!(replies.values().all(|notes| notes.len() == 1));
    let rejected = fixture.rkc(
        "stale-publish",
        &worktree,
        &["publish", "--effects", "review"],
        CliOptions {
            expected: 3,
            ..Default::default()
        },
    )?;
    assert_eq!(first_code(&rejected), "source_changed");
    assert_eq!(stale.manifest()?["publication_attempted"], false);
    let after = fixture.observe("stale-after", &mr)?;
    assert_eq!(authored_notes(&after), authored_notes(&before));
    assert_eq!(authored_discussions(&after), authored_discussions(&before));
    let stopped = fixture.rkc(
        "stale-stop",
        &worktree,
        &[
            "run",
            "stop",
            "--reason",
            "The merge request has a newer head.",
        ],
        CliOptions::default(),
    )?;
    assert_eq!(stopped["status"], "stopped");
    let (fixed, draft) = fixture.prepare("fixed", &worktree, &mr, CliOptions::default())?;
    assert_eq!(fixed.started["carried_findings"], 2);
    assert_eq!(fixed.started["next_finding_id"], 3);
    assert_eq!(
        draft["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|slot| slot["reassessment"]["id"].as_u64().unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
    );
    let fixed_value = assessments::fixed(draft, &replies);
    fixed.write_draft(&fixed_value)?;
    check(&mut fixture, "fixed-check", &fixed, None)?;
    fixture.accept("fixed-assessment", &fixed, CliOptions::default())?;
    let (_, result) = publish(
        &mut fixture,
        "fixed-publish",
        &fixed,
        "completed",
        CliOptions::default(),
    )?;
    assert_eq!(result["round"], 2);
    let (summaries, roots) = fixture.owned(&fixture.observe("fixed", &mr)?);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["id"], summary_id);
    assert_eq!(roots.keys().copied().collect::<Vec<_>>(), [1, 2]);
    assert_round(&summaries[0], 2, &[1]);
    assert_counts(&summaries[0], 0, 2);
    for (id, discussion) in &roots {
        let original = &original_roots[id];
        assert_eq!(discussion["id"], original["id"]);
        assert_eq!(discussion["notes"][0]["id"], original["notes"][0]["id"]);
        assert_eq!(discussion["notes"][0]["body"], original["notes"][0]["body"]);
        assert_eq!(discussion["notes"][0]["resolved"], true);
        let notes = discussion["notes"].as_array().unwrap();
        let records: Vec<_> = notes
            .iter()
            .skip(1)
            .filter(|note| note["author"]["id"] == fixture.manifest["reviewer"]["id"])
            .filter_map(|note| marker(note["body"].as_str().unwrap()))
            .collect();
        for reply in &replies[id] {
            assert!(records.contains(&json!({"kind": "answer", "finding": id, "round": 2, "note": reply["id"].to_string()})));
        }
        assert!(records.contains(&json!({"kind": "transition", "finding": id, "round": 2})));
        for answer in fixed_value["answers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|answer| answer["finding"] == *id)
        {
            let expected =
                json!({"kind": "answer", "finding": id, "round": 2, "note": answer["in_reply_to"]});
            assert!(notes.iter().skip(1).any(|note| {
                marker(note["body"].as_str().unwrap()) == Some(expected.clone())
                    && note["body"]
                        .as_str()
                        .unwrap()
                        .contains(answer["body"].as_str().unwrap())
            }));
        }
    }
    repeat(
        &mut fixture,
        "fixed-repeat",
        &fixed,
        &mr,
        2,
        CliOptions::default(),
    )?;

    fixture.evolve(true)?;
    let mr = fixture.manifest["mrs"]["review"].clone();
    // A new worktree continues from provider history and the shared journal.
    let fresh = fixture.worktree("recurrence")?;
    let (recurrence, draft) = fixture.prepare("recurrence", &fresh, &mr, CliOptions::default())?;
    assert_eq!(recurrence.started["carried_findings"], 2);
    recurrence.write_draft(&assessments::recurrent(draft))?;
    check(&mut fixture, "recurrence-check", &recurrence, None)?;
    fixture.accept("recurrence-assessment", &recurrence, CliOptions::default())?;
    let (_, result) = publish(
        &mut fixture,
        "recurrence-publish",
        &recurrence,
        "completed",
        CliOptions::default(),
    )?;
    assert_eq!(result["round"], 3);
    let (summaries, roots) = fixture.owned(&fixture.observe("recurrence", &mr)?);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["id"], summary_id);
    assert_eq!(roots.keys().copied().collect::<Vec<_>>(), [1, 2]);
    assert_round(&summaries[0], 3, &[1, 2]);
    assert_counts(&summaries[0], 1, 1);
    for (id, discussion) in &roots {
        let original = &original_roots[id];
        assert_eq!(discussion["id"], original["id"]);
        assert_eq!(discussion["notes"][0]["id"], original["notes"][0]["id"]);
        assert_eq!(discussion["notes"][0]["body"], original["notes"][0]["body"]);
        assert_eq!(discussion["notes"][0]["resolved"], *id == 2);
    }
    assert!(
        roots[&1]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .any(|note| marker(note["body"].as_str().unwrap())
                == Some(json!({"kind": "transition", "finding": 1, "round": 3})))
    );
    repeat(
        &mut fixture,
        "recurrence-repeat",
        &recurrence,
        &mr,
        3,
        CliOptions::default(),
    )?;
    fixture.finish()
}
