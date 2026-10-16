#![allow(clippy::unwrap_used, clippy::expect_used)]

use review_kit_application::{
    conversation::{Conversation, ConversationChange, Discussion, Note, Position, Resolution},
    publication::ProviderId,
};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_json_adapter::{ConversationDocument, canonical_position, decode, encode};
use serde_json::{Map, Value, json};

// Packet examples 01, 02, 04 and 06 verbatim. Examples 03, 05, 07 and 08 are
// byte-identical to 02.
const INITIAL_REVIEW: &str = r#"{
  "version": 1,
  "kind": "review_conversation",
  "subject": {
    "provider": "gitlab",
    "host": "https://gitlab.example.test",
    "repository": "team/project",
    "request": 1
  },
  "observed_at": "2026-10-13T12:00:00Z",
  "title": "Accept quantities from JSON input",
  "description": "Allow JSON input while retaining the positive quantity contract.",
  "discussions": [
    {
      "id": "fixture-general-thread",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-general-note",
          "author": "10",
          "body": "Please check whether JSON input preserves the positive quantity contract.",
          "position": null
        }
      ]
    }
  ]
}"#;

const FOLLOW_UP: &str = r#"{
  "version": 1,
  "kind": "review_conversation",
  "subject": {
    "provider": "gitlab",
    "host": "https://gitlab.example.test",
    "repository": "team/project",
    "request": 1
  },
  "observed_at": "2026-10-13T12:00:00Z",
  "title": "Accept quantities from JSON input",
  "description": "Allow JSON input while retaining the positive quantity contract.",
  "discussions": [
    {
      "id": "fixture-general-thread",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-general-note",
          "author": "10",
          "body": "Please check whether JSON input preserves the positive quantity contract.",
          "position": null
        }
      ]
    },
    {
      "id": "discussion-fixture-summary",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-summary",
          "author": "9",
          "body": "Illustrative managed review summary.",
          "position": null
        }
      ]
    },
    {
      "id": "fixture-thread-1",
      "resolvable": true,
      "resolved": false,
      "notes": [
        {
          "id": "fixture-root-note-1",
          "author": "9",
          "body": "JSON input can construct a zero quantity even though Quantity::new rejects zero. Route deserialization through the constructor to preserve the documented positive-value guarantee.",
          "position": {
            "position_type": "text",
            "old_path": "src/quantity.rs",
            "new_path": "src/quantity.rs",
            "new_line": 3,
            "head_sha": "1111111111111111111111111111111111111111",
            "base_sha": "2222222222222222222222222222222222222222",
            "start_sha": "3333333333333333333333333333333333333333"
          }
        }
      ]
    }
  ]
}"#;

const FOLLOW_UP_REPLY: &str = r#"{
  "version": 1,
  "kind": "review_conversation",
  "subject": {
    "provider": "gitlab",
    "host": "https://gitlab.example.test",
    "repository": "team/project",
    "request": 1
  },
  "observed_at": "2026-10-13T12:00:00Z",
  "title": "Accept quantities from JSON input",
  "description": "Allow JSON input while retaining the positive quantity contract.",
  "discussions": [
    {
      "id": "fixture-general-thread",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-general-note",
          "author": "10",
          "body": "Please check whether JSON input preserves the positive quantity contract.",
          "position": null
        }
      ]
    },
    {
      "id": "fixture-thread-1",
      "resolvable": true,
      "resolved": false,
      "notes": [
        {
          "id": "fixture-root-note-1",
          "author": "9",
          "body": "JSON input can construct a zero quantity even though Quantity::new rejects zero. Route deserialization through the constructor to preserve the documented positive-value guarantee.",
          "position": {
            "position_type": "text",
            "old_path": "src/quantity.rs",
            "new_path": "src/quantity.rs",
            "new_line": 3,
            "head_sha": "1111111111111111111111111111111111111111",
            "base_sha": "2222222222222222222222222222222222222222",
            "start_sha": "3333333333333333333333333333333333333333"
          }
        },
        {
          "id": "fixture-fixed-reply",
          "author": "9",
          "body": "The Deserialize try_from conversion calls Quantity::new, which rejects zero before construction.",
          "position": null
        }
      ]
    },
    {
      "id": "discussion-fixture-summary",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-summary",
          "author": "9",
          "body": "Illustrative managed review summary.",
          "position": null
        }
      ]
    }
  ]
}"#;

