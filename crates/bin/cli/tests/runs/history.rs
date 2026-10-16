//! Managed review history conflicts reported by run start, planning and publication.

use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness, read_json, ready, write_json},
    server::{Fault, MR_PATH, Response},
};

#[test]
fn unsupported_retained_summary_version_stops_current_run_discovery_without_writes() {
    let harness = Harness::new();
    published_fix(&harness);
    let open = accepted_follow_up(&harness);
    {
        let mut state = harness.http.state.lock().unwrap();
        let original = state.notes[0]["body"].as_str().unwrap();
        assert!(original.contains("<!-- review-kit:state:v3 "));
        state.notes[0]["body"] = original
            .replace("<!-- review-kit:state:v3 ", "<!-- review-kit:state:v4 ")
            .into();
    }
    let writes = harness.writes();
    for args in [
        vec!["run", "start", "--new"],
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let refused = harness.fails(&args, 5, "unsupported_retained_state_version");
        let diagnostic = &refused["diagnostics"][0];
        assert_eq!(diagnostic["state_version"], 4);
        assert_eq!(diagnostic["supported_state_version"], 3);
        assert!(!diagnostic["next"].as_str().unwrap().contains("repair"));
        assert_eq!(harness.writes(), writes);
    }
    assert_eq!(harness.selected(), open);
    assert_eq!(harness.manifest(&open)["publication_attempted"], false);
}

#[test]
fn retained_state_sizes_are_measured_from_single_and_fixed_followup_publication() {
    let first = Harness::new();
    first.accepted_with_finding();
    first.ok(&["publish", "--effects", "review"]);
    measure_retained_state(&first, "published_single_round");
    let followup = Harness::new();
    published_fix(&followup);
    measure_retained_state(&followup, "published_fixed_followup");
}

