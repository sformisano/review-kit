use std::fs;

use serde_json::json;

use super::harness::{ERROR_POLICY, Harness, PROOF_POLICY, read_json, ready, sha256, write_json};

#[test]
fn start_accept_plan_and_publish_communicate_the_accepted_chain() {
    let harness = Harness::new();
    let started = harness.ok(&["run", "start", "--mr", &harness.mr()]);
    let run_id = started["run_id"].as_str().unwrap().to_owned();
    assert_eq!(started["draft"], "review-assessment.draft.json");
    assert_eq!(
        started["run_directory"],
        format!(".review-kit/review-runs/{run_id}")
    );
    assert_eq!(harness.selected(), run_id);
    let manifest = harness.manifest(&run_id);
    assert_eq!(manifest["status"], "open");
    assert_eq!(manifest["publication_attempted"], false);
    for (file, key) in [
        ("review-context.json", "review-context.json"),
        ("reviewed-source.json", "reviewed-source.json"),
        ("merge-request.diff", "merge-request.diff"),
        ("review-conversation.json", "review-conversation.json"),
    ] {
        assert_eq!(
            manifest["snapshot_files"][key],
            sha256(&fs::read(harness.file(&run_id, file)).unwrap())
        );
    }
    assert_eq!(
        fs::read_to_string(harness.runtime().join(".gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(harness.writes(), 0);

    harness.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
    assert!(harness.manifest(&run_id)["review_assessment"].is_null());
    assert!(
        !harness
            .file(&run_id, "policy-inventory.draft.json")
            .exists()
    );
    assert!(
        !harness
            .file(&run_id, "policy-applicability.draft.json")
            .exists()
    );
    harness.fill_finding(&run_id);
    let checked = harness.ok(&["check"]);
    assert_eq!(checked["status"], "valid", "{checked}");
    assert!(
        harness
            .file(&run_id, "rendered/review-summary.md")
            .is_file()
    );
    assert_eq!(harness.manifest(&run_id)["review_assessment"], json!(null));
    let accepted = harness.ok(&["accept"]);
    assert_eq!(accepted["status"], "stored", "{accepted}");
    let bytes = fs::read(harness.file(&run_id, "review-assessment.draft.json")).unwrap();
    assert_eq!(accepted["reference"], sha256(&bytes));
    assert_eq!(
        fs::read(harness.file(&run_id, accepted["path"].as_str().unwrap())).unwrap(),
        bytes
    );
    assert_eq!(
        harness.manifest(&run_id)["review_assessment"],
        accepted["reference"]
    );
    assert!(
        harness
            .file(&run_id, "review-render-profile.json")
            .is_file()
    );

    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned}");
    assert_eq!(harness.writes(), 0);
    assert!(
        harness.journal_dir().is_none(),
        "plan created journal state"
    );
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);

    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(published["settlement"]["outcome"], "published");
    let manifest = harness.manifest(&run_id);
    assert_eq!(manifest["status"], "completed");
    assert_eq!(manifest["completion"], "published");
    assert_eq!(manifest["publication_attempted"], true);
    assert_eq!(
        harness.writes_to("POST", "/notes"),
        1,
        "one summary creation"
    );
    assert_eq!(
        harness.writes_to("POST", "/discussions"),
        1,
        "one finding thread"
    );
    assert_eq!(harness.writes_to("PUT", ""), 1, "one summary completion");
    let journal = harness.journal();
    assert_eq!(journal["version"], 3);
    assert_eq!(journal["coverage_binding"]["run_id"], run_id);
    assert_eq!(journal["settled_runs"][0]["run_id"], run_id);
    assert_eq!(journal["settled_runs"][0]["outcome"], "published");
    assert_eq!(journal["dispatch_may_have_started"], true);
    assert!(
        harness
            .file(&run_id, "review-publication-events.json")
            .is_file()
    );
    assert!(
        harness
            .file(&run_id, "review-publication-result.json")
            .is_file()
    );

    let writes = harness.writes();
    let again = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(again["status"], "already_settled");
    assert_eq!(harness.writes(), writes);
    harness.fails(&["accept"], 5, "run_terminal");
}

#[test]
fn local_completion_and_stop_need_no_provider_writes() {
    let harness = Harness::new();
    let first = harness.draft_with_policies(&[ERROR_POLICY]);
    harness.fill_ready(&first);
    harness.ok(&["accept"]);
    let finished = harness.ok(&["run", "finish"]);
    assert_eq!(finished["status"], "completed");
    assert_eq!(finished["completion"], "local_only");
    assert_eq!(harness.manifest(&first)["completion"], "local_only");
    assert_eq!(harness.writes(), 0);
    for args in [
        vec!["accept"],
        vec!["revise", "--reason", "Reconsider the inventory."],
        vec!["run", "finish"],
        vec!["run", "stop", "--reason", "No longer needed."],
        vec!["publish", "--effects", "review"],
    ] {
        harness.fails(&args, 5, "run_terminal");
    }
    let status = harness.ok(&["status"]);
    assert_eq!(status["status"], "completed");
    assert_eq!(status["completion"], "local_only");

    let second = harness.ok(&["run", "start"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(second, first);
    harness.fails(&["run", "stop", "--reason", " "], 2, "blank_reason");
    let stopped = harness.ok(&["run", "stop", "--reason", "The policy list is unavailable."]);
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["reason"], "The policy list is unavailable.");
    let manifest = harness.manifest(&second);
    assert_eq!(manifest["status"], "stopped");
    assert_eq!(manifest["completion"], json!(null));
    assert!(
        harness
            .file(&second, "review-assessment.draft.json")
            .is_file()
    );
    harness.fails(&["accept"], 5, "run_terminal");
    assert_eq!(harness.writes(), 0);
}

#[test]
fn exact_acceptance_retry_keeps_the_same_snapshot_manifest_and_draft() {
    let harness = Harness::new();
    let run_id = harness.ready_run(false);
    let reference = harness.manifest(&run_id)["review_assessment"].clone();
    let manifest = harness.file(&run_id, "review-run.json");
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let manifest_bytes = fs::read(&manifest).unwrap();
    let draft_bytes = fs::read(&path).unwrap();
    let existing = harness.ok(&["accept"]);
    assert_eq!(existing["status"], "existing");
    assert_eq!(existing["reference"], reference);
    assert_eq!(fs::read(manifest).unwrap(), manifest_bytes);
    assert_eq!(fs::read(path).unwrap(), draft_bytes);
    assert_eq!(harness.ok(&["status"])["assessment"]["state"], "accepted");
    assert_eq!(harness.writes(), 0);
}

#[test]
fn run_start_help_is_text_with_success_exit() {
    let harness = Harness::new();
    let help = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_rkc"))
            .args(args)
            .current_dir(&harness.repo)
            .env("HOME", harness.root.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(help(&["run", "start", "--help"]).contains("--mr"));
    assert!(help(&["publish", "--help"]).contains("--abandon"));
    assert!(!help(&["check", "--help"]).contains("--assessment"));
    let commands = help(&["--help"]);
    assert!(!commands.contains("discover"), "{commands}");
    assert!(!commands.contains("  init"), "{commands}");
}

/// A09: a later run that repeats the published assessment exactly, with the
/// trusted configuration naming a review workflow.
#[test]
fn an_exact_unchanged_repeat_with_a_configured_workflow_settles_without_writes() {
    let harness = Harness::new();
    assert_eq!(
        read_json(&harness.config)["review_workflow"],
        "codebase-policies-review"
    );
    let first = harness.accepted_with_finding();
    let preview = fs::read_to_string(harness.file(&first, "rendered/review-summary.md")).unwrap();
    assert!(
        preview.contains("- Workflow: codebase-policies-review"),
        "{preview}"
    );
    harness.ok(&["publish", "--effects", "review"]);
    let writes = harness.writes();

    let second = harness.start();
    harness.declare_policies(&second, &[ERROR_POLICY, PROOF_POLICY]);

    let path = harness.file(&second, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    ready(&mut draft);
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind":"violated","evidence":"The failure path discards the actionable error."});
        }
    }
    write_json(&path, &draft);
    let checked = harness.ok(&["check"]);
    assert_eq!(checked["plan"]["unchanged"], true, "{checked:#}");
    let accepted = harness.ok(&["accept"]);
    assert_eq!(accepted["status"], "stored", "{accepted:#}");
    assert_eq!(accepted["plan"]["unchanged"], true);
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        planned["run"]["outcome"]["plan"]["unchanged"], true,
        "{planned:#}"
    );
    assert_eq!(planned["effects"]["summary_writes"], json!([]), "no writes");
    assert_eq!(planned["effects"]["outside"], json!([]));

    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(published["settlement"]["outcome"], "no_changes");
    assert_eq!(harness.writes(), writes, "no provider writes");
    assert_eq!(harness.manifest(&second)["completion"], "no_changes");
}

#[test]
fn an_excluded_policy_and_its_reason_remain_in_the_public_summary() {
    let harness = Harness::new();
    let run = harness.start();
    harness.declare_policies(&run, &[ERROR_POLICY, "rust-async-work-placement"]);

    let path = harness.file(&run, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["policies"][1]["outcome"] = json!({"kind":"excluded"});
    draft["policies"][1]["reason"] = json!("The change contains no asynchronous work.");
    write_json(&path, &draft);

    harness.fill_ready(&run);
    harness.ok(&["accept"]);
    let preview = fs::read_to_string(harness.file(&run, "rendered/review-summary.md")).unwrap();
    assert!(preview.contains("rust-async-work-placement"), "{preview}");
    assert!(
        preview.contains(
            "\n| Policy rust-async-work-placement | The change contains no asynchronous work. |\n"
        ),
        "{preview}"
    );
    assert_eq!(harness.writes(), 0);
}

#[test]
fn full_applicability_survives_publication_fresh_discovery_and_reason_changes() {
    let harness = Harness::new();
    let accept = |reason: &str| {
        let run = harness.start();
        harness.declare_policies(&run, &[ERROR_POLICY, "async-work", "unavailable-policy"]);
        let assessment = harness.file(&run, "review-assessment.draft.json");
        let mut value = read_json(&assessment);
        ready(&mut value);
        value["policies"][1]["outcome"] = json!({"kind":"excluded"});
        value["policies"][1]["reason"] = json!(reason);
        value["policies"][2]["outcome"] =
            json!({"kind":"unresolved","limitation":"The selected definition is unavailable."});
        value["policies"][2]["reason"] = json!("The selected definition is unavailable.");
        value["decision"] = json!("blocked");
        value["limitation"] = json!("The selected definition is unavailable.");
        write_json(&assessment, &value);
        harness.ok(&["accept"]);
        run
    };
    let first = accept("The change contains no asynchronous work.");
    let preview = fs::read_to_string(harness.file(&first, "rendered/review-summary.md")).unwrap();
    assert!(
        preview.contains("\n| Policy async-work | The change contains no asynchronous work. |\n"),
        "{preview}"
    );
    assert!(
        preview.contains(
            "\n| Policy unavailable-policy | Blocked | — | The selected definition is unavailable. |\n"
        ),
        "{preview}"
    );
    harness.ok(&["publish", "--effects", "review"]);
    assert!(harness.http.state.lock().unwrap().discussions.is_empty());
    fs::remove_dir_all(harness.runtime()).unwrap();
    let second = accept("Only synchronous control flow changes.");
    let captured = read_json(&harness.file(&second, "review-context.json"));
    let original = captured["snapshot"]["summaries"][0]["current"]["policy_declarations"].clone();
    assert_eq!(
        original
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["policy"]["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [ERROR_POLICY, "async-work", "unavailable-policy"]
    );
    assert_eq!(original[1]["status"], "excluded");
    assert_eq!(original[2]["status"], "unresolved");
    assert_eq!(
        original[1]["reason"],
        "The change contains no asynchronous work."
    );
    let plan = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(
        plan["run"]["outcome"]["plan"]["unchanged"], false,
        "{plan:#}"
    );
    harness.ok(&["publish", "--effects", "review"]);
    let writes = harness.writes();
    let third = accept("Only synchronous control flow changes.");
    let captured = read_json(&harness.file(&third, "review-context.json"));
    assert_eq!(
        captured["snapshot"]["summaries"][0]["history"][0]["policy_declarations"],
        original
    );
    assert_eq!(captured["snapshot"]["summaries"][0]["current"]["number"], 2);
    let repeat = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(repeat["settlement"]["outcome"], "no_changes", "{repeat:#}");
    assert_eq!(harness.writes(), writes);
    assert!(harness.http.state.lock().unwrap().discussions.is_empty());
}
