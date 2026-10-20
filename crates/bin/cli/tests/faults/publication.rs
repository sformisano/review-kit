use serde_json::{Value, json};

use super::{
    crash,
    harness::{Harness, git},
};

/// Writes the fake provider received: summary creations, finding threads and summary updates.
fn effects(harness: &Harness) -> (usize, usize, usize) {
    (
        harness.writes_to("POST", "/merge_requests/7/notes"),
        harness.writes_to("POST", "/merge_requests/7/discussions"),
        harness.writes_to("PUT", ""),
    )
}

fn publish(harness: &Harness) -> Value {
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    published
}

#[test]
fn crashes_before_any_binding_leave_an_unbound_journal_and_publish_cleanly_after_restart() {
    for (fault, attempted) in [
        ("publish.after_journal_load", false),
        ("publish.after_attempt_flag", true),
    ] {
        let harness = Harness::new();
        let run_id = harness.accepted_with_finding();
        crash(&harness, fault, &["publish", "--effects", "review"]);
        assert_eq!(harness.writes(), 0, "{fault}");
        let journal = harness.journal();
        assert_eq!(journal["coverage_binding"], json!(null), "{fault}");
        assert_eq!(journal["unresolved"], json!(null), "{fault}");
        assert_eq!(
            harness.manifest(&run_id)["publication_attempted"],
            attempted,
            "{fault}"
        );
        assert_eq!(
            harness.ok(&["status"])["journal"]["publication_owner"],
            json!(null)
        );
        publish(&harness);
        assert_eq!(effects(&harness), (1, 1, 1), "{fault}");
        assert_eq!(harness.manifest(&run_id)["completion"], "published");
    }
}

#[test]
fn crash_after_prepared_intent_resumes_after_preflight_without_duplicates() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_intent:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(harness.writes(), 0);
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], run_id);
    assert_eq!(journal["dispatch_may_have_started"], false);
    assert_eq!(journal["unresolved"]["phase"], "prepared");
    for args in [vec!["accept"], vec!["run", "stop", "--reason", "Stop."]] {
        harness.fails(&args, 5, "publication_pending_owner");
    }
    publish(&harness);
    assert_eq!(effects(&harness), (1, 1, 1));
}

#[test]
fn crash_after_dispatch_checkpoint_recovers_with_one_bounded_resend() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_dispatch_flag:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(harness.writes(), 0, "the abort precedes the HTTP request");
    let journal = harness.journal();
    assert_eq!(journal["dispatch_may_have_started"], true);
    assert_eq!(journal["unresolved"]["phase"], "dispatch_possible");
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["publication_status"], "pending");
    assert_eq!(status["journal"]["dispatch_may_have_started"], true);
    let published = publish(&harness);
    assert_eq!(published["recovery"], true);
    assert_eq!(published["publication"]["retried_creations"], 1);
    assert_eq!(effects(&harness), (1, 1, 1));
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
}

#[test]
fn retries_consumed_before_a_second_crash_are_not_granted_again() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_dispatch_flag:1",
        &["publish", "--effects", "review"],
    );
    crash(
        &harness,
        "publish.after_dispatch_flag:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(harness.writes(), 0);
    let journal = harness.journal();
    assert_eq!(journal["retried_creations"].as_array().unwrap().len(), 1);
    let (output, uncertain) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{uncertain:#}");
    assert_eq!(uncertain["status"], "unfinished");
    assert_eq!(
        harness.writes(),
        0,
        "the consumed retry is not granted again"
    );
    let after = harness.journal();
    assert_eq!(after["retried_creations"], journal["retried_creations"]);
    assert_eq!(after["unresolved"]["phase"], "dispatch_possible");
    assert_eq!(after["authorization"], journal["authorization"]);
}

#[test]
fn crash_before_readback_record_reconciles_the_committed_write() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    crash(
        &harness,
        "publish.before_readback_record:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(
        effects(&harness),
        (1, 0, 0),
        "the summary reached the provider"
    );
    assert_eq!(
        harness.journal()["unresolved"]["phase"],
        "dispatch_possible"
    );
    let published = publish(&harness);
    assert_eq!(published["publication"]["retried_creations"], 0);
    assert_eq!(
        effects(&harness),
        (1, 1, 1),
        "the visible summary is not recreated"
    );
}

#[test]
fn crash_after_a_verified_effect_continues_with_the_remaining_effects() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_verified:1",
        &["publish", "--effects", "review"],
    );
    assert_eq!(effects(&harness), (1, 0, 0));
    let journal = harness.journal();
    assert_eq!(journal["unresolved"], json!(null));
    assert_eq!(journal["dispatch_may_have_started"], true);
    publish(&harness);
    assert_eq!(effects(&harness), (1, 1, 1));
}

#[test]
fn crash_before_settlement_settles_on_restart_without_new_writes() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.before_settlement",
        &["publish", "--effects", "review"],
    );
    assert_eq!(effects(&harness), (1, 1, 1));
    let journal = harness.journal();
    assert_eq!(journal["settled_runs"], json!([]));
    assert_eq!(journal["coverage_binding"]["run_id"], run_id);
    assert_eq!(harness.manifest(&run_id)["status"], "open");
    let published = publish(&harness);
    assert_eq!(published["settlement"]["outcome"], "published");
    assert_eq!(effects(&harness), (1, 1, 1));
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
}