fn measure_retained_state(harness: &Harness, scenario: &str) {
    use review_kit_json_adapter::rounds::{RoundData, State};
    let body = harness.http.state.lock().unwrap().notes[0]["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let (visible, tail) = body.split_once("\n\n<!-- review-kit:state:v3 ").unwrap();
    let (payload, _) = tail.split_once(" -->\n\n<!-- review-kit:v1 ").unwrap();
    let (current, history) = State::decode(payload.as_bytes())
        .unwrap()
        .into_rounds()
        .unwrap();
    let compact = State::from_rounds(&current, &history).unwrap();
    let compact_raw = serde_json::to_string(&compact).unwrap();
    let full_raw = serde_json::to_string(&json!({
        "current": RoundData::from(&current),
        "history": history.iter().map(RoundData::from).collect::<Vec<_>>(),
    }))
    .unwrap();
    let escape = |text: &str| {
        text.replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026")
            .replace('`', "\\u0060")
            .replace('~', "\\u007e")
    };
    assert_eq!(escape(&compact_raw), payload);
    assert_eq!(compact.into_rounds().unwrap(), (current, history));
    println!(
        "STATE_V3_MEASUREMENT {}",
        json!({
            "scenario": scenario, "full_wrapper_raw":full_raw.len(),
            "full_wrapper_escaped":escape(&full_raw).len(), "compact_raw":compact_raw.len(),
            "compact_escaped":payload.len(), "visible_bytes":visible.len(),
            "framing_bytes":body.len()-visible.len()-payload.len(), "note_bytes":body.len(),
        })
    );
}

/// Publish finding 1, then a follow-up that fixes it with a transition reply.
fn published_fix(harness: &Harness) {
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let fix = harness.draft_with_policies(&[ERROR_POLICY]);
    let path = harness.file(&fix, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    let mut fixed = draft["findings"][0]["reassessment"].clone();
    fixed["status"] = json!("fixed");
    fixed["evidence"] = json!("The failure branch now returns the cause.");
    draft["findings"][0]["reassessment"] = fixed;
    draft["transitions"] = json!([{"finding":1,"from":"open","to":"fixed",
        "body":"The failure path now keeps its cause; the regression passes."}]);
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let notes = harness.http.state.lock().unwrap().discussions[0]["notes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(notes, 2, "the finding thread holds its root and transition");
}

/// An open run whose complete chain reassesses the fixed finding.
fn accepted_follow_up(harness: &Harness) -> String {
    let run_id = harness.draft_with_policies(&[ERROR_POLICY]);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    let mut fixed = draft["findings"][0]["reassessment"].clone();
    fixed["evidence"] = json!("The failure branch still returns the cause.");
    draft["findings"][0]["reassessment"] = fixed;
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    run_id
}

fn assert_conflict(value: &Value, conflict: &str, finding: Option<u64>) {
    let diagnostic = &value["diagnostics"][0];
    assert_eq!(diagnostic["code"], "managed_history_conflict", "{value:#}");
    assert_eq!(diagnostic["conflict"], conflict, "{value:#}");
    assert_eq!(diagnostic["finding"], json!(finding), "{value:#}");
    let message = diagnostic["message"].as_str().unwrap();
    if let Some(finding) = finding {
        assert!(message.contains(&format!("finding {finding}")), "{message}");
    }
    let next = diagnostic["next"].as_str().unwrap();
    assert!(next.contains("does not repair provider history"), "{next}");
    assert!(next.contains("operation is blocked"), "{next}");
    assert!(next.contains("operator"), "{next}");
}

/// D06 with surviving replies: the thread keeps the reviewer's transition after
/// its root note is deleted, so the summary and that reply still name finding 1.
#[test]
fn a_deleted_finding_root_with_surviving_replies_is_a_managed_history_conflict() {
    let harness = Harness::new();
    published_fix(&harness);
    let open = accepted_follow_up(&harness);
    harness.http.state.lock().unwrap().discussions[0]["notes"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    let writes = harness.writes();

    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
        vec!["run", "start", "--new"],
    ] {
        let refused = harness.fails(&args, 5, "managed_history_conflict");
        assert_conflict(&refused, "missing_finding_root", Some(1));
    }
    assert_eq!(harness.writes(), writes, "no provider writes");
    assert_eq!(harness.selected(), open, "the selection is unchanged");
    assert_eq!(
        harness.manifest(&open)["publication_attempted"],
        false,
        "refused publication never records an attempt"
    );
    let notes = harness.http.state.lock().unwrap().discussions[0]["notes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(notes, 1, "the deleted root is not recreated");
}

/// D06 without replies: GitLab removes the whole thread with its only note.
#[test]
fn a_deleted_finding_thread_is_a_managed_history_conflict_naming_the_finding() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    harness.http.state.lock().unwrap().discussions.clear();
    let writes = harness.writes();
    let refused = harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        5,
        "managed_history_conflict",
    );
    assert_conflict(&refused, "missing_finding_root", Some(1));
    assert_eq!(harness.writes(), writes);
    assert_eq!(harness.discussions(), 0, "the finding is not recreated");
}

/// A late original creation after a resend: two managed roots claim finding 1.
/// Every command that discovers the merge request names both root notes and
/// their creation times, and Review Kit deletes neither.
#[test]
fn two_roots_claiming_one_finding_are_a_duplicate_finding_conflict_naming_every_root() {
    let harness = Harness::new();
    let published = harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let open = harness.follow_up_in(&harness.repo, false);
    let original = {
        let mut state = harness.http.state.lock().unwrap();
        let mut copy = state.discussions[0].clone();
        copy["id"] = json!("discussion-copy");
        copy["notes"][0]["id"] = json!(9001);
        copy["notes"][0]["created_at"] = json!("2026-01-01T09:00:00.000Z");
        state.discussions.push(copy);
        state.discussions[0]["notes"][0].clone()
    };
    let roots = json!([
        {"note": original["id"], "created_at": original["created_at"]},
        {"note": 9001, "created_at": "2026-01-01T09:00:00.000Z"},
    ]);
    let writes = harness.writes();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
        vec!["run", "start", "--new"],
    ] {
        let refused = harness.fails(&args, 5, "managed_history_conflict");
        assert_conflict(&refused, "duplicate_finding", Some(1));
        let diagnostic = &refused["diagnostics"][0];
        assert_eq!(diagnostic["roots"], roots, "{args:?}: {refused:#}");
        let message = diagnostic["message"].as_str().unwrap();
        for root in roots.as_array().unwrap() {
            assert!(
                message.contains(&root["note"].to_string()),
                "{args:?}: {message}"
            );
        }
        let next = diagnostic["next"].as_str().unwrap();
        assert!(next.contains("never deletes duplicates"), "{next}");
        assert!(next.contains("separately authorized operator"), "{next}");
        assert!(next.contains("CLI guide"), "{next}");
    }
    // The guide's repair identifies the verified root from the publishing run's
    // finding readback records.
    let events = read_json(&harness.file(&published, "review-publication-events.json"));
    let verified: Vec<&Value> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["kind"] == "readback"
                && event["write"]["kind"] == "finding"
                && event["write"]["finding"]["id"] == 1
        })
        .flat_map(|event| event["snapshot"]["findings"].as_array().unwrap())
        .filter(|root| root["finding"]["id"] == 1)
        .map(|root| &root["root_note"])
        .collect();
    assert_eq!(verified, [&json!(original["id"].to_string())]);
    assert_eq!(harness.writes(), writes, "no provider writes");
    assert_eq!(harness.discussions(), 2, "neither root is deleted");
    assert_eq!(harness.selected(), open);
    assert_eq!(harness.manifest(&open)["publication_attempted"], false);
}

/// D07: corrupted retained summary data is inconsistent history without a
/// single finding, while a provider read failure keeps its own category.
#[test]
fn corrupted_summary_history_conflicts_while_read_failures_keep_their_code() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    {
        let mut state = harness.http.state.lock().unwrap();
        let body = state.notes[0]["body"]
            .as_str()
            .unwrap()
            .replace("\"request\":7", "\"request\":0");
        state.notes[0]["body"] = body.into();
    }
    let refused = harness.fails(
        &["run", "start", "--mr", &harness.mr()],
        5,
        "managed_history_conflict",
    );
    assert_conflict(&refused, "inconsistent_history", None);

    let harness = Harness::new();
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::response(
            "GET",
            &format!("{MR_PATH}/discussions"),
            Response::status(503),
        ));
    harness.fails(&["run", "start", "--mr", &harness.mr()], 7, "discovery");
}
