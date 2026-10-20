//! The conversation preflight before a run's first possible dispatch: only
//! changes to what the publication writes to stop it, namely the reviewer's
//! summary note and the finding threads that receive a reply, a resolve or a
//! reopen. Replies in the summary's discussion, the reviewer's other threads,
//! activity by other people or bots elsewhere, and title or description edits
//! never block, and publication leaves them untouched.

use serde_json::{Value, json};

use super::{
    harness::{ERROR_POLICY, Harness, PROOF_POLICY, read_json, write_json},
    server::{Fault, MR_PATH},
};

/// The fixture's authenticated reviewer.
const REVIEWER: u64 = 42;
/// The merge request author.
const AUTHOR: u64 = 5;
/// Another human participant.
const COMMENTER: u64 = 77;
/// An automated account, such as a project access token's bot user.
const BOT: u64 = 91;

/// A text anchor on the changed `src/lib.rs` line 2 at the current head.
fn position(harness: &Harness) -> Value {
    let head = harness.http.state.lock().unwrap().head.clone();
    json!({"position_type":"text","base_sha":"2".repeat(40),"start_sha":"3".repeat(40),
        "head_sha":head,"old_path":"src/old.rs","new_path":"src/lib.rs","new_line":2,"old_line":null})
}

/// Add a general note as its own discussion and return the note ID.
fn general_note(harness: &Harness, discussion: &str, author: u64, body: &str) -> u64 {
    let mut state = harness.http.state.lock().unwrap();
    let note = state.note(body, author);
    let id = note["id"].as_u64().unwrap();
    state
        .discussions
        .push(json!({"id": discussion, "individual_note": true, "notes": [note]}));
    id
}

/// Start an unresolved diff thread on a changed line and return its root note ID.
fn diff_thread(harness: &Harness, discussion: &str, author: u64, body: &str) -> u64 {
    let position = position(harness);
    let mut state = harness.http.state.lock().unwrap();
    let mut note = state.note(body, author);
    note["type"] = json!("DiffNote");
    note["position"] = position;
    note["resolvable"] = json!(true);
    let id = note["id"].as_u64().unwrap();
    state
        .discussions
        .push(json!({"id": discussion, "individual_note": false, "notes": [note]}));
    id
}

/// Reply in a discussion with the resolution flags its root carries, as GitLab does.
fn reply(harness: &Harness, discussion: &str, author: u64, body: &str) -> u64 {
    let mut state = harness.http.state.lock().unwrap();
    let mut note = state.note(body, author);
    note["type"] = json!("DiscussionNote");
    let thread = state
        .discussions
        .iter_mut()
        .find(|thread| thread["id"] == discussion)
        .unwrap();
    note["resolvable"] = thread["notes"][0]["resolvable"].clone();
    note["resolved"] = thread["notes"][0]["resolved"].clone();
    let id = note["id"].as_u64().unwrap();
    thread["notes"].as_array_mut().unwrap().push(note);
    id
}

/// Add a system note to a discussion, as GitLab records some events there.
fn system_note(harness: &Harness, discussion: &str, body: &str) {
    let mut state = harness.http.state.lock().unwrap();
    let mut note = state.note(body, AUTHOR);
    note["system"] = json!(true);
    note["type"] = json!("DiscussionNote");
    state
        .discussions
        .iter_mut()
        .find(|thread| thread["id"] == discussion)
        .unwrap()["notes"]
        .as_array_mut()
        .unwrap()
        .push(note);
}

fn resolve(harness: &Harness, discussion: &str, resolved: bool) {
    let mut state = harness.http.state.lock().unwrap();
    let thread = state
        .discussions
        .iter_mut()
        .find(|thread| thread["id"] == discussion)
        .unwrap();
    for note in thread["notes"].as_array_mut().unwrap() {
        if note["resolvable"] == true {
            note["resolved"] = json!(resolved);
        }
    }
}

fn edit_note(harness: &Harness, note: u64, body: &str) {
    let mut state = harness.http.state.lock().unwrap();
    let state = &mut *state;
    let target = state
        .notes
        .iter_mut()
        .chain(
            state
                .discussions
                .iter_mut()
                .flat_map(|thread| thread["notes"].as_array_mut().unwrap().iter_mut()),
        )
        .find(|candidate| candidate["id"] == note)
        .unwrap();
    target["body"] = json!(body);
}

