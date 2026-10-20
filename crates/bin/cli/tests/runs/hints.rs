//! `next` guidance for publication that cannot proceed as the usual recovery
//! hint says: local files that recovery reads are not intact, or a possibly
//! dispatched creation can no longer be resent.

use std::fs;

use serde_json::Value;

use super::{
    harness::{ERROR_POLICY, Harness, read_json, write_json},
    server::{Fault, MR_PATH},
};

const NARROW: [&str; 3] = ["publish", "--effects", "create_summary,update_summary"];
const RECOVERY_HINT: &str = "Run rkc publish with the originally permitted effects";

/// A run whose partial publication created its summary and stopped before the
/// finding: it owns an unfinished publication with a possible provider write.
fn partial_owner(harness: &Harness) -> String {
    let owner = harness.accepted_with_finding();
    let (output, partial) = harness.rkc(&NARROW);
    assert_eq!(output.status.code(), Some(4), "{partial:#}");
    assert_eq!(partial["status"], "unfinished");
    let status = harness.ok(&["status"]);
    assert_eq!(status["journal"]["publication_status"], "pending");
    assert!(next(&status).starts_with(RECOVERY_HINT), "{status:#}");
    owner
}

fn next(value: &Value) -> &str {
    value["next"].as_str().unwrap()
}

fn position(text: &str, part: &str) -> usize {
    let index = text.find(part);
    assert!(index.is_some(), "{part:?} is missing from {text:?}");
    index.unwrap()
}

/// Restoring comes first; abandonment is only the public alternative.
fn assert_restore_first(next: &str, subject: &str) {
    let restore = position(next, "Restore");
    assert!(restore < position(next, "rkc publish with"), "{next}");
    assert!(next.contains(subject), "{next}");
    let abandon = position(next, "rkc publish --abandon --reason TEXT");
    assert!(restore < abandon, "{next}");
    assert!(next.contains("cannot be restored"), "{next}");
    assert!(next.contains("publicly"), "{next}");
    assert!(
        next.contains("a fresh run with rkc run start reviews again"),
        "{next}"
    );
}

#[test]
fn status_of_an_owner_whose_accepted_snapshot_is_gone_says_to_restore_it_first() {
    let harness = Harness::new();
    let owner = partial_owner(&harness);
    let reference = harness.manifest(&owner)["review_assessment"]
        .as_str()
        .unwrap()
        .to_owned();
    let hex = reference.strip_prefix("sha256:").unwrap();
    let directory = harness.run_dir(&owner).join("accepted").join(hex);
    let aside = harness.root.path().join("accepted-aside");
    fs::rename(&directory, &aside).unwrap();

    let status = harness.ok(&["status"]);
    assert_eq!(status["assessment"]["state"], "missing_snapshot");
    let missing = next(&status);
    assert_restore_first(missing, &format!("assessment snapshot {reference}"));
    assert!(missing.contains(&format!("accepted/{hex}/")), "{missing}");
    assert!(missing.contains("missing"), "{missing}");
    harness.fails(
        &["publish", "--effects", "review"],
        2,
        "accepted_snapshot_missing",
    );

    fs::rename(&aside, &directory).unwrap();
    let file = directory.join("review-assessment.json");
    let mut snapshot = read_json(&file);
    snapshot["scope"] = Value::from("Edited after acceptance.");
    write_json(&file, &snapshot);
    let status = harness.ok(&["status"]);
    assert_eq!(status["assessment"]["state"], "damaged_snapshot");
    let damaged = next(&status);
    assert_restore_first(damaged, &format!("assessment snapshot {reference}"));
    assert!(damaged.contains("damaged"), "{damaged}");
}

#[test]
fn status_of_an_owner_with_damaged_retained_files_says_to_restore_them_first() {
    let harness = Harness::new();
    let owner = partial_owner(&harness);
    let conversation = harness.file(&owner, "review-conversation.json");
    let original = fs::read(&conversation).unwrap();
    fs::remove_file(&conversation).unwrap();

    let status = harness.ok(&["status"]);
    assert_eq!(status["retained_files"]["state"], "damaged");
    let damaged = next(&status);
    let restore = position(damaged, "Restore");
    assert!(restore < position(damaged, "rkc publish with"), "{damaged}");
    assert!(damaged.contains("review-conversation.json"), "{damaged}");
    assert!(
        !damaged.contains("--abandon"),
        "abandonment reads the same files: {damaged}"
    );
    harness.fails(
        &["publish", "--abandon", "--reason", "Gone."],
        2,
        "hash_mismatch",
    );

    fs::write(&conversation, original).unwrap();
    assert!(next(&harness.ok(&["status"])).starts_with(RECOVERY_HINT));
}

/// The documented options for a creation that stays uncertain, without the
/// unconditional "publish again" of a recoverable publication.
fn assert_stranded(next: &str, original_head: Option<&str>) {
    assert!(next.contains("stays uncertain"), "{next}");
    assert!(!next.contains("publish again"), "{next}");
    assert!(next.contains("late arrival"), "{next}");
    assert!(next.contains("different reviewer identity"), "{next}");
    assert!(next.contains("README.md#uncertain-writes"), "{next}");
    match original_head {
        Some(head) => {
            assert!(next.contains("restore the reviewed revision"), "{next}");
            assert!(next.contains(head), "{next}");
        }
        None => assert!(!next.contains("restore the reviewed revision"), "{next}"),
    }
}

