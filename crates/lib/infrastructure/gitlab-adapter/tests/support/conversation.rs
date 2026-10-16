use super::support::PublicationProgress;

use review_kit_application::{
    conversation::{Conversation, ConversationChange, Resolution},
    publication::{Effects, ProviderId},
    use_cases::publishing::publish,
};
use review_kit_domain::{FindingStatus, Subject};
use review_kit_gitlab_adapter::{GitLabError, Limits};
use review_kit_json_adapter::{ConversationDocument, canonical_position, decode, encode};
use serde_json::{Value, json};

use super::support::{Fault, Fixture, MR_PATH, Renderer, Response, finding, request};

fn capture(fixture: &Fixture) -> Result<Conversation, GitLabError> {
    let client = fixture.connect();
    client.capture_conversation(client.subject())
}

fn id(value: &str) -> ProviderId {
    ProviderId::new(value.into()).unwrap()
}

fn thread(id: &str, individual: bool, notes: Vec<Value>) -> Value {
    json!({"id":id,"individual_note":individual,"notes":notes})
}

fn inline_position() -> Value {
    json!({
        "base_sha":"2".repeat(40),"start_sha":"3".repeat(40),"head_sha":"1".repeat(40),
        "old_path":"src/old.rs","new_path":"src/lib.rs","position_type":"text",
        "old_line":null,"new_line":2,
        "line_range":{"start":{"line_code":"a_1_2","type":"new","old_line":null,"new_line":2},
            "end":{"line_code":"a_1_2","type":"new","old_line":null,"new_line":2}},
        "x_unfamiliar":{"nested":[1,"two",{"three":3.5}]}
    })
}

/// Seeds every supported and unsupported note shape behind more than one full
/// page of discussions.
fn seed(fixture: &Fixture) -> Value {
    let mut state = fixture.state.lock().unwrap();
    for index in 0..120 {
        let note = state.note(&format!("Foreign note {index}"), 77);
        state
            .discussions
            .push(thread(&format!("filler-{index}"), true, vec![note]));
    }
    let mut system = state.note("added 1 commit", 1);
    system["system"] = true.into();
    state
        .discussions
        .push(thread("system-only", true, vec![system]));
    let mut bot = state.note("Pipeline bot report", 900);
    bot["author"]["bot"] = true.into();
    state.discussions.push(thread("bot", true, vec![bot]));
    let mut root = state.note("This anchor needs a test.", 77);
    root["type"] = "DiffNote".into();
    root["position"] = inline_position();
    root["resolvable"] = true.into();
    root["resolved"] = false.into();
    let mut changed = state.note("changed this line in version 2", 1);
    changed["system"] = true.into();
    let mut reply = state.note("Agreed.", 78);
    reply["type"] = "DiscussionNote".into();
    reply["resolvable"] = true.into();
    reply["resolved"] = true.into();
    state
        .discussions
        .push(thread("inline", false, vec![root, changed, reply]));
    let mut general = state.note("General resolved thread.", 78);
    general["type"] = "DiscussionNote".into();
    general["resolvable"] = true.into();
    general["resolved"] = true.into();
    state
        .discussions
        .push(thread("general-thread", false, vec![general]));
    inline_position()
}

