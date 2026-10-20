//! A crashed owner whose merge request changed: who may publish next.

use std::fs;

use serde_json::{Value, json};

use super::{
    crash,
    harness::Harness,
    server::{Fault, MR_PATH, Response},
};

/// How the owner's binding came to have no possible provider write.
#[derive(Clone, Copy, Debug)]
enum Unsent {
    /// A crash right after the first prepared intent.
    Prepared,
    /// That crash, then a resumed first write that GitLab definitely rejected.
    Rejected,
}

/// An owner run bound by a two-effect publication of which nothing reached GitLab.
fn unsent_owner(harness: &Harness, unsent: Unsent) -> String {
    let owner = harness.accepted_with_finding();
    let effects = ["publish", "--effects", "create_summary,update_summary"];
    crash(harness, "publish.after_intent:1", &effects);
    if let Unsent::Rejected = unsent {
        harness
            .http
            .state
            .lock()
            .unwrap()
            .faults
            .push(Fault::response(
                "POST",
                &format!("{MR_PATH}/notes"),
                Response::status(422),
            ));
        let (output, rejected) = harness.rkc(&effects);
        assert_eq!(output.status.code(), Some(7), "{rejected:#}");
        assert_eq!(rejected["provider_write_possible"], false, "{rejected:#}");
    }
    let pending = harness.journal();
    assert_eq!(pending["coverage_binding"]["run_id"], owner);
    assert_eq!(pending["dispatch_may_have_started"], false);
    match unsent {
        Unsent::Prepared => assert_eq!(pending["unresolved"]["phase"], "prepared"),
        Unsent::Rejected => assert_eq!(pending["unresolved"], Value::Null),
    }
    assert_eq!(
        pending["authorization"]["effects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    owner
}

/// After the owner's crash the head moves, so the owner can no longer publish
/// and a fresh run releases its binding.
fn yields_to_a_fresh_run(unsent: Unsent) {
    let harness = Harness::new();
    let owner = unsent_owner(&harness, unsent);
    let pending = harness.journal();
    let status = harness.ok(&["status"]);
    assert_eq!(
        status["journal"]["publication_status"], "bound_unsent",
        "{unsent:?}"
    );
    assert_eq!(status["journal"]["provider_write_possible"], false);
    assert!(
        status["next"]
            .as_str()
            .unwrap()
            .contains("rkc run start --new"),
        "{status:#}"
    );
    let refused = harness.fails(
        &["run", "stop", "--reason", "Too early."],
        5,
        "publication_pending_owner",
    );
    assert_eq!(refused["diagnostics"][0]["provider_write_possible"], false);
    harness.push_commit();
    let writes = harness.writes();
    let failed = harness.fails(&["publish", "--effects", "review"], 3, "source_changed");
    assert!(
        failed["diagnostics"][0]["next"]
            .as_str()
            .unwrap()
            .contains("rkc run start --new"),
        "{failed:#}"
    );
    assert_eq!(harness.writes(), writes, "{unsent:?}");
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert_eq!(journal["unresolved"], pending["unresolved"]);

    let fresh = harness.accepted_follow_up();
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["publication_owner"], json!(owner));
    assert_eq!(status["journal"]["owner_provider_write_possible"], false);
    let next = status["next"].as_str().unwrap();
    assert!(
        next.contains("rkc publish --effects review --plan"),
        "{status:#}"
    );
    let journal_file = harness
        .journal_dir()
        .unwrap()
        .join("publication-progress.json");
    let stored = fs::read(&journal_file).unwrap();
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned:#}");
    assert_eq!(planned["publication_owner_released"], json!(owner));
    assert!(
        planned["run"]["outcome"]["plan"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["kind"] == "finding"),
        "the plan is not limited by the owner's two-effect grant: {planned:#}"
    );
    assert_eq!(harness.writes(), writes, "planning writes nothing");
    assert_eq!(
        fs::read(&journal_file).unwrap(),
        stored,
        "planning persists nothing"
    );

    let notes = harness.writes_to("POST", "/merge_requests/7/notes");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], fresh);
    assert_eq!(journal["unresolved"], json!(null));
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/notes"),
        notes + 1
    );
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        1
    );
    assert_eq!(
        journal["authorization"]["effects"]
            .as_array()
            .unwrap()
            .len(),
        6,
        "the fresh run is not capped by the earlier two-effect grant"
    );

    harness.ok(&["run", "select", &owner]);
    let status = harness.ok(&["status"]);
    assert_ne!(status["journal"]["publication_owner"], json!(owner));
    assert_eq!(status["journal"]["publication_status"], Value::Null);
    let stopped = harness.ok(&["run", "stop", "--reason", "Superseded by a fresh run."]);
    assert_eq!(stopped["status"], "stopped");
}