fn edit_title_and_description(harness: &Harness, suffix: &str) {
    let mut state = harness.http.state.lock().unwrap();
    state.title = Some(json!(format!("Fixture merge request {suffix}")));
    state.description = Some(json!(format!("Updated description {suffix}.")));
}

/// Every note of another author, with its discussion and resolution, in note order.
fn foreign_notes(harness: &Harness) -> Vec<Value> {
    let state = harness.http.state.lock().unwrap();
    let mut notes: Vec<Value> = state
        .discussions
        .iter()
        .flat_map(|thread| {
            thread["notes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|note| json!({"discussion": thread["id"], "note": note}))
        })
        .chain(
            state
                .notes
                .iter()
                .map(|note| json!({"discussion": null, "note": note})),
        )
        .filter(|entry| entry["note"]["author"]["id"] != REVIEWER)
        .collect();
    notes.sort_by_key(|entry| entry["note"]["id"].as_u64());
    notes
}

/// Provider writes as `(method, path)`, in request order.
fn writes(harness: &Harness) -> Vec<(String, String)> {
    harness
        .http
        .state
        .lock()
        .unwrap()
        .writes()
        .iter()
        .map(|request| (request.method.clone(), request.path.clone()))
        .collect()
}

fn summary_note(harness: &Harness) -> u64 {
    harness.http.state.lock().unwrap().notes[0]["id"]
        .as_u64()
        .unwrap()
}

/// Foreign activity outside the reviewer's threads: notes by a person and a
/// bot, a diff thread with a bot reply and a resolution, a thread and a note
/// that copy Review Kit markers, and title and description edits.
fn foreign_activity(harness: &Harness, label: &str) {
    general_note(
        harness,
        &format!("{label}-commenter"),
        COMMENTER,
        "Could the error path log its cause?",
    );
    general_note(
        harness,
        &format!("{label}-bot"),
        BOT,
        "Automated review: the change looks consistent.",
    );
    let thread = format!("{label}-thread");
    diff_thread(
        harness,
        &thread,
        COMMENTER,
        "This added line deserves a comment.",
    );
    reply(
        harness,
        &thread,
        BOT,
        "Suggested wording: explain the fallback.",
    );
    resolve(harness, &thread, true);
    diff_thread(
        harness,
        &format!("{label}-copied-finding"),
        COMMENTER,
        "Copied review note.\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":1} -->",
    );
    general_note(
        harness,
        &format!("{label}-copied-summary"),
        BOT,
        "Copied summary.\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
    );
    edit_title_and_description(harness, label);
}

/// The reviewer's finding root discussions, in stored order.
fn owned_threads(harness: &Harness) -> Vec<String> {
    harness
        .http
        .state
        .lock()
        .unwrap()
        .discussions
        .iter()
        .filter(|thread| thread["notes"][0]["author"]["id"] == REVIEWER)
        .map(|thread| thread["id"].as_str().unwrap().to_owned())
        .collect()
}

/// A round 1 publication, then a follow-up that answers a question asked in
/// the finding thread before the follow-up's capture.
struct FollowUp {
    run_id: String,
    /// The finding root discussion.
    thread: String,
    question: u64,
}