// D09: general, bot and unmanaged inline notes are captured as context beyond
// the first discussions page; system notes are not part of the conversation.
#[test]
fn capture_exhausts_discussion_pages_and_keeps_supported_notes_verbatim() {
    let fixture = Fixture::new();
    let position = seed(&fixture);
    fixture.state.lock().unwrap().requests.clear();
    let conversation = capture(&fixture).unwrap();

    assert_eq!(conversation.title(), "Fixture merge request");
    assert_eq!(conversation.description(), "Fixture description");
    let ids: Vec<_> = conversation
        .discussions()
        .iter()
        .map(|discussion| discussion.id().as_str())
        .collect();
    assert_eq!(ids.len(), 123);
    assert!(!ids.contains(&"system-only"));
    assert_eq!(ids[120..], ["bot", "inline", "general-thread"]);
    let bot = &conversation.discussions()[120];
    assert_eq!(bot.notes()[0].author.as_str(), "900");
    assert_eq!(bot.resolution(), Resolution::NotResolvable);
    let inline = &conversation.discussions()[121];
    assert_eq!(inline.resolution(), Resolution::Unresolved);
    let bodies: Vec<_> = inline
        .notes()
        .iter()
        .map(|note| note.body.as_str())
        .collect();
    assert_eq!(bodies, ["This anchor needs a test.", "Agreed."]);
    assert_eq!(
        inline.notes()[0].position.as_ref(),
        Some(&canonical_position(position.as_object().unwrap()).unwrap())
    );
    assert_eq!(inline.notes()[1].position, None);
    assert_eq!(
        conversation.discussions()[122].resolution(),
        Resolution::Resolved
    );

    let state = fixture.state.lock().unwrap();
    let pages: Vec<_> = state
        .requests
        .iter()
        .filter(|request| request.path == format!("{MR_PATH}/discussions"))
        .map(|request| request.query["page"].clone())
        .collect();
    assert_eq!(pages, ["1", "2"]);
    assert!(state.writes().is_empty());
    drop(state);

    // F23: the retained document keeps the provider object, including keys the
    // adapter does not interpret.
    let document = ConversationDocument::new(&conversation, "2026-10-13T12:00:00Z".into()).unwrap();
    let bytes = encode(&document).unwrap();
    let stored: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stored["discussions"][121]["notes"][0]["position"], position);
    let restored = decode::<ConversationDocument>(&bytes)
        .unwrap()
        .into_conversation()
        .unwrap();
    assert_eq!(restored, conversation);
}

// D09: owned summary and finding notes are ordinary conversation context;
// capture neither validates nor changes managed review state.
#[test]
fn published_review_notes_are_captured_without_provider_writes() {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let req = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    publish(
        &req,
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    )
    .unwrap();
    let mut state = fixture.state.lock().unwrap();
    state.question("discussion-1", "Is this still open?");
    let writes = state.writes().len();
    drop(state);

    let conversation = capture(&fixture).unwrap();
    assert_eq!(conversation.discussions().len(), 2);
    let summary = &conversation.discussions()[0];
    assert!(summary.notes()[0].body.contains("review-kit:v1"));
    assert_eq!(summary.notes()[0].author.as_str(), "42");
    let finding = &conversation.discussions()[1];
    assert_eq!(finding.id().as_str(), "discussion-1");
    assert_eq!(finding.resolution(), Resolution::Unresolved);
    assert!(finding.notes()[0].position.is_some());
    assert_eq!(finding.notes()[1].author.as_str(), "77");
    assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
}