fn lose_summary_creation(harness: &Harness) {
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/notes"), false));
}

/// A summary creation that may have been dispatched and never arrived.
fn uncertain_owner(harness: &Harness) -> String {
    let owner = harness.accepted_with_finding();
    lose_summary_creation(harness);
    let (output, uncertain) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{uncertain:#}");
    assert_eq!(uncertain["publication"]["status"], "uncertain");
    assert!(
        next(&uncertain).contains("rkc publish again"),
        "a resend is still possible: {uncertain:#}"
    );
    owner
}

#[test]
fn an_uncertain_creation_after_a_new_head_names_the_documented_options() {
    let harness = Harness::lost_responses();
    let owner = uncertain_owner(&harness);
    let head = harness.manifest(&owner)["revision"]["head"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.push_commit();
    let journal = harness.journal();
    let writes = harness.writes();

    let (output, recovered) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{recovered:#}");
    assert_eq!(
        recovered["run"]["outcome"]["result"]["stop"][0]["code"],
        "unresolved_write"
    );
    assert_stranded(next(&recovered), Some(&head));

    let (output, refused) = harness.rkc(&["publish", "--abandon", "--reason", "Changed."]);
    assert_eq!(output.status.code(), Some(2), "{refused:#}");
    assert_eq!(
        refused["abandon"]["diagnostics"][0]["code"],
        "not_abandonable"
    );
    assert_stranded(next(&refused), Some(&head));

    harness.accepted_follow_up();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let blocked = harness.fails(&args, 5, "publication_pending_owner");
        let diagnostic = &blocked["diagnostics"][0];
        assert_eq!(diagnostic["owner_run_id"], owner.as_str());
        let next = diagnostic["next"].as_str().unwrap();
        assert_stranded(next, Some(&head));
        assert!(next.contains(&format!("rkc run select {owner}")), "{next}");
    }
    assert_eq!(harness.writes(), writes, "guidance changes no write");
    assert_eq!(harness.journal()["unresolved"], journal["unresolved"]);
}

#[test]
fn an_uncertain_creation_whose_one_resend_was_used_names_the_documented_options() {
    let harness = Harness::lost_responses();
    let owner = uncertain_owner(&harness);
    lose_summary_creation(&harness);
    let (output, resent) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{resent:#}");
    assert_eq!(
        harness.journal()["retried_creations"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_stranded(next(&resent), None);

    let (output, recovered) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(6), "{recovered:#}");
    assert_stranded(next(&recovered), None);

    harness.accepted_follow_up();
    let blocked = harness.fails(
        &["publish", "--effects", "review"],
        5,
        "publication_pending_owner",
    );
    assert_eq!(blocked["diagnostics"][0]["owner_run_id"], owner.as_str());
    assert_stranded(blocked["diagnostics"][0]["next"].as_str().unwrap(), None);
}

/// The way back that a refusal of a changed profile for unfinished work names.
fn assert_restore_hint(next: &Value) {
    let next = next.as_str().unwrap_or_default();
    assert!(next.contains("exact profile"), "{next:?}");
    assert!(next.contains("--render-profile PATH"), "{next:?}");
    assert!(next.contains("render_profile in configuration"), "{next:?}");
    assert!(
        next.contains("Restore that profile's manifest and assets"),
        "{next:?}"
    );
}

/// Select an exported profile whose finding template differs from the default.
fn select_changed_profile(harness: &Harness) {
    let profile = harness.root.path().join("changed-profile");
    harness.ok(&[
        "render-profile",
        "export",
        "--dir",
        profile.to_str().unwrap(),
    ]);
    let manifest = read_json(&profile.join("profile.json"));
    let template = profile.join(
        manifest["artifacts"]["finding"]["template"]
            .as_str()
            .unwrap(),
    );
    let body = fs::read_to_string(&template).unwrap();
    fs::write(&template, format!("CHANGED PROFILE\n{body}")).unwrap();
    let mut config = read_json(&harness.config);
    config["render_profile"] = profile.join("profile.json").to_str().unwrap().into();
    write_json(&harness.config, &config);
}

#[test]
fn refusals_of_a_changed_profile_for_an_unfinished_round_name_the_way_back() {
    let harness = Harness::new();
    let owner = partial_owner(&harness);
    let follower = harness.ok(&["run", "start", "--new"])["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    harness.declare_policies(&follower, &[ERROR_POLICY]);

    harness.fill_ready(&follower);
    select_changed_profile(&harness);
    let writes = harness.writes();

    for args in [["check"], ["accept"]] {
        let (output, refused) = harness.rkc(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {refused:#}");
        assert_eq!(
            refused["diagnostics"][0]["code"], "render_profile_changed",
            "{args:?}: {refused:#}"
        );
        assert_restore_hint(&refused["next"]);
    }

    harness.ok(&["run", "select", &owner]);
    let refused = harness.fails(&NARROW, 2, "render_profile_changed");
    assert_restore_hint(&refused["diagnostics"][0]["next"]);
    assert_eq!(harness.writes(), writes, "refusals write nothing");
}