#[test]
fn an_owner_with_only_prepared_intent_yields_to_a_fresh_run_after_the_merge_request_changes() {
    yields_to_a_fresh_run(Unsent::Prepared);
}

#[test]
fn an_owner_without_intent_after_a_rejected_resend_yields_to_a_fresh_run() {
    yields_to_a_fresh_run(Unsent::Rejected);
}

/// Comments outside the reviewer's threads do not stop the owner from resuming
/// a publication of which nothing reached GitLab.
#[test]
fn an_unsent_owner_resumes_its_publication_past_foreign_comments() {
    let harness = Harness::new();
    let owner = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_intent:1",
        &["publish", "--effects", "review"],
    );
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert_eq!(journal["dispatch_may_have_started"], false);
    assert_eq!(journal["unresolved"]["phase"], "prepared");
    harness.add_comment("question-thread", "Does the cause survive the new path?");
    harness.http.state.lock().unwrap().description = Some(json!("Edited after the crash."));
    let foreign = harness.http.state.lock().unwrap().discussions.clone();
    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(recovered["recovery"], true);
    assert_eq!(harness.manifest(&owner)["completion"], "published");
    assert_eq!(harness.writes_to("POST", "/merge_requests/7/notes"), 1);
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        1
    );
    let state = harness.http.state.lock().unwrap();
    assert_eq!(
        state.discussions[..foreign.len()],
        foreign[..],
        "every earlier discussion, including the foreign comment, is untouched"
    );
    assert_eq!(
        state.discussions.len(),
        foreign.len() + 1,
        "one new finding"
    );
}

/// A follow-up owner whose summary note someone edited after its crash cannot
/// publish its assessed snapshot; a fresh run releases its binding.
#[test]
fn an_unsent_follow_up_owner_whose_summary_changed_yields_to_a_fresh_run() {
    for unsent in [Unsent::Prepared, Unsent::Rejected] {
        follow_up_owner_yields_after_its_summary_changed(unsent);
    }
}

fn follow_up_owner_yields_after_its_summary_changed(unsent: Unsent) {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let owner = harness.follow_up_in(&harness.repo, false);
    crash(
        &harness,
        "publish.after_intent:1",
        &["publish", "--effects", "review"],
    );
    if let Unsent::Rejected = unsent {
        let summary = harness.http.state.lock().unwrap().notes[0]["id"].clone();
        harness
            .http
            .state
            .lock()
            .unwrap()
            .faults
            .push(Fault::response(
                "PUT",
                &format!("{MR_PATH}/notes/{summary}"),
                Response::status(422),
            ));
        let (output, rejected) = harness.rkc(&["publish", "--effects", "review"]);
        assert_eq!(output.status.code(), Some(7), "{rejected:#}");
        assert_eq!(rejected["provider_write_possible"], false, "{rejected:#}");
    }
    let pending = harness.journal();
    assert_eq!(pending["coverage_binding"]["run_id"], owner, "{unsent:?}");
    assert_eq!(pending["dispatch_may_have_started"], false, "{unsent:?}");
    match unsent {
        Unsent::Prepared => assert_eq!(pending["unresolved"]["phase"], "prepared"),
        Unsent::Rejected => assert_eq!(pending["unresolved"], Value::Null),
    }
    let summary = {
        let mut state = harness.http.state.lock().unwrap();
        let summary = &mut state.notes[0];
        let body = format!(
            "Maintainer note: see the description.\n\n{}",
            summary["body"].as_str().unwrap()
        );
        summary["body"] = json!(body);
        summary["id"].clone()
    };
    let writes = harness.writes();
    let failed = harness.fails(
        &["publish", "--effects", "review"],
        3,
        "conversation_changed",
    );
    assert_eq!(
        failed["diagnostics"][0]["changes"],
        json!([{"kind": "note_body_changed", "note": summary.to_string()}]),
        "{failed:#}"
    );
    assert!(
        failed["diagnostics"][0]["next"]
            .as_str()
            .unwrap()
            .contains("rkc run start --new"),
        "{failed:#}"
    );
    assert_eq!(harness.writes(), writes);
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert_eq!(journal["unresolved"], pending["unresolved"]);

    let fresh = harness.follow_up_in(&harness.repo, true);
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["publication_owner_released"], json!(owner));
    assert_eq!(harness.writes(), writes, "planning writes nothing");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.journal()["coverage_binding"]["run_id"], fresh);
    harness.ok(&["run", "select", &owner]);
    let stopped = harness.ok(&["run", "stop", "--reason", "Superseded by a fresh run."]);
    assert_eq!(stopped["status"], "stopped");
}