// F14: additions, edits, deletions and resolution changes between captures are
// differences; provider ordering is not.
#[test]
fn fresh_captures_detect_conversation_changes_but_not_reordering() {
    let fixture = Fixture::new();
    seed(&fixture);
    let retained = capture(&fixture).unwrap();
    fixture.state.lock().unwrap().discussions.reverse();
    let reordered = capture(&fixture).unwrap();
    assert_ne!(reordered, retained);
    assert!(reordered.matches(&retained));

    let (edited, removed, added) = {
        let mut state = fixture.state.lock().unwrap();
        let inline = state
            .discussions
            .iter()
            .position(|discussion| discussion["id"] == "inline")
            .unwrap();
        state.discussions[inline]["notes"][0]["body"] = "Edited anchor note.".into();
        state.discussions[inline]["notes"][0]["position"]["new_line"] = 3.into();
        state.discussions[inline]["notes"][0]["resolved"] = true.into();
        let edited = state.discussions[inline]["notes"][0]["id"].to_string();
        let removed = state.discussions.remove(0);
        let note = state.note("A later question.", 79);
        let added = note["id"].to_string();
        state.discussions.push(thread("later", true, vec![note]));
        state.title = Some("Renamed".into());
        (edited, removed, added)
    };
    let removed_id = removed["id"].as_str().unwrap();
    let removed_note = removed["notes"][0]["id"].to_string();
    assert_eq!(
        capture(&fixture).unwrap().changes_since(&retained),
        [
            ConversationChange::Title,
            ConversationChange::DiscussionAdded(id("later")),
            ConversationChange::DiscussionRemoved(id(removed_id)),
            ConversationChange::ResolutionChanged {
                discussion: id("inline"),
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteAdded {
                discussion: id("later"),
                note: id(&added),
            },
            ConversationChange::NoteRemoved {
                discussion: id(removed_id),
                note: id(&removed_note),
            },
            ConversationChange::NoteBodyChanged(id(&edited)),
            ConversationChange::NotePositionChanged(id(&edited)),
        ]
    );
}

#[test]
fn resolution_and_description_follow_provider_flags() {
    let note = |resolvable: Option<bool>, resolved: Option<bool>| {
        let mut note = json!({"id":0,"body":"text","author":{"id":77},"system":false,
            "noteable_id":55,"noteable_type":"MergeRequest","project_id":11,"noteable_iid":7,
            "type":"DiscussionNote","position":null});
        if let Some(resolvable) = resolvable {
            note["resolvable"] = resolvable.into();
        }
        if let Some(resolved) = resolved {
            note["resolved"] = resolved.into();
        }
        note
    };
    let cases = [
        (
            vec![note(Some(true), Some(true)), note(Some(false), None)],
            Ok(Resolution::Resolved),
        ),
        (
            vec![note(Some(true), Some(true)), note(Some(true), Some(false))],
            Ok(Resolution::Unresolved),
        ),
        (
            vec![note(Some(false), None), note(Some(false), Some(false))],
            Ok(Resolution::NotResolvable),
        ),
        (vec![note(Some(true), None)], Err(GitLabError::Protocol)),
        (vec![note(None, None)], Err(GitLabError::Protocol)),
    ];
    for (notes, expected) in cases {
        let fixture = Fixture::new();
        {
            let mut state = fixture.state.lock().unwrap();
            state.description = Some(Value::Null);
            let notes = notes
                .into_iter()
                .enumerate()
                .map(|(index, mut note)| {
                    note["id"] = (500 + index).into();
                    note
                })
                .collect();
            state.discussions.push(thread("thread", false, notes));
        }
        let observed = capture(&fixture).map(|conversation| {
            assert_eq!(conversation.description(), "");
            conversation.discussions()[0].resolution()
        });
        assert_eq!(observed, expected);
    }
}

// F17: an incomplete or unreadable discussion collection never yields a
// conversation.
#[test]
fn incomplete_pagination_limits_and_protocol_errors_fail_capture() {
    let discussions = format!("{MR_PATH}/discussions");
    type Setup = fn(&Fixture);
    let broken: [(&str, Setup, GitLabError); 11] = [
        (
            "next page skipped",
            |f| f.state.lock().unwrap().broken_pagination = true,
            GitLabError::Protocol,
        ),
        (
            "server error",
            |f| {
                f.state.lock().unwrap().faults.push(Fault::response(
                    "GET",
                    &format!("{MR_PATH}/discussions"),
                    Response::status(503),
                ))
            },
            GitLabError::Rejected(503),
        ),
        (
            "later page fails",
            |f| {
                let mut state = f.state.lock().unwrap();
                state.page_size = 1;
                for index in 0..3 {
                    let note = state.note("ordinary", 77);
                    state
                        .discussions
                        .push(thread(&format!("t{index}"), true, vec![note]));
                }
                state.faults.push(Fault::response_after(
                    "GET",
                    &format!("{MR_PATH}/discussions"),
                    1,
                    Response::status(500),
                ));
            },
            GitLabError::Rejected(500),
        ),
        (
            "malformed page",
            |f| {
                f.state.lock().unwrap().faults.push(Fault::response(
                    "GET",
                    &format!("{MR_PATH}/discussions"),
                    Response::json(json!([{"id":"x","individual_note":true}])),
                ))
            },
            GitLabError::Protocol,
        ),
        (
            "empty discussion",
            |f| {
                f.state
                    .lock()
                    .unwrap()
                    .discussions
                    .push(thread("empty", false, vec![]))
            },
            GitLabError::Protocol,
        ),
        (
            "position is not an object",
            |f| {
                let mut state = f.state.lock().unwrap();
                let mut note = state.note("anchored", 77);
                note["position"] = "src/lib.rs:2".into();
                state.discussions.push(thread("bad", true, vec![note]));
            },
            GitLabError::Protocol,
        ),
        (
            "duplicate discussion",
            |f| {
                let mut state = f.state.lock().unwrap();
                let first = state.note("first", 77);
                let second = state.note("second", 77);
                state.discussions.push(thread("same", true, vec![first]));
                state.discussions.push(thread("same", true, vec![second]));
            },
            GitLabError::Protocol,
        ),
        (
            "note in two discussions",
            |f| {
                let mut state = f.state.lock().unwrap();
                let note = state.note("shared", 77);
                state
                    .discussions
                    .push(thread("one", true, vec![note.clone()]));
                state.discussions.push(thread("two", true, vec![note]));
            },
            GitLabError::Protocol,
        ),
        (
            "missing title",
            |f| f.state.lock().unwrap().title = None,
            GitLabError::Protocol,
        ),
        (
            "missing description",
            |f| f.state.lock().unwrap().description = None,
            GitLabError::Protocol,
        ),
        (
            "null title",
            |f| f.state.lock().unwrap().title = Some(Value::Null),
            GitLabError::Protocol,
        ),
    ];
    for (name, setup, expected) in broken {
        let fixture = Fixture::new();
        let client = fixture.connect();
        setup(&fixture);
        assert_eq!(
            client.capture_conversation(client.subject()).unwrap_err(),
            expected,
            "{name}"
        );
    }
    let fixture = Fixture::new();
    {
        let mut state = fixture.state.lock().unwrap();
        state.page_size = 1;
        for index in 0..2 {
            let note = state.note("ordinary", 77);
            state
                .discussions
                .push(thread(&format!("t{index}"), true, vec![note]));
        }
    }
    let client = fixture
        .connect_with_limits(Limits {
            pages: 1,
            ..Limits::default()
        })
        .unwrap();
    assert_eq!(
        client.capture_conversation(client.subject()).unwrap_err(),
        GitLabError::Limit
    );
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|request| request.path != discussions || request.query["page"] == "1")
    );
}