/// Publish round 1 with finding 1, add a question to its thread, then capture
/// and accept a follow-up that answers it.
fn answering_follow_up(harness: &Harness) -> FollowUp {
    harness.accepted_with_finding();
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let [thread] = owned_threads(harness).try_into().unwrap();
    let question = reply(
        harness,
        &thread,
        AUTHOR,
        "Does the caller now receive the cause?",
    );
    let run_id = harness.start();
    harness.declare_policies(&run_id, &[ERROR_POLICY, PROOF_POLICY]);
    harness.fill_carried_open(&run_id);
    let path = harness.file(&run_id, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    draft["answers"] = json!([{"finding": 1, "in_reply_to": question.to_string(),
        "body": "Not yet: the failure branch still returns without the cause."}]);
    write_json(&path, &draft);
    harness.ok(&["accept"]);
    FollowUp {
        run_id,
        thread,
        question,
    }
}

/// A first publication, with no reviewer thread yet, writes exactly its plan
/// after foreign and bot activity and leaves that activity untouched.
#[test]
fn foreign_and_bot_activity_after_capture_does_not_block_a_first_publication() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    foreign_activity(&harness, "late");
    let foreign = foreign_notes(&harness);

    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned:#}");
    let actions = planned["run"]["outcome"]["plan"]["actions"]
        .as_array()
        .unwrap();
    assert_eq!(actions.len(), 1, "{planned:#}");
    assert_eq!(actions[0]["kind"], "finding");
    assert_eq!(writes(&harness), []);

    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.manifest(&run_id)["completion"], "published");
    let result = &published["run"]["outcome"]["result"];
    assert_eq!(result["status"], "completed", "{published:#}");
    assert_eq!(result["round"], 1);
    assert_eq!(
        result["planned"],
        planned["run"]["outcome"]["plan"]["actions"]
    );
    assert_eq!(
        writes(&harness),
        [
            ("POST".to_owned(), format!("{MR_PATH}/notes")),
            ("POST".to_owned(), format!("{MR_PATH}/discussions")),
            (
                "PUT".to_owned(),
                format!("{MR_PATH}/notes/{}", summary_note(&harness))
            ),
        ],
        "the pending summary, the finding root and the final summary"
    );
    assert_eq!(
        foreign_notes(&harness),
        foreign,
        "foreign notes are untouched"
    );
    let owned = owned_threads(&harness);
    assert_eq!(owned.len(), 1, "one reviewer thread was created");
    let finding = &result["final_snapshot"]["findings"];
    assert_eq!(finding.as_array().unwrap().len(), 1, "{published:#}");
    assert_eq!(
        finding[0]["id"], owned[0],
        "a copied finding marker is not an owned finding root"
    );
    assert_eq!(
        result["final_snapshot"]["summaries"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a copied summary marker is not an owned summary"
    );
}

/// A follow-up writes exactly its plan after foreign activity, a foreign note
/// edit and a system note inside its own finding thread.
#[test]
fn foreign_activity_outside_write_targets_does_not_block_a_follow_up() {
    let harness = Harness::new();
    let bot_note = general_note(&harness, "early-bot", BOT, "Pipeline report: passed.");
    let follow_up = answering_follow_up(&harness);
    foreign_activity(&harness, "late");
    edit_note(&harness, bot_note, "Pipeline report: passed after a rerun.");
    system_note(
        &harness,
        &follow_up.thread,
        "changed this line in version 2 of the diff",
    );
    let foreign = foreign_notes(&harness);
    let before = writes(&harness).len();

    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned:#}");
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(
        harness.manifest(&follow_up.run_id)["completion"],
        "published"
    );
    let result = &published["run"]["outcome"]["result"];
    assert_eq!(result["status"], "completed", "{published:#}");
    assert_eq!(result["round"], 2);
    assert_eq!(
        result["planned"],
        planned["run"]["outcome"]["plan"]["actions"]
    );
    let summary = format!("{MR_PATH}/notes/{}", summary_note(&harness));
    assert_eq!(
        writes(&harness)[before..],
        [
            ("PUT".to_owned(), summary.clone()),
            (
                "POST".to_owned(),
                format!("{MR_PATH}/discussions/{}/notes", follow_up.thread)
            ),
            ("PUT".to_owned(), summary),
        ],
        "the pending summary, the answer and the final summary"
    );
    assert_eq!(
        foreign_notes(&harness),
        foreign,
        "foreign notes are untouched"
    );
    let state = harness.http.state.lock().unwrap();
    let thread = state
        .discussions
        .iter()
        .find(|thread| thread["id"] == follow_up.thread.as_str())
        .unwrap();
    let answer = thread["notes"].as_array().unwrap().last().unwrap();
    assert_eq!(answer["author"]["id"], REVIEWER);
    assert!(
        answer["body"].as_str().unwrap().ends_with(&format!(
            "<!-- review-kit:v1 {{\"kind\":\"answer\",\"finding\":1,\"round\":2,\"note\":\"{}\"}} -->",
            follow_up.question
        )),
        "{answer:#}"
    );
}