const FOLLOW_UP_QUESTION: &str = r#"{
  "version": 1,
  "kind": "review_conversation",
  "subject": {
    "provider": "gitlab",
    "host": "https://gitlab.example.test",
    "repository": "team/project",
    "request": 1
  },
  "observed_at": "2026-10-13T12:00:00Z",
  "title": "Accept quantities from JSON input",
  "description": "Allow JSON input while retaining the positive quantity contract.",
  "discussions": [
    {
      "id": "fixture-general-thread",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-general-note",
          "author": "10",
          "body": "Please check whether JSON input preserves the positive quantity contract.",
          "position": null
        }
      ]
    },
    {
      "id": "fixture-thread-1",
      "resolvable": true,
      "resolved": false,
      "notes": [
        {
          "id": "fixture-root-note-1",
          "author": "9",
          "body": "JSON input can construct a zero quantity even though Quantity::new rejects zero. Route deserialization through the constructor to preserve the documented positive-value guarantee.",
          "position": {
            "position_type": "text",
            "old_path": "src/quantity.rs",
            "new_path": "src/quantity.rs",
            "new_line": 3,
            "head_sha": "1111111111111111111111111111111111111111",
            "base_sha": "2222222222222222222222222222222222222222",
            "start_sha": "3333333333333333333333333333333333333333"
          }
        },
        {
          "id": "fixture-question",
          "author": "10",
          "body": "Does the private field prevent a zero value from JSON?",
          "position": null
        }
      ]
    },
    {
      "id": "discussion-fixture-summary",
      "resolvable": false,
      "resolved": null,
      "notes": [
        {
          "id": "fixture-summary",
          "author": "9",
          "body": "Illustrative managed review summary.",
          "position": null
        }
      ]
    }
  ]
}"#;

fn schema() -> jsonschema::Validator {
    let text = include_str!("../schemas/review-conversation-v1.schema.json");
    jsonschema::options()
        .should_validate_formats(true)
        .build(&serde_json::from_str(text).unwrap())
        .unwrap()
}

fn restore(value: &Value) -> Result<Conversation, review_kit_json_adapter::JsonError> {
    decode::<ConversationDocument>(&serde_json::to_vec(value).unwrap())?.into_conversation()
}

fn written(conversation: &Conversation, observed_at: &str) -> Value {
    let document = ConversationDocument::new(conversation, observed_at.into()).unwrap();
    serde_json::from_slice(&encode(&document).unwrap()).unwrap()
}

fn id(value: &str) -> ProviderId {
    ProviderId::new(value.into()).unwrap()
}

#[test]
fn packet_examples_satisfy_the_schema_and_round_trip_exactly() {
    let schema = schema();
    for text in [
        INITIAL_REVIEW,
        FOLLOW_UP,
        FOLLOW_UP_REPLY,
        FOLLOW_UP_QUESTION,
    ] {
        let original: Value = serde_json::from_str(text).unwrap();
        assert!(schema.is_valid(&original), "{original}");
        let conversation = restore(&original).unwrap();
        let rewritten = written(&conversation, original["observed_at"].as_str().unwrap());
        assert_eq!(rewritten, original);
        assert!(schema.is_valid(&rewritten));
    }
}

// F23: an inline note keeps its complete provider position, including keys this
// crate does not interpret, and an anchor change is a freshness difference.
#[test]
fn positions_round_trip_verbatim_and_anchor_changes_are_detected() {
    let mut original: Value = serde_json::from_str(FOLLOW_UP).unwrap();
    let position = json!({
        "position_type": "text",
        "new_path": "src/quantity.rs",
        "old_path": "src/quantity.rs",
        "new_line": 3,
        "old_line": null,
        "head_sha": "1111111111111111111111111111111111111111",
        "base_sha": "2222222222222222222222222222222222222222",
        "start_sha": "3333333333333333333333333333333333333333",
        "line_range": {
            "start": {"line_code": "abc_2_2", "type": "new", "new_line": 2, "old_line": null},
            "end": {"type": "new", "line_code": "abc_3_3", "old_line": null, "new_line": 3}
        },
        "ignore_whitespace_change": false,
        "x_future_anchor": ["köln", 1.5, {"z": true, "a": "\u{0007}\"quoted\""}]
    });
    original["discussions"][2]["notes"][0]["position"] = position.clone();
    assert!(schema().is_valid(&original));
    let conversation = restore(&original).unwrap();
    assert_eq!(written(&conversation, "2026-10-13T12:00:00Z"), original);
    let stored = conversation.discussions()[2].notes()[0]
        .position
        .as_ref()
        .unwrap();
    assert_eq!(
        stored.as_str(),
        concat!(
            r#"{"base_sha":"2222222222222222222222222222222222222222","#,
            r#""head_sha":"1111111111111111111111111111111111111111","#,
            r#""ignore_whitespace_change":false,"#,
            r#""line_range":{"end":{"line_code":"abc_3_3","new_line":3,"old_line":null,"type":"new"},"#,
            r#""start":{"line_code":"abc_2_2","new_line":2,"old_line":null,"type":"new"}},"#,
            r#""new_line":3,"new_path":"src/quantity.rs","old_line":null,"old_path":"src/quantity.rs","#,
            r#""position_type":"text","start_sha":"3333333333333333333333333333333333333333","#,
            r#""x_future_anchor":["köln",1.5,{"a":"\u0007\"quoted\"","z":true}]}"#,
        )
    );
    let mut moved = original.clone();
    moved["discussions"][2]["notes"][0]["position"]["line_range"]["start"]["new_line"] = 1.into();
    assert_eq!(
        restore(&moved).unwrap().changes_since(&conversation),
        [ConversationChange::NotePositionChanged(id(
            "fixture-root-note-1"
        ))]
    );
}

