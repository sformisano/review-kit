use std::error::Error;

use review_kit_domain::Subject;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn id(value: &str) -> Result<ProviderId, Box<dyn Error>> {
    Ok(ProviderId::new(value.into())?)
}

fn note(
    note: &str,
    author: &str,
    body: &str,
    position: Option<&str>,
) -> Result<Note, Box<dyn Error>> {
    Ok(Note {
        id: id(note)?,
        author: ReviewerId::try_from(author.to_owned())?,
        body: body.into(),
        position: position
            .map(|text| Position::from_canonical_json(text.into()))
            .transpose()?,
    })
}

fn subject(request: u64) -> Result<Subject, Box<dyn Error>> {
    Ok(Subject::new(
        "gitlab".into(),
        "https://gitlab.example".into(),
        "team/project".into(),
        request,
    )?)
}

const ANCHOR: &str = r#"{"new_line":3,"new_path":"src/lib.rs","position_type":"text"}"#;

fn conversation() -> Result<Conversation, Box<dyn Error>> {
    Ok(Conversation::new(
        subject(1)?,
        "Accept JSON input".into(),
        "Keep the positive quantity contract.".into(),
        vec![
            Discussion::new(
                id("general")?,
                Resolution::NotResolvable,
                vec![note("10", "7", "Please check zero input.", None)?],
            )?,
            Discussion::new(
                id("inline")?,
                Resolution::Unresolved,
                vec![
                    note("21", "9", "This line accepts zero.", Some(ANCHOR))?,
                    note("22", "7", "Agreed.", None)?,
                ],
            )?,
        ],
    )?)
}

fn rebuild(
    original: &Conversation,
    edit: impl FnOnce(&mut Vec<(ProviderId, Resolution, Vec<Note>)>) -> TestResult,
) -> Result<Conversation, Box<dyn Error>> {
    let mut parts = original
        .discussions()
        .iter()
        .map(|discussion| {
            (
                discussion.id().clone(),
                discussion.resolution(),
                discussion.notes().to_vec(),
            )
        })
        .collect();
    edit(&mut parts)?;
    let discussions = parts
        .into_iter()
        .map(|(id, resolution, notes)| Discussion::new(id, resolution, notes))
        .collect::<Result<_, _>>()?;
    Ok(Conversation::new(
        original.subject().clone(),
        original.title().into(),
        original.description().into(),
        discussions,
    )?)
}

#[test]
fn ordering_of_discussions_and_notes_is_not_a_change() -> TestResult {
    let retained = conversation()?;
    let reordered = rebuild(&retained, |parts| {
        parts.reverse();
        for (_, _, notes) in parts.iter_mut() {
            notes.reverse();
        }
        Ok(())
    })?;
    assert_ne!(reordered, retained);
    assert_eq!(reordered.changes_since(&retained), []);
    assert!(reordered.matches(&retained));
    assert_eq!(reordered.normalized(), retained.normalized());
    let ids: Vec<_> = reordered
        .normalized()
        .discussions()
        .iter()
        .map(|discussion| discussion.id().as_str().to_owned())
        .collect();
    assert_eq!(ids, ["general", "inline"]);
    Ok(())
}

#[test]
fn additions_deletions_and_edits_are_reported_by_identifier() -> TestResult {
    let retained = conversation()?;
    let current = rebuild(&retained, |parts| {
        parts[0].2.push(note("11", "7", "A new question.", None)?);
        parts[1].2.retain(|note| note.id.as_str() != "22");
        parts[1].2[0].body = "This line accepts zero values.".into();
        Ok(())
    })?;
    assert_eq!(
        current.changes_since(&retained),
        [
            ConversationChange::NoteAdded {
                discussion: id("general")?,
                note: id("11")?,
            },
            ConversationChange::NoteRemoved {
                discussion: id("inline")?,
                note: id("22")?,
            },
            ConversationChange::NoteBodyChanged(id("21")?),
        ]
    );
    assert_ne!(current.normalized(), retained.normalized());
    Ok(())
}