/// A change to the follow-up's write targets after its capture: the
/// answered finding thread or the summary note.
#[derive(Clone, Copy, Debug)]
enum OwnedChange {
    /// The author replies in the finding thread.
    Reply,
    /// The author resolves the finding thread.
    Resolution,
    /// The question in the finding thread is edited.
    EditedNote,
    /// The question in the finding thread is deleted.
    DeletedNote,
    /// A maintainer edits the reviewer's summary note.
    EditedSummary,
}

/// Apply `change` and return the one change detail the preflight reports.
fn apply(harness: &Harness, change: OwnedChange, follow_up: &FollowUp) -> Value {
    let FollowUp {
        thread, question, ..
    } = follow_up;
    match change {
        OwnedChange::Reply => {
            let note = reply(
                harness,
                thread,
                AUTHOR,
                "One more case: the timeout branch.",
            );
            json!({"kind": "note_added", "discussion": thread, "note": note.to_string()})
        }
        OwnedChange::Resolution => {
            resolve(harness, thread, true);
            json!({"kind": "resolution_changed", "discussion": thread})
        }
        OwnedChange::EditedNote => {
            edit_note(
                harness,
                *question,
                "Does the caller receive the cause and context?",
            );
            json!({"kind": "note_body_changed", "note": question.to_string()})
        }
        OwnedChange::DeletedNote => {
            let mut state = harness.http.state.lock().unwrap();
            let thread = state
                .discussions
                .iter_mut()
                .find(|candidate| candidate["id"] == thread.as_str())
                .unwrap();
            thread["notes"]
                .as_array_mut()
                .unwrap()
                .retain(|note| note["id"] != *question);
            json!({"kind": "note_removed", "discussion": follow_up.thread, "note": question.to_string()})
        }
        OwnedChange::EditedSummary => {
            let note = summary_note(harness);
            let mut state = harness.http.state.lock().unwrap();
            let summary = &mut state.notes[0];
            let body = format!(
                "Maintainer note: see the description.\n\n{}",
                summary["body"].as_str().unwrap()
            );
            summary["body"] = json!(body);
            json!({"kind": "note_body_changed", "note": note.to_string()})
        }
    }
}

#[test]
fn changes_to_write_targets_block_before_the_first_dispatch() {
    for change in [
        OwnedChange::Reply,
        OwnedChange::Resolution,
        OwnedChange::EditedNote,
        OwnedChange::DeletedNote,
        OwnedChange::EditedSummary,
    ] {
        let harness = Harness::new();
        let follow_up = answering_follow_up(&harness);
        let journal = harness.journal();
        let expected = apply(&harness, change, &follow_up);
        foreign_activity(&harness, "late");
        let before = writes(&harness);
        for args in [
            vec!["publish", "--effects", "review", "--plan"],
            vec!["publish", "--effects", "review"],
        ] {
            let refused = harness.fails(&args, 3, "conversation_changed");
            assert_eq!(
                refused["diagnostics"][0]["changes"],
                json!([expected]),
                "{change:?} {args:?}: only the write-target change is listed"
            );
            assert_stop_then_start(&refused);
        }
        assert_eq!(writes(&harness), before, "{change:?}: nothing is written");
        assert_eq!(
            harness.manifest(&follow_up.run_id)["publication_attempted"],
            false
        );
        assert_eq!(harness.journal(), journal, "{change:?}");
    }
}

/// A refused run without a binding is stopped before the new run starts, so
/// `run start` needs no `--new`.
fn assert_stop_then_start(refused: &Value) {
    let message = refused["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains(
            "stop this run with rkc run stop --reason TEXT, then start a new run with rkc run start"
        ),
        "{refused:#}"
    );
    assert!(!message.contains("--new"), "{refused:#}");
}

/// Discovery reports a deleted owned thread as a managed-history conflict
/// before the conversation preflight runs.
#[test]
fn a_deleted_owned_thread_stops_publication_as_a_history_conflict() {
    let harness = Harness::new();
    let follow_up = answering_follow_up(&harness);
    harness
        .http
        .state
        .lock()
        .unwrap()
        .discussions
        .retain(|thread| thread["id"] != follow_up.thread.as_str());
    let before = writes(&harness);
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let refused = harness.fails(&args, 5, "managed_history_conflict");
        assert_eq!(
            refused["diagnostics"][0]["conflict"],
            "missing_finding_root"
        );
    }
    assert_eq!(writes(&harness), before);
    assert_eq!(
        harness.manifest(&follow_up.run_id)["publication_attempted"],
        false
    );
}