#[test]
fn crash_after_settlement_is_repaired_from_the_journal_and_never_relabelled() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_settlement",
        &["publish", "--effects", "review"],
    );
    assert_eq!(harness.journal()["settled_runs"][0]["run_id"], run_id);
    assert_eq!(harness.manifest(&run_id)["status"], "open");
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["settlement"]["outcome"], "published");
    assert!(status["next"].as_str().unwrap().contains("rkc publish"));
    assert_eq!(
        harness.manifest(&run_id)["status"],
        "open",
        "status never repairs"
    );
    let refused = harness.fails(&["run", "finish"], 5, "run_terminal");
    assert_eq!(
        refused["diagnostics"][0]["repaired_from_journal"]["outcome"],
        "published"
    );
    let manifest = harness.manifest(&run_id);
    assert_eq!(manifest["status"], "completed");
    assert_eq!(
        manifest["completion"], "published",
        "not relabelled local_only"
    );
    let again = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(again["status"], "already_settled");
    assert_eq!(effects(&harness), (1, 1, 1));
}

#[test]
fn a_later_prepared_effect_after_dispatch_keeps_the_flag_and_skips_conversation_preflight() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_intent:2",
        &["publish", "--effects", "review"],
    );
    assert_eq!(effects(&harness), (1, 0, 0));
    let journal = harness.journal();
    assert_eq!(journal["dispatch_may_have_started"], true);
    assert_eq!(journal["unresolved"]["phase"], "prepared");
    assert_eq!(journal["unresolved"]["write"]["kind"], "finding");
    harness.add_comment("late-thread", "Unrelated comment after dispatch.");
    publish(&harness);
    let (summaries, findings, updates) = effects(&harness);
    assert_eq!((summaries, findings), (1, 1), "no duplicate creations");
    assert!(updates >= 1);
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
}

#[test]
fn another_worktree_advancing_the_binding_keeps_the_earlier_settlement_for_repair() {
    let harness = Harness::new();
    let first = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_settlement",
        &["publish", "--effects", "review"],
    );
    let linked = harness.root.path().join("linked");
    git(
        &harness.repo,
        &["worktree", "add", "-q", linked.to_str().unwrap()],
    );
    let second = harness.follow_up_in(&linked, false);
    let published = harness.ok_in(&linked, &["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let journal = harness.journal();
    assert_eq!(journal["coverage_binding"]["run_id"], second);
    let settled: Vec<&Value> = journal["settled_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["run_id"])
        .collect();
    assert_eq!(settled, [&json!(first), &json!(second)]);
    let discussions = harness.discussions();

    assert_eq!(
        harness.ok(&["status"])["journal"]["settlement"]["run_id"],
        first
    );
    let repaired = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(repaired["status"], "already_settled");
    assert_eq!(repaired["repaired"], true);
    assert_eq!(harness.manifest(&first)["completion"], "published");
    assert_eq!(harness.discussions(), discussions);
}

#[test]
fn source_change_during_an_uncertain_write_reconciles_it_before_stopping() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.before_readback_record:2",
        &["publish", "--effects", "review"],
    );
    assert_eq!(
        effects(&harness),
        (1, 1, 0),
        "the finding thread reached the provider"
    );
    harness.push_commit();
    let (output, recovered) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(
        effects(&harness).1,
        1,
        "the uncertain finding is reconciled, not recreated: {recovered:#}"
    );
    assert_eq!(
        harness.journal()["unresolved"],
        json!(null),
        "{recovered:#}"
    );
    assert_eq!(output.status.code(), Some(3), "{recovered:#}");
    assert_eq!(recovered["status"], "unfinished");
    let diagnostic = &recovered["diagnostics"][0];
    assert_eq!(diagnostic["code"], "changed_revision", "{recovered:#}");
    for next in [&diagnostic["next"], &recovered["next"]] {
        let next = next.as_str().unwrap();
        assert!(
            next.contains("rkc publish --abandon --reason TEXT"),
            "{next}"
        );
        // Abandonment settles this run, so the fresh run needs no `--new`.
        assert!(
            next.contains("then start a fresh run with rkc run start."),
            "{next}"
        );
        assert!(!next.contains("--new"), "{next}");
        assert!(!next.contains("publish again"), "{next}");
    }
    assert_eq!(harness.manifest(&run_id)["status"], "open");
    let abandoned = harness.ok(&[
        "publish",
        "--abandon",
        "--reason",
        "The merge request changed during publication.",
    ]);
    assert_eq!(abandoned["status"], "settled", "{abandoned:#}");
    assert_eq!(harness.manifest(&run_id)["completion"], "abandoned");
    let fresh = harness.ok(&["run", "start"]);
    assert_eq!(fresh["status"], "created", "{fresh:#}");
    assert_eq!(effects(&harness).1, 1);
}

/// A settled run whose manifest is still open and whose conversation a newer
/// rkc wrote is told to record the settlement with that rkc.
#[test]
fn a_settled_run_with_a_newer_conversation_is_recorded_by_the_newer_rkc() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    crash(
        &harness,
        "publish.after_settlement",
        &["publish", "--effects", "review"],
    );
    let mut newer = harness.run_file_json(&run_id, "review-conversation.json");
    newer["version"] = 3.into();
    harness.retain_conversation(&run_id, &newer);
    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "newer", "{status:#}");
    let next = status["next"].as_str().unwrap();
    assert!(next.contains("settled publication"), "{next}");
    assert!(next.contains("a newer rkc wrote"), "{next}");
    assert!(next.contains("that rkc version or a later one"), "{next}");
}