#[test]
fn canonical_positions_ignore_key_order_and_spacing_only() {
    let parse = |text: &str| -> Map<String, Value> { serde_json::from_str(text).unwrap() };
    let first = canonical_position(&parse(
        r#"{"b": {"y": 1, "x": [2, {"q": 0, "p": null}]}, "a": "s"}"#,
    ))
    .unwrap();
    let second =
        canonical_position(&parse(r#"{"a":"s","b":{"x":[2,{"p":null,"q":0}],"y":1}}"#)).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first.as_str(),
        r#"{"a":"s","b":{"x":[2,{"p":null,"q":0}],"y":1}}"#
    );
    for different in [
        r#"{"a":"s","b":{"x":[{"p":null,"q":0},2],"y":1}}"#,
        r#"{"a":"s","b":{"x":[2,{"p":null,"q":0}],"y":"1"}}"#,
        r#"{"a":"s","b":{"x":[2,{"q":0}],"y":1}}"#,
    ] {
        assert_ne!(canonical_position(&parse(different)).unwrap(), first);
    }
}

// F14: a note added or edited before dispatch changes the normalized
// conversation; observation time and discussion order do not.
#[test]
fn freshness_ignores_order_and_observed_time_but_detects_note_changes() {
    let follow_up: Value = serde_json::from_str(FOLLOW_UP).unwrap();
    let retained = restore(&follow_up).unwrap();
    let mut reordered = follow_up.clone();
    reordered["observed_at"] = "2026-10-14T08:30:00.125Z".into();
    reordered["discussions"].as_array_mut().unwrap().reverse();
    let reordered = restore(&reordered).unwrap();
    assert_ne!(reordered, retained);
    assert!(reordered.matches(&retained));

    let reply = restore(&serde_json::from_str(FOLLOW_UP_REPLY).unwrap()).unwrap();
    assert_eq!(
        reply.changes_since(&retained),
        [ConversationChange::NoteAdded {
            discussion: id("fixture-thread-1"),
            note: id("fixture-fixed-reply"),
        }]
    );
    let question = restore(&serde_json::from_str(FOLLOW_UP_QUESTION).unwrap()).unwrap();
    assert_eq!(
        retained.changes_since(&question),
        [ConversationChange::NoteRemoved {
            discussion: id("fixture-thread-1"),
            note: id("fixture-question"),
        }]
    );

    let mut edited = follow_up.clone();
    edited["description"] = "Allow JSON input.".into();
    edited["discussions"][0]["notes"][0]["body"] = "Edited question.".into();
    edited["discussions"][2]["resolved"] = true.into();
    assert_eq!(
        restore(&edited).unwrap().changes_since(&retained),
        [
            ConversationChange::Description,
            ConversationChange::ResolutionChanged {
                discussion: id("fixture-thread-1"),
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteBodyChanged(id("fixture-general-note")),
        ]
    );
}

#[test]
fn strict_decoding_rejects_unknown_fields_and_inconsistent_state() {
    let schema = schema();
    let original: Value = serde_json::from_str(FOLLOW_UP).unwrap();
    type Edit = fn(&mut Value);
    // The final flag records whether the schema can express the rule.
    let cases: [(&str, Edit, bool); 20] = [
        ("unknown top-level field", |v| v["extra"] = 1.into(), true),
        (
            "unknown discussion field",
            |v| v["discussions"][0]["extra"] = 1.into(),
            true,
        ),
        (
            "unknown note field",
            |v| v["discussions"][0]["notes"][0]["extra"] = 1.into(),
            true,
        ),
        (
            "missing resolved",
            |v| {
                v["discussions"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("resolved");
            },
            true,
        ),
        (
            "missing position",
            |v| {
                v["discussions"][0]["notes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("position");
            },
            true,
        ),
        (
            "missing observed time",
            |v| {
                v.as_object_mut().unwrap().remove("observed_at");
            },
            true,
        ),
        (
            "position string",
            |v| v["discussions"][0]["notes"][0]["position"] = "src/lib.rs:3".into(),
            true,
        ),
        (
            "position array",
            |v| v["discussions"][0]["notes"][0]["position"] = json!([]),
            true,
        ),
        (
            "resolved without resolvable",
            |v| v["discussions"][0]["resolved"] = false.into(),
            true,
        ),
        (
            "resolvable without resolved",
            |v| v["discussions"][2]["resolved"] = Value::Null,
            true,
        ),
        (
            "empty notes",
            |v| v["discussions"][0]["notes"] = json!([]),
            true,
        ),
        (
            "blank note ID",
            |v| v["discussions"][0]["notes"][0]["id"] = " ".into(),
            true,
        ),
        (
            "blank author",
            |v| v["discussions"][0]["notes"][0]["author"] = "".into(),
            true,
        ),
        ("version 2", |v| v["version"] = 2.into(), true),
        ("other kind", |v| v["kind"] = "context".into(), true),
        (
            "local time",
            |v| v["observed_at"] = "2026-10-13T12:00:00+02:00".into(),
            true,
        ),
        (
            "invalid month",
            |v| v["observed_at"] = "2026-13-13T12:00:00Z".into(),
            true,
        ),
        ("request zero", |v| v["subject"]["request"] = 0.into(), true),
        (
            "duplicate discussion",
            |v| {
                let copy = v["discussions"][0].clone();
                v["discussions"].as_array_mut().unwrap().push(copy);
            },
            false,
        ),
        (
            "note in two discussions",
            |v| {
                let copy = v["discussions"][0]["notes"][0].clone();
                v["discussions"][1]["notes"]
                    .as_array_mut()
                    .unwrap()
                    .push(copy);
            },
            false,
        ),
    ];
    for (name, edit, schema_rule) in cases {
        let mut value = original.clone();
        edit(&mut value);
        assert!(restore(&value).is_err(), "{name}");
        assert_eq!(!schema.is_valid(&value), schema_rule, "{name}");
    }
}

#[test]
fn documents_written_from_the_application_model_satisfy_the_schema() {
    let subject = Subject::new(
        "gitlab".into(),
        "http://127.0.0.1:8931".into(),
        "group/project".into(),
        4,
    )
    .unwrap();
    let note = |note: &str, author: &str, position: Option<&str>| Note {
        id: id(note),
        author: ReviewerId::try_from(author.to_owned()).unwrap(),
        body: format!("Body of {note}"),
        position: position.map(|text| Position::from_canonical_json(text.into()).unwrap()),
    };
    let conversation = Conversation::new(
        subject,
        "Title".into(),
        String::new(),
        vec![
            Discussion::new(
                id("a1"),
                Resolution::NotResolvable,
                vec![note("1", "5", None)],
            )
            .unwrap(),
            Discussion::new(
                id("b2"),
                Resolution::Resolved,
                vec![
                    note("2", "6", Some(r#"{"new_line":1,"new_path":"a.rs"}"#)),
                    note("3", "7", None),
                ],
            )
            .unwrap(),
        ],
    )
    .unwrap();
    for observed_at in ["2026-10-13T12:00:00Z", "2026-10-13T12:00:00.123456Z"] {
        let value = written(&conversation, observed_at);
        assert!(schema().is_valid(&value), "{value}");
        assert_eq!(value["discussions"][0]["resolved"], Value::Null);
        assert_eq!(value["discussions"][1]["resolved"], true);
        assert_eq!(
            value["discussions"][1]["notes"][0]["position"],
            json!({"new_path":"a.rs","new_line":1})
        );
        assert_eq!(restore(&value).unwrap(), conversation);
    }
    for observed_at in [
        "",
        "2026-10-13",
        "2026-10-13T12:00:00",
        "2026-10-13T24:00:00Z",
        "2026-10-13T12:00:00.Z",
        "２026-10-13T12:00:00Z",
    ] {
        assert!(
            ConversationDocument::new(&conversation, observed_at.into()).is_err(),
            "{observed_at}"
        );
    }
}