/// The discussion with `id`, as the fixture lists it.
fn thread_json(harness: &Harness, id: &str) -> Value {
    harness
        .http
        .state
        .lock()
        .unwrap()
        .listed_discussions()
        .into_iter()
        .find(|thread| thread["id"] == id)
        .unwrap()
}

/// Publish round 1 with finding 1, then capture and accept a follow-up that
/// keeps it open and only updates the summary.
///
/// Returns the follow-up run ID and finding 1's thread.
fn summary_only_follow_up(harness: &Harness) -> (String, String) {
    harness.accepted_with_finding();
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    let [thread] = owned_threads(harness).try_into().unwrap();
    let run_id = harness.follow_up_in(&harness.repo, false);
    (run_id, thread)
}

/// Publish a follow-up that writes only the summary, and require exactly its
/// two summary updates.
fn publishes_summary_only(harness: &Harness, run_id: &str) {
    let before = writes(harness).len();
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    assert_eq!(planned["status"], "planned", "{planned:#}");
    assert_eq!(planned["run"]["outcome"]["plan"]["actions"], json!([]));
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.manifest(run_id)["completion"], "published");
    let summary = format!("{MR_PATH}/notes/{}", summary_note(harness));
    assert_eq!(
        writes(harness)[before..],
        [
            ("PUT".to_owned(), summary.clone()),
            ("PUT".to_owned(), summary)
        ],
        "the pending and the final summary"
    );
}

/// Replies by a person and a bot and a resolution change in the reviewer's
/// own finding thread do not block a publication that writes nothing there.
#[test]
fn changes_in_an_own_thread_without_a_planned_effect_do_not_block() {
    let harness = Harness::new();
    let (run_id, thread) = summary_only_follow_up(&harness);
    reply(&harness, &thread, AUTHOR, "I disagree with this finding.");
    reply(
        &harness,
        &thread,
        BOT,
        "Automated note: the linked issue moved.",
    );
    resolve(&harness, &thread, true);
    let untouched = thread_json(&harness, &thread);
    publishes_summary_only(&harness, &run_id);
    assert_eq!(
        thread_json(&harness, &thread),
        untouched,
        "the thread is untouched"
    );
}

/// A reply in the summary note's discussion does not block; the summary
/// update proceeds and the reply stays.
#[test]
fn a_reply_to_the_summary_does_not_block() {
    let harness = Harness::new();
    let (run_id, _) = summary_only_follow_up(&harness);
    let summary = summary_note(&harness);
    let reply = {
        let mut state = harness.http.state.lock().unwrap();
        let mut note = state.note("Thanks, I will look at the summary.", AUTHOR);
        note["type"] = json!("DiscussionNote");
        state.note_replies.insert(summary, vec![note.clone()]);
        note
    };
    publishes_summary_only(&harness, &run_id);
    let listed = thread_json(&harness, &format!("note-{summary}"));
    assert_eq!(listed["notes"][1], reply, "the reply is untouched");
    assert_ne!(
        listed["notes"][0]["body"],
        json!(null),
        "the summary note was updated in place"
    );
}

/// The follow-up's planned effect on finding 1: a transition and resolution
/// change, or a reopening after an earlier fix.
#[derive(Clone, Copy, Debug)]
enum PlannedEffect {
    /// Open to fixed: a transition reply and a resolve.
    Fixed,
    /// Fixed to open after a published fix: a transition reply and a reopen.
    Recurrence,
}