#[test]
fn position_author_resolution_and_membership_participate_in_freshness() -> TestResult {
    let retained = conversation()?;
    let moved_anchor = r#"{"new_line":4,"new_path":"src/lib.rs","position_type":"text"}"#;
    let current = rebuild(&retained, |parts| {
        parts[1].1 = Resolution::Resolved;
        parts[1].2[0].position = Some(Position::from_canonical_json(moved_anchor.into())?);
        parts[1].2[1].author = ReviewerId::try_from("8".to_owned())?;
        let moved = parts[1].2.remove(1);
        parts.push((id("split")?, Resolution::NotResolvable, vec![moved]));
        Ok(())
    })?;
    assert_eq!(
        current.changes_since(&retained),
        [
            ConversationChange::DiscussionAdded(id("split")?),
            ConversationChange::ResolutionChanged {
                discussion: id("inline")?,
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteMoved {
                note: id("22")?,
                retained: id("inline")?,
                current: id("split")?,
            },
            ConversationChange::NoteAuthorChanged(id("22")?),
            ConversationChange::NotePositionChanged(id("21")?),
        ]
    );
    Ok(())
}

#[test]
fn removed_discussion_title_description_and_subject_are_reported() -> TestResult {
    let retained = conversation()?;
    let current = Conversation::new(
        subject(2)?,
        "Accept JSON and YAML input".into(),
        String::new(),
        vec![retained.discussions()[1].clone()],
    )?;
    assert_eq!(
        current.changes_since(&retained),
        [
            ConversationChange::Subject,
            ConversationChange::Title,
            ConversationChange::Description,
            ConversationChange::DiscussionRemoved(id("general")?),
            ConversationChange::NoteRemoved {
                discussion: id("general")?,
                note: id("10")?,
            },
        ]
    );
    Ok(())
}

#[test]
fn construction_rejects_ambiguous_identifiers_and_empty_discussions() -> TestResult {
    let original = conversation()?;
    let duplicate = original.discussions()[0].clone();
    assert_eq!(
        Conversation::new(
            subject(1)?,
            String::new(),
            String::new(),
            vec![duplicate.clone(), duplicate],
        ),
        Err(InvalidConversation::DuplicateDiscussion(id("general")?))
    );
    let shared_note = note("10", "7", "Copied.", None)?;
    assert_eq!(
        Conversation::new(
            subject(1)?,
            String::new(),
            String::new(),
            vec![
                original.discussions()[0].clone(),
                Discussion::new(
                    id("other")?,
                    Resolution::NotResolvable,
                    vec![shared_note.clone()]
                )?,
            ],
        ),
        Err(InvalidConversation::DuplicateNote(id("10")?))
    );
    assert_eq!(
        Discussion::new(
            id("twice")?,
            Resolution::NotResolvable,
            vec![shared_note.clone(), shared_note],
        ),
        Err(InvalidConversation::DuplicateNote(id("10")?))
    );
    assert_eq!(
        Discussion::new(id("empty")?, Resolution::NotResolvable, vec![]),
        Err(InvalidConversation::EmptyDiscussion(id("empty")?))
    );
    Ok(())
}

#[test]
fn resolution_flags_admit_only_consistent_pairs() {
    assert_eq!(
        Resolution::from_flags(false, None),
        Ok(Resolution::NotResolvable)
    );
    assert_eq!(
        Resolution::from_flags(true, Some(false)),
        Ok(Resolution::Unresolved)
    );
    assert_eq!(
        Resolution::from_flags(true, Some(true)),
        Ok(Resolution::Resolved)
    );
    for (resolvable, resolved) in [(false, Some(false)), (false, Some(true)), (true, None)] {
        assert_eq!(
            Resolution::from_flags(resolvable, resolved),
            Err(InvalidConversation::Resolution)
        );
    }
    for resolution in [
        Resolution::NotResolvable,
        Resolution::Unresolved,
        Resolution::Resolved,
    ] {
        assert_eq!(
            Resolution::from_flags(resolution.resolvable(), resolution.resolved()),
            Ok(resolution)
        );
    }
}

#[test]
fn positions_require_object_text() {
    assert!(Position::from_canonical_json("{}".into()).is_ok());
    for text in ["", "null", "[]", "\"{}\"", "{\"a\":1"] {
        assert_eq!(
            Position::from_canonical_json(text.into()),
            Err(InvalidConversation::Position)
        );
    }
}
