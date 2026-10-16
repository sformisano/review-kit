use anyhow::Result;
use serde_json::json;

use super::super::{
    assessments,
    fixture::{CliOptions, Fixture, provision},
    source,
};
use super::{assert_counts, check, plan, publish};

pub fn outdated_position_followup(mut fixture: Fixture) -> Result<()> {
    let mr = fixture.manifest["mrs"]["review"].clone();
    let worktree = fixture.worktree("outdated")?;
    let (initial, draft) =
        fixture.prepare("outdated-initial", &worktree, &mr, CliOptions::default())?;
    let mut value = assessments::initial(draft);
    let original: Vec<_> = source::quantity_input()
        .lines()
        .skip(2)
        .take(4)
        .map(str::to_owned)
        .collect();
    value["new_findings"][0]["suggestion"] = json!({
        "location": {"path": "src/quantity.rs", "line": 3},
        "revision": value["revision"], "start_line": 3,
        "original": original,
        "replacement": ["#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]", "pub struct Quantity {", "    value: u32,", "}"],
        "evidence": "Removing derived deserialization closes the constructor bypass."
    });
    value["new_findings"][1]["suggestion"] = json!(null);
    initial.write_draft(&value)?;
    fixture.accept("outdated-initial-accept", &initial, CliOptions::default())?;
    publish(
        &mut fixture,
        "outdated-initial-publish",
        &initial,
        "completed",
        CliOptions::default(),
    )?;
    let before = fixture.observe("outdated-before-fix", &mr)?;
    let (summaries, roots) = fixture.owned(&before);
    let summary_id = summaries[0]["id"].clone();
    let fixed_source = source::quantity_fixed();
    let commit = fixture.author.post(
        &format!("projects/{}/repository/commits", fixture.manifest["project_id"]),
        &json!({"branch": "review", "commit_message": "Validate deserialized quantities",
            "actions": [{"action": "update", "file_path": "src/quantity.rs", "content": fixed_source}]}),
    )?;
    provision::wait_diff(
        &fixture.author,
        &fixture.manifest["project_id"],
        &mr["iid"],
        commit["id"].as_str().unwrap(),
        None,
    )?;
    let moved = fixture.observe("outdated-after-fix", &mr)?;
    let (_, moved_roots) = fixture.owned(&moved);
    let position = &moved_roots[&1]["notes"][0]["position"];
    assert_eq!(position["head_sha"], commit["id"]);
    assert_eq!(position["new_line"], 3);
    assert_eq!(position["line_range"]["start"]["type"], "new");
    assert_eq!(position["line_range"]["end"]["type"], "old");
    assert_eq!(
        moved_roots[&2]["notes"][0]["position"]["new_line"],
        roots[&2]["notes"][0]["position"]["new_line"]
    );

    let (followup, draft) =
        fixture.prepare("outdated-followup", &worktree, &mr, CliOptions::default())?;
    assert_eq!(followup.started["carried_findings"], 2);
    assert_eq!(followup.started["outdated_positions"], json!([1]));
    assert!(
        draft["local_context"]
            .as_str()
            .unwrap()
            .contains("Outdated comment positions: F1")
    );
    let context = followup.context()?;
    assert_eq!(
        context["snapshot"]["findings"][0]["finding"],
        value["new_findings"][0]
    );
    assert_eq!(
        context["snapshot"]["findings"][0]["revision"],
        value["revision"]
    );
    let mut fixed = assessments::equivalent(&initial.accepted()?, draft);
    let finding = &mut fixed["findings"][0]["reassessment"];
    finding["status"] = json!("fixed");
    finding["suggestion"] = json!(null);
    finding["evidence"] = json!("Deserialization calls Quantity::new and rejects zero.");
    fixed["transitions"] = json!([{"finding": 1, "from": "open", "to": "fixed",
        "body": "Fixed at src/quantity.rs:4: deserialization uses the constructor. The old suggestion is outdated."}]);
    followup.write_draft(&fixed)?;
    check(&mut fixture, "outdated-followup-check", &followup, None)?;
    fixture.accept("outdated-followup-accept", &followup, CliOptions::default())?;
    let planned = plan(
        &mut fixture,
        "outdated-followup-plan",
        &followup,
        CliOptions::default(),
    )?;
    let actions = planned["run"]["outcome"]["plan"]["actions"]
        .as_array()
        .unwrap();
    assert_eq!(actions.len(), 2);
    assert!(actions.iter().all(|action| action["kind"] != "finding"));
    publish(
        &mut fixture,
        "outdated-followup-publish",
        &followup,
        "completed",
        CliOptions::default(),
    )?;
    let (summaries, after) = fixture.owned(&fixture.observe("outdated-completed", &mr)?);
    assert_eq!(summaries[0]["id"], summary_id);
    assert_counts(&summaries[0], 1, 1);
    for id in [1, 2] {
        assert_eq!(after[&id]["id"], roots[&id]["id"]);
        assert_eq!(
            after[&id]["notes"][0]["body"],
            roots[&id]["notes"][0]["body"]
        );
        assert_eq!(after[&id]["notes"][0]["resolved"], id == 1);
    }
    let notes = after[&1]["notes"].as_array().unwrap();
    assert!(notes.iter().skip(1).any(|note| {
        note["body"]
            .as_str()
            .unwrap()
            .contains("old suggestion is outdated")
    }));
    assert_eq!(after[&2]["notes"], moved_roots[&2]["notes"]);
    fixture.finish()
}