/// A thread that receives a transition reply and a resolution change in this
/// publication is compared: a later reply there blocks before any write.
#[test]
fn a_thread_with_a_planned_transition_and_resolution_change_is_compared() {
    for effect in [PlannedEffect::Fixed, PlannedEffect::Recurrence] {
        let harness = Harness::new();
        harness.accepted_with_finding();
        harness.ok(&["publish", "--effects", "review"]);
        let [thread] = owned_threads(&harness).try_into().unwrap();
        if let PlannedEffect::Recurrence = effect {
            harness.transition_follow_up("open", "fixed");
            let fixed = harness.ok(&["publish", "--effects", "review"]);
            assert_eq!(fixed["status"], "settled", "{fixed:#}");
            assert_eq!(thread_json(&harness, &thread)["notes"][0]["resolved"], true);
        }
        let run_id = match effect {
            PlannedEffect::Fixed => harness.transition_follow_up("open", "fixed"),
            PlannedEffect::Recurrence => harness.transition_follow_up("fixed", "open"),
        };
        let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
        let kinds: Vec<_> = planned["run"]["outcome"]["plan"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|action| action["kind"].clone())
            .collect();
        assert_eq!(kinds, [json!("reply"), json!("set_resolved")], "{effect:?}");
        let note = reply(&harness, &thread, BOT, "Automated note: please recheck.");
        let before = writes(&harness);
        for args in [
            vec!["publish", "--effects", "review", "--plan"],
            vec!["publish", "--effects", "review"],
        ] {
            let refused = harness.fails(&args, 3, "conversation_changed");
            assert_eq!(
                refused["diagnostics"][0]["changes"],
                json!([{"kind": "note_added", "discussion": thread, "note": note.to_string()}]),
                "{effect:?} {args:?}"
            );
        }
        assert_eq!(writes(&harness), before, "{effect:?}: nothing is written");
        assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);
    }
}

/// When someone deletes the reviewer's summary note after capture, discovery
/// stops publication with a managed-history conflict before any write.
#[test]
fn a_deleted_summary_note_stops_publication_as_a_history_conflict() {
    let harness = Harness::new();
    let (run_id, _) = summary_only_follow_up(&harness);
    harness.http.state.lock().unwrap().notes.clear();
    let before = writes(&harness);
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        harness.fails(&args, 5, "managed_history_conflict");
    }
    assert_eq!(writes(&harness), before, "nothing is written");
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);
}

/// A follow-up that writes only the summary still compares the summary note:
/// a maintainer's edit to it blocks before any write.
#[test]
fn an_edited_summary_note_blocks_a_summary_only_follow_up() {
    let harness = Harness::new();
    let (run_id, thread) = summary_only_follow_up(&harness);
    let follow_up = FollowUp {
        run_id,
        thread,
        question: 0,
    };
    let expected = apply(&harness, OwnedChange::EditedSummary, &follow_up);
    let before = writes(&harness);
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let refused = harness.fails(&args, 3, "conversation_changed");
        assert_eq!(refused["diagnostics"][0]["changes"], json!([expected]));
    }
    assert_eq!(writes(&harness), before, "nothing is written");
    assert_eq!(
        harness.manifest(&follow_up.run_id)["publication_attempted"],
        false
    );
}

/// A first publication writes to no existing thread, so after foreign
/// activity only a changed source stops it.
#[test]
fn a_first_publication_is_stopped_only_by_a_changed_source() {
    let harness = Harness::new();
    let run_id = harness.accepted_with_finding();
    foreign_activity(&harness, "late");
    harness.push_commit();
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        assert_stop_then_start(&harness.fails(&args, 3, "source_changed"));
    }
    assert_eq!(writes(&harness), []);
    assert_eq!(harness.manifest(&run_id)["publication_attempted"], false);
}

