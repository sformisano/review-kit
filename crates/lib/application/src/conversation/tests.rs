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
        account: Some(AuthorAccount {
            username: Some(format!("user-{author}")),
            bot: BotStatus::NotBot,
        }),
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

/// The reviewer's summary note `100` alone in `summary`, its finding thread
/// `finding` with an author question, a foreign diff thread and a bot note,
/// with a publication that updates the summary and writes in `finding`.
fn reviewed() -> Result<(Conversation, WriteTargets), Box<dyn Error>> {
    let conversation = Conversation::new(
        subject(1)?,
        "Accept JSON input".into(),
        "Keep the positive quantity contract.".into(),
        vec![
            Discussion::new(
                id("summary")?,
                Resolution::NotResolvable,
                vec![note("100", "42", "Review summary.", None)?],
            )?,
            Discussion::new(
                id("finding")?,
                Resolution::Unresolved,
                vec![
                    note("200", "42", "Zero passes validation.", Some(ANCHOR))?,
                    note("201", "5", "Does the regression cover it?", None)?,
                ],
            )?,
            Discussion::new(
                id("foreign")?,
                Resolution::Unresolved,
                vec![note("300", "77", "Comment this line.", Some(ANCHOR))?],
            )?,
            Discussion::new(
                id("bot")?,
                Resolution::NotResolvable,
                vec![note("400", "91", "Automated report.", None)?],
            )?,
        ],
    )?;
    let targets = WriteTargets {
        summary_notes: BTreeSet::from([id("100")?]),
        threads: BTreeSet::from([id("finding")?]),
    };
    Ok((conversation, targets))
}

fn with_text(
    original: &Conversation,
    subject: Subject,
    title: &str,
    description: &str,
) -> Result<Conversation, Box<dyn Error>> {
    Ok(Conversation::new(
        subject,
        title.into(),
        description.into(),
        original.discussions().to_vec(),
    )?)
}

#[test]
fn foreign_activity_and_request_text_are_outside_the_targets() -> TestResult {
    let (retained, targets) = reviewed()?;
    let edited = rebuild(&retained, |parts| {
        let foreign = &mut parts[2];
        foreign.1 = Resolution::Resolved;
        foreign.2[0].body = "Comment this added line.".into();
        foreign
            .2
            .push(note("301", "91", "Suggested wording.", None)?);
        parts.remove(3);
        parts.push((
            id("copied")?,
            Resolution::Unresolved,
            vec![note(
                "500",
                "77",
                "Copied.\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":1} -->",
                Some(ANCHOR),
            )?],
        ));
        parts.push((
            id("duo")?,
            Resolution::NotResolvable,
            vec![note("600", "91", "Automated review.", None)?],
        ));
        Ok(())
    })?;
    let current = with_text(
        &edited,
        subject(1)?,
        "Accept JSON and YAML input",
        "A new description.",
    )?;
    assert_eq!(current.changes_since(&retained).len(), 11);
    assert_eq!(current.changes_to(&retained, &targets), []);
    Ok(())
}

