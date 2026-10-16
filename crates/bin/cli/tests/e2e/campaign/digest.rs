//! Compact digests of independent merge request observations and their
//! comparison. Managed notes are classified with the caller's marker parser
//! so these functions stay independent of the fixture client.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Parses a managed note's trailing marker into its JSON record.
pub type MarkerParser = fn(&str) -> Option<Value>;

fn digest_text(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Reads the JSON state block from a managed summary body, if present.
pub fn summary_state(body: &str) -> Option<Value> {
    let data = body.split_once("\n\n<!-- review-kit:state:v3 ")?.1;
    let text = data.split_once(" -->\n\n<!-- review-kit:v1 ")?.0;
    serde_json::from_str(text).ok()
}

/// Compact state used for printing and comparison.
pub fn summarize(raw: &Value, reviewer: &Value, marker: MarkerParser) -> Value {
    let mr = &raw["merge_request"];
    let notes = raw["notes"].as_array().cloned().unwrap_or_default();
    let discussions = raw["discussions"].as_array().cloned().unwrap_or_default();
    let mut authors = BTreeMap::<String, u64>::new();
    let mut system = 0_u64;
    for note in &notes {
        if note["system"] == true {
            system += 1;
        } else {
            *authors
                .entry(
                    note["author"]["username"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_owned(),
                )
                .or_default() += 1;
        }
    }
    let (mut summaries, mut roots, mut threads) = (Vec::new(), Vec::new(), Map::new());
    let (mut resolvable, mut resolved) = (0_u64, 0_u64);
    for discussion in &discussions {
        let entries: Vec<_> = discussion["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|note| note["system"] != true)
            .collect();
        if entries.is_empty() {
            continue;
        }
        let states: Vec<_> = entries
            .iter()
            .filter(|note| note["resolvable"] == true)
            .map(|note| note["resolved"] == true)
            .collect();
        let state = if states.is_empty() {
            Value::Null
        } else {
            resolvable += 1;
            let all = states.iter().all(|state| *state);
            resolved += u64::from(all);
            json!(all)
        };
        let id = discussion["id"].as_str().unwrap_or_default().to_owned();
        threads.insert(
            id.clone(),
            json!({"resolved": state, "notes": entries.iter().map(|note| json!({"id": note["id"], "author": note["author"]["id"], "type": note["type"], "body_sha256": digest_text(note["body"].as_str().unwrap_or("")), "position": note["position"]})).collect::<Vec<_>>()}),
        );
        let root = entries[0];
        if root["author"]["id"] != *reviewer {
            continue;
        }
        match root["body"].as_str().and_then(marker) {
            Some(record) if record["kind"] == "finding" => roots.push(json!({"finding": record["finding"], "discussion": id, "note": root["id"], "resolved": state, "path": root["position"]["new_path"], "line": root["position"]["new_line"], "head": root["position"]["head_sha"], "replies": entries.len() - 1})),
            _ => {}
        }
    }
    for note in &notes {
        if note["author"]["id"] == *reviewer
            && let Some(body) = note["body"].as_str()
            && marker(body) == Some(json!({"kind": "summary"}))
        {
            let state = summary_state(body);
            summaries.push(json!({"note": note["id"], "round": state.as_ref().map(|state| state["current"]["number"].clone()), "status": state.as_ref().map(|state| state["current"]["status"].clone()), "history": state.as_ref().and_then(|state| state["history"].as_array().map(Vec::len)), "body_sha256": digest_text(body), "body": body}));
        }
    }
    let versions = raw["versions"].as_array().cloned().unwrap_or_default();
    json!({
        "url": mr["web_url"],
        "state": mr["state"],
        "title": mr["title"],
        "description_sha256": digest_text(mr["description"].as_str().unwrap_or("")),
        "source_branch": mr["source_branch"],
        "target_branch": mr["target_branch"],
        "diff_refs": mr["diff_refs"],
        "versions": {"count": versions.len(), "latest": versions.first().map(|version| json!({"head": version["head_commit_sha"], "base": version["base_commit_sha"], "start": version["start_commit_sha"], "state": version["state"]}))},
        "notes": {"total": notes.len(), "system": system, "by_author": authors},
        "discussions": {"total": discussions.len(), "with_authored_notes": threads.len(), "resolvable": resolvable, "resolved": resolved, "pages": discussions.len().div_ceil(100)},
        "managed": {"summaries": summaries, "finding_roots": roots},
        "threads": threads,
    })
}

/// Lists what changed between two digests of the same merge request.
pub fn compare(before: &Value, after: &Value) -> Value {
    let notes = |summary: &Value| -> BTreeMap<String, (Value, Value)> {
        summary["threads"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(discussion, thread)| {
                thread["notes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(move |note| (note["id"].to_string(), (json!(discussion), note.clone())))
            })
            .collect()
    };
    let (old, new) = (notes(before), notes(after));
    let added: Vec<_> = new
        .keys()
        .filter(|id| !old.contains_key(*id))
        .map(|id| json!(id))
        .collect();
    let removed: Vec<_> = old
        .keys()
        .filter(|id| !new.contains_key(*id))
        .map(|id| json!(id))
        .collect();
    let edited: Vec<_> = new
        .iter()
        .filter(|(id, (_, note))| {
            old.get(*id)
                .is_some_and(|(_, previous)| previous["body_sha256"] != note["body_sha256"])
        })
        .map(|(id, _)| json!(id))
        .collect();
    let moved: Vec<_> = new
        .iter()
        .filter(|(id, (_, note))| {
            old.get(*id)
                .is_some_and(|(_, previous)| previous["position"] != note["position"])
        })
        .map(|(id, _)| json!(id))
        .collect();
    let resolution: Vec<_> = after["threads"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, thread)| {
            let previous = &before["threads"][id]["resolved"];
            (before["threads"].get(id).is_some() && *previous != thread["resolved"])
                .then(|| json!({"discussion": id, "before": previous, "after": thread["resolved"]}))
        })
        .collect();
    let roots = |summary: &Value| -> BTreeMap<String, Value> {
        summary["managed"]["finding_roots"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|root| (root["finding"].to_string(), root.clone()))
            .collect()
    };
    let (old_roots, new_roots) = (roots(before), roots(after));
    let summary = |summary: &Value| -> Value {
        summary["managed"]["summaries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| json!({"note": entry["note"], "round": entry["round"], "status": entry["status"], "body_sha256": entry["body_sha256"]}))
            .collect()
    };
    json!({
        "diff_refs_changed": before["diff_refs"] != after["diff_refs"],
        "target_changed": before["target_branch"] != after["target_branch"],
        "title_changed": before["title"] != after["title"],
        "description_changed": before["description_sha256"] != after["description_sha256"],
        "notes_added": added,
        "notes_removed": removed,
        "notes_edited": edited,
        "positions_changed": moved,
        "resolution_changed": resolution,
        "finding_roots_added": new_roots.keys().filter(|id| !old_roots.contains_key(*id)).collect::<Vec<_>>(),
        "finding_roots_removed": old_roots.keys().filter(|id| !new_roots.contains_key(*id)).collect::<Vec<_>>(),
        "summaries_before": summary(before),
        "summaries_after": summary(after),
        "authored_notes_before": before["notes"]["total"].as_u64().unwrap_or(0) - before["notes"]["system"].as_u64().unwrap_or(0),
        "authored_notes_after": after["notes"]["total"].as_u64().unwrap_or(0) - after["notes"]["system"].as_u64().unwrap_or(0),
    })
}

/// The digest without per-thread detail, for terminal output.
pub fn compact(summary: &Value) -> Value {
    let mut compact = summary.clone();
    if let Some(object) = compact.as_object_mut() {
        object.remove("threads");
    }
    if let Some(summaries) = compact["managed"]["summaries"].as_array_mut() {
        for entry in summaries {
            if let Some(object) = entry.as_object_mut() {
                object.remove("body");
            }
        }
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(body: &str) -> Option<Value> {
        let text = body
            .strip_suffix(" -->")?
            .rsplit_once("<!-- review-kit:v1 ")?
            .1;
        serde_json::from_str(text).ok()
    }

    fn raw(resolved: bool, body: &str) -> Value {
        json!({
            "merge_request": {"web_url": "u", "title": "t", "description": "d", "diff_refs": {"head_sha": "h"}},
            "notes": [
                {"id": 1, "system": false, "author": {"id": 7, "username": "reviewer"}, "body": body},
                {"id": 2, "system": true, "author": {"id": 3, "username": "author"}, "body": "changed"}
            ],
            "discussions": [{"id": "d1", "notes": [{"id": 1, "system": false, "author": {"id": 7}, "type": "DiffNote", "resolvable": true, "resolved": resolved, "body": body, "position": {"new_path": "src/a.rs", "new_line": 3, "head_sha": "h"}}]}],
            "versions": [{"head_commit_sha": "h", "state": "collected"}]
        })
    }

    #[test]
    fn digests_count_authored_notes_and_managed_roots() {
        let body = "finding\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":4} -->";
        let summary = summarize(&raw(false, body), &json!(7), marker);
        assert_eq!(
            summary["notes"],
            json!({"total": 2, "system": 1, "by_author": {"reviewer": 1}})
        );
        assert_eq!(summary["discussions"]["resolvable"], 1);
        assert_eq!(summary["managed"]["finding_roots"][0]["finding"], 4);
        assert_eq!(summary["managed"]["finding_roots"][0]["line"], 3);
        assert_eq!(
            summarize(&raw(false, body), &json!(8), marker)["managed"]["finding_roots"],
            json!([])
        );
    }

    #[test]
    fn comparison_reports_edits_and_resolution_changes() {
        let before = summarize(&raw(false, "first"), &json!(7), marker);
        let after = summarize(&raw(true, "edited"), &json!(7), marker);
        let changes = compare(&before, &after);
        assert_eq!(changes["notes_edited"], json!(["1"]));
        assert_eq!(
            changes["resolution_changed"],
            json!([{"discussion": "d1", "before": false, "after": true}])
        );
        assert_eq!(changes["notes_added"], json!([]));
        let unchanged = compare(&before, &before);
        assert_eq!(unchanged["resolution_changed"], json!([]));
        assert_eq!(unchanged["diff_refs_changed"], false);
    }
}