#[test]
fn notes_outside_the_bound_merge_request_and_other_subjects_are_rejected() {
    let foreign: [(&str, &str, Value); 6] = [
        ("project", "project_id", 12.into()),
        ("merge request", "noteable_id", 56.into()),
        ("noteable type", "noteable_type", "Issue".into()),
        ("noteable IID", "noteable_iid", 8.into()),
        ("author", "author", json!({"id":0})),
        ("note ID", "id", 0.into()),
    ];
    for (name, field, value) in foreign {
        let fixture = Fixture::new();
        {
            let mut state = fixture.state.lock().unwrap();
            let ordinary = state.note("ordinary", 77);
            let mut note = state.note("system note elsewhere", 1);
            note["system"] = true.into();
            note[field] = value;
            state
                .discussions
                .push(thread("mixed", false, vec![ordinary, note]));
        }
        assert_eq!(
            capture(&fixture).unwrap_err(),
            GitLabError::Identity,
            "{name}"
        );
    }

    let fixture = Fixture::new();
    let client = fixture.connect();
    let other = Subject::new(
        "gitlab".into(),
        client.subject().host().into(),
        "group/sub/other".into(),
        7,
    )
    .unwrap();
    let before = fixture.state.lock().unwrap().requests.len();
    assert_eq!(
        client.capture_conversation(&other).unwrap_err(),
        GitLabError::Identity
    );
    assert_eq!(fixture.state.lock().unwrap().requests.len(), before);

    fixture.state.lock().unwrap().source_project = Some(23);
    assert_eq!(
        client.capture_conversation(client.subject()).unwrap_err(),
        GitLabError::Identity
    );
}
