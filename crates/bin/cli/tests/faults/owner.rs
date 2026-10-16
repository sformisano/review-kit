//! A crashed owner whose merge request changed: who may publish next.

use std::fs;

use serde_json::{Value, json};

use super::{
    crash,
    harness::Harness,
    server::{Fault, MR_PATH, Response},
};

#[derive(Clone, Copy, Debug)]
enum Change {
    Head,
    Conversation,
}

fn change(harness: &Harness, change: Change) -> &'static str {
    match change {
        Change::Head => {
            harness.push_commit();
            "source_changed"
        }
        Change::Conversation => {
            harness.add_comment("question-thread", "Does the cause survive the new path?");
            "conversation_changed"
        }
    }
}

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

fn yields_to_a_fresh_run(kind: Change, unsent: Unsent) {
    let harness = Harness::new();
    let owner = unsent_owner(&harness, unsent);
    let pending = harness.journal();
    let status = harness.ok(&["status"]);
    assert_eq!(
        status["journal"]["publication_status"], "bound_unsent",
        "{kind:?} {unsent:?}"
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
    let code = change(&harness, kind);
    let writes = harness.writes();
    let failed = harness.fails(&["publish", "--effects", "review"], 3, code);
    assert!(
        failed["diagnostics"][0]["next"]
            .as_str()
            .unwrap()
            .contains("rkc run start --new"),
        "{failed:#}"
    );
    assert_eq!(harness.writes(), writes, "{kind:?} {unsent:?}");
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
    assert_eq!(published["status"], "settled", "{kind:?}: {published:#}");
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
        "{kind:?}: the fresh run is not capped by the earlier two-effect grant"
    );

    harness.ok(&["run", "select", &owner]);
    let status = harness.ok(&["status"]);
    assert_ne!(status["journal"]["publication_owner"], json!(owner));
    assert_eq!(status["journal"]["publication_status"], Value::Null);
    let stopped = harness.ok(&["run", "stop", "--reason", "Superseded by a fresh run."]);
    assert_eq!(stopped["status"], "stopped", "{kind:?}");
}

#[test]
fn an_owner_with_only_prepared_intent_yields_to_a_fresh_run_after_the_merge_request_changes() {
    for kind in [Change::Head, Change::Conversation] {
        yields_to_a_fresh_run(kind, Unsent::Prepared);
    }
}

#[test]
fn an_owner_without_intent_after_a_rejected_resend_yields_to_a_fresh_run() {
    for kind in [Change::Head, Change::Conversation] {
        yields_to_a_fresh_run(kind, Unsent::Rejected);
    }
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
    change(&harness, Change::Head);
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
    change(&harness, Change::Conversation);
    let recovered = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(recovered["status"], "settled", "{recovered:#}");
    assert_eq!(harness.manifest(&owner)["completion"], "published");
    assert_eq!(harness.writes_to("POST", "/merge_requests/7/notes"), 1);
    assert_eq!(
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        1
    );
}