/// The targets come from the plan against the current history, which is what
/// publication executes. A run captured as an exact repeat of round 1 plans
/// no write against its capture; after another run publishes round 2, its
/// publication would write the summary, so the summary's change blocks it.
#[test]
fn targets_follow_the_plan_against_the_current_history() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    harness.ok(&["publish", "--effects", "review"]);
    let repeat = harness.start();
    harness.declare_policies(&repeat, &[ERROR_POLICY, PROOF_POLICY]);
    let path = harness.file(&repeat, "review-assessment.draft.json");
    let mut draft = read_json(&path);
    super::harness::ready(&mut draft);
    draft["decision"] = json!("changes_required");
    for policy in draft["policies"].as_array_mut().unwrap() {
        if policy["policy_id"] == ERROR_POLICY {
            policy["outcome"] = json!({"kind":"violated","evidence":"The failure path discards the actionable error."});
        }
    }
    write_json(&path, &draft);
    assert_eq!(harness.ok(&["accept"])["plan"]["unchanged"], true);

    let newer = harness.follow_up_in(&harness.repo, true);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.manifest(&newer)["completion"], "published");

    harness.ok(&["run", "select", &repeat]);
    let summary = summary_note(&harness);
    let before = writes(&harness);
    for args in [
        vec!["publish", "--effects", "review", "--plan"],
        vec!["publish", "--effects", "review"],
    ] {
        let refused = harness.fails(&args, 3, "conversation_changed");
        assert_eq!(
            refused["diagnostics"][0]["changes"],
            json!([{"kind": "note_body_changed", "note": summary.to_string()}]),
            "{args:?}"
        );
    }
    assert_eq!(writes(&harness), before, "nothing is written");
    assert_eq!(harness.manifest(&repeat)["publication_attempted"], false);
}

/// The publication engine plans again from its own discovery and refuses a
/// round that would write outside the compared targets before recording any
/// intent. Here the summary note moves to another ID after the preflight
/// captured the conversation and before the engine discovers the history.
#[test]
fn the_engine_refuses_a_write_outside_the_compared_targets() {
    let harness = Harness::new();
    let (run_id, _) = summary_only_follow_up(&harness);
    let summary = summary_note(&harness);
    let moved = summary + 1000;
    harness
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::change_after_read(
            &format!("{MR_PATH}/discussions"),
            1,
            move |state| state.notes[0]["id"] = json!(moved),
        ));
    let journal = harness.journal();
    let before = writes(&harness);
    let refused = harness.fails(&["publish", "--effects", "review"], 3, "uncompared_write");
    let next = refused["next"].as_str().unwrap();
    assert!(
        next.contains(
            "Stop this run with rkc run stop --reason TEXT, then start a fresh run with rkc run start."
        ),
        "{next}"
    );
    assert!(!next.contains("--new"), "{next}");
    assert_eq!(refused["diagnostics"][0]["next"], refused["next"]);
    assert_eq!(
        refused["run"]["outcome"]["diagnostics"][0]["code"], "uncompared_write",
        "{refused:#}"
    );
    assert_eq!(writes(&harness), before, "nothing is written");
    assert_eq!(
        harness.journal()["coverage_binding"],
        journal["coverage_binding"]
    );
    assert_eq!(harness.journal()["unresolved"], json!(null));
    assert_eq!(harness.manifest(&run_id)["completion"], json!(null));

    // The refused run is unbound, so it stops; a fresh run without `--new`
    // captures the moved summary and publishes.
    let stopped = harness.ok(&["run", "stop", "--reason", "The summary moved."]);
    assert_eq!(stopped["status"], "stopped", "{stopped:#}");
    let fresh = harness.follow_up_in(&harness.repo, false);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(harness.manifest(&fresh)["completion"], "published");
    assert_eq!(summary_note(&harness), moved);
}

/// A change that makes the current plan fail, here a deleted answered note,
/// is still compared through the captured plan's targets and reported; the
/// operation log keeps why the current plan failed.
#[test]
fn a_change_that_breaks_the_current_plan_is_reported_and_its_reason_logged() {
    let harness = Harness::new();
    let follow_up = answering_follow_up(&harness);
    let expected = apply(&harness, OwnedChange::DeletedNote, &follow_up);
    let refused = harness.fails(
        &["publish", "--effects", "review"],
        3,
        "conversation_changed",
    );
    assert_eq!(refused["diagnostics"][0]["changes"], json!([expected]));
    let log = std::fs::read_to_string(
        harness
            .root
            .path()
            .join(".review-kit/logs/review-kit.jsonl"),
    )
    .unwrap();
    let failed: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["execution"].clone())
        .filter(|record| {
            record["operation"] == "application.publication.check_review_with_progress"
                && record["outcome"] == "failure"
        })
        .collect();
    assert_eq!(failed.len(), 1, "{failed:#?}");
    assert_eq!(
        failed[0]["error"]["fields"]["issues"][0]["error"]["variant"], "invalid_answer",
        "{failed:#?}"
    );
}