#[test]
fn every_change_inside_a_target_thread_and_to_the_summary_note_is_reported() -> TestResult {
    let (retained, targets) = reviewed()?;
    let moved_anchor = r#"{"new_line":4,"new_path":"src/lib.rs","position_type":"text"}"#;
    let current = rebuild(&retained, |parts| {
        parts[0].1 = Resolution::Unresolved;
        parts[0].2[0].body = "Review summary, edited by a maintainer.".into();
        parts[0]
            .2
            .push(note("101", "77", "A reply to the summary.", None)?);
        let finding = &mut parts[1];
        finding.1 = Resolution::Resolved;
        finding.2[0].position = Some(Position::from_canonical_json(moved_anchor.into())?);
        finding.2[1].body = "Does the regression cover zero?".into();
        finding.2[1].author = ReviewerId::try_from("6".to_owned())?;
        finding.2.push(note("202", "5", "One more case.", None)?);
        Ok(())
    })?;
    // The reply to the summary and its discussion's resolution are not changes
    // to the summary note itself.
    assert_eq!(
        current.changes_to(&retained, &targets),
        [
            ConversationChange::ResolutionChanged {
                discussion: id("finding")?,
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteAdded {
                discussion: id("finding")?,
                note: id("202")?,
            },
            ConversationChange::NoteAuthorChanged(id("201")?),
            ConversationChange::NoteBodyChanged(id("100")?),
            ConversationChange::NoteBodyChanged(id("201")?),
            ConversationChange::NotePositionChanged(id("200")?),
        ]
    );
    Ok(())
}

#[test]
fn removed_targets_and_notes_moved_across_the_boundary_stay_in_scope() -> TestResult {
    let (retained, targets) = reviewed()?;
    let current = rebuild(&retained, |parts| {
        let summary = parts.remove(0);
        let question = parts[0].2.remove(1);
        parts[1].2.push(question);
        let foreign_root = parts[1].2.remove(0);
        parts[0].2.push(foreign_root);
        parts.push((id("summary-copy")?, Resolution::NotResolvable, summary.2));
        Ok(())
    })?;
    assert_eq!(
        current.changes_to(&retained, &targets),
        [
            ConversationChange::NoteMoved {
                note: id("100")?,
                retained: id("summary")?,
                current: id("summary-copy")?,
            },
            ConversationChange::NoteMoved {
                note: id("201")?,
                retained: id("finding")?,
                current: id("foreign")?,
            },
            ConversationChange::NoteMoved {
                note: id("300")?,
                retained: id("foreign")?,
                current: id("finding")?,
            },
        ]
    );
    let without_finding = rebuild(&retained, |parts| {
        parts.remove(1);
        Ok(())
    })?;
    assert_eq!(
        without_finding.changes_to(&retained, &targets),
        [
            ConversationChange::DiscussionRemoved(id("finding")?),
            ConversationChange::NoteRemoved {
                discussion: id("finding")?,
                note: id("200")?,
            },
            ConversationChange::NoteRemoved {
                discussion: id("finding")?,
                note: id("201")?,
            },
        ]
    );
    Ok(())
}

#[test]
fn a_deleted_summary_note_is_a_change_but_its_discussion_is_not_a_thread() -> TestResult {
    let (retained, targets) = reviewed()?;
    let current = rebuild(&retained, |parts| {
        parts.remove(0);
        Ok(())
    })?;
    assert_eq!(
        current.changes_to(&retained, &targets),
        [ConversationChange::NoteRemoved {
            discussion: id("summary")?,
            note: id("100")?,
        }]
    );
    Ok(())
}

#[test]
fn an_own_thread_without_a_planned_effect_is_outside_the_targets() -> TestResult {
    let (retained, _) = reviewed()?;
    let summary_only = WriteTargets {
        summary_notes: BTreeSet::from([id("100")?]),
        threads: BTreeSet::new(),
    };
    let current = rebuild(&retained, |parts| {
        parts[0]
            .2
            .push(note("101", "77", "A reply to the summary.", None)?);
        let finding = &mut parts[1];
        finding.1 = Resolution::Resolved;
        finding.2.push(note("202", "91", "Automated reply.", None)?);
        finding.2[1].body = "Edited question.".into();
        Ok(())
    })?;
    assert_eq!(current.changes_to(&retained, &summary_only), []);
    let edited = rebuild(&current, |parts| {
        parts[0].2[0].body = "Edited summary.".into();
        Ok(())
    })?;
    assert_eq!(
        edited.changes_to(&retained, &summary_only),
        [ConversationChange::NoteBodyChanged(id("100")?)]
    );
    Ok(())
}

#[test]
fn without_targets_only_a_different_subject_is_reported() -> TestResult {
    let (retained, _) = reviewed()?;
    let edited = rebuild(&retained, |parts| {
        parts[1].1 = Resolution::Resolved;
        parts[1].2.push(note("202", "5", "One more case.", None)?);
        parts.remove(0);
        Ok(())
    })?;
    let none = WriteTargets::default();
    let same = with_text(&edited, subject(1)?, "New title", "New description")?;
    assert!(!same.changes_since(&retained).is_empty());
    assert_eq!(same.changes_to(&retained, &none), []);
    let other = with_text(&edited, subject(2)?, "New title", "New description")?;
    assert_eq!(
        other.changes_to(&retained, &none),
        [ConversationChange::Subject]
    );
    Ok(())
}

#[test]
fn author_accounts_are_context_and_never_a_change() -> TestResult {
    let (retained, targets) = reviewed()?;
    let current = rebuild(&retained, |parts| {
        for (_, _, notes) in parts.iter_mut() {
            for note in notes.iter_mut() {
                note.account = match note.id.as_str() {
                    "200" => Some(AuthorAccount {
                        username: Some("renamed-reviewer".into()),
                        bot: BotStatus::Unknown,
                    }),
                    "201" => None,
                    _ => Some(AuthorAccount {
                        username: None,
                        bot: BotStatus::Bot,
                    }),
                };
            }
        }
        Ok(())
    })?;
    assert_ne!(current, retained);
    assert_eq!(current.changes_since(&retained), []);
    assert!(current.matches(&retained));
    assert_eq!(current.changes_to(&retained, &targets), []);
    let reassigned = rebuild(&retained, |parts| {
        parts[1].2[1].author = ReviewerId::try_from("6".to_owned())?;
        Ok(())
    })?;
    assert_eq!(
        reassigned.changes_to(&retained, &targets),
        [ConversationChange::NoteAuthorChanged(id("201")?)]
    );
    Ok(())
}