/// Guidance for a possibly dispatched creation that the new head prevents
/// resending: the documented options, not a recovery that cannot succeed.
fn assert_stranded(next: &Value) {
    let next = next.as_str().unwrap_or_default();
    assert!(next.contains("stays uncertain"), "{next:?}");
    assert!(next.contains("restore the reviewed revision"), "{next:?}");
    assert!(next.contains("different reviewer identity"), "{next:?}");
    assert!(!next.contains("publish again"), "{next:?}");
}

#[test]
fn an_owner_that_may_have_dispatched_keeps_uncertainty_and_blocks_replacement_after_a_new_head() {
    let harness = Harness::new();
    let owner = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_dispatch_flag:1",
        &["publish", "--effects", "review"],
    );
    harness.push_commit();
    let before = harness.journal();
    let (output, recovered) = harness.rkc(&["publish", "--effects", "review"]);
    assert!(!output.status.success(), "{recovered:#}");
    assert_stranded(&recovered["next"]);
    let (output, refused) = harness.rkc(&["publish", "--abandon", "--reason", "Changed."]);
    assert_eq!(output.status.code(), Some(2), "{refused:#}");
    assert_eq!(
        refused["abandon"]["diagnostics"][0]["code"],
        "not_abandonable"
    );
    assert_stranded(&refused["next"]);
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], owner);
    assert!(journal["unresolved"].is_object(), "uncertainty is kept");
    let snapshot = (
        journal["authorization"].clone(),
        journal["retried_creations"].clone(),
    );
    let writes = harness.writes();

    harness.accepted_follow_up();
    let refused = harness.fails(
        &["publish", "--effects", "review"],
        5,
        "publication_pending_owner",
    );
    assert_eq!(refused["diagnostics"][0]["owner_run_id"], owner);
    assert_stranded(&refused["diagnostics"][0]["next"]);
    assert_eq!(harness.writes(), writes, "no replacement writes");
    let after = harness.journal();
    assert_eq!(
        (
            after["authorization"].clone(),
            after["retried_creations"].clone()
        ),
        snapshot
    );
    assert_eq!(after["coverage_binding"], before["coverage_binding"]);
}

#[test]
fn an_owner_that_may_have_dispatched_reconciles_despite_a_later_comment() {
    let harness = Harness::new();
    let owner = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_dispatch_flag:1",
        &["publish", "--effects", "review"],
    );
    harness.add_comment("question-thread", "Does the cause survive the new path?");
    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(harness.manifest(&owner)["completion"], "published");
    assert_eq!(harness.writes_to("POST", "/merge_requests/7/notes"), 1);
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        1
    );
}

/// An owner that sent nothing and whose conversation a newer rkc wrote is told
/// that this rkc cannot resume it, with the fresh-run way forward.
#[test]
fn an_unsent_owner_with_a_newer_conversation_is_pointed_at_a_fresh_run() {
    let harness = Harness::new();
    let owner = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_intent:1",
        &["publish", "--effects", "review"],
    );
    let mut newer = harness.run_file_json(&owner, "review-conversation.json");
    newer["version"] = 3.into();
    harness.retain_conversation(&owner, &newer);
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["publication_status"], "bound_unsent");
    assert_eq!(status["retained_files"]["state"], "newer", "{status:#}");
    let next = status["next"].as_str().unwrap();
    assert!(next.contains("cannot resume it"), "{next}");
    assert!(
        next.contains("Resume it with that rkc version or a later one"),
        "{next}"
    );
    assert!(next.contains("rkc run start --new"), "{next}");
}

/// An owner that sent nothing resumes its retained summary write past a reply
/// in its own finding thread that this publication does not write to.
#[test]
fn an_unsent_follow_up_owner_resumes_past_a_reply_in_an_untouched_thread() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let owner = harness.follow_up_in(&harness.repo, false);
    crash(
        &harness,
        "publish.after_intent:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(harness.journal()["unresolved"]["phase"], "prepared");
    harness
        .http
        .state
        .lock()
        .unwrap()
        .question("discussion-1", "Is the cause still lost?");
    let untouched = harness.http.state.lock().unwrap().discussions[0].clone();
    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(recovered["recovery"], true);
    assert_eq!(harness.manifest(&owner)["completion"], "published");
    assert_eq!(
        harness.http.state.lock().unwrap().discussions[0],
        untouched,
        "the finding thread is untouched"
    );
}
