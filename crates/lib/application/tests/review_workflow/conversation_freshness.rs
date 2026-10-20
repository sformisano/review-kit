//! Publication freshness scoped to the threads of verified managed history:
//! the engine's published summary and finding root are owned; a foreign root
//! that claims the same finding is not.

use review_kit_application::conversation::{
    AuthorAccount, BotStatus, Conversation, ConversationChange, Discussion, Note, OwnedThreads,
    Resolution,
};

use super::*;

fn note(id: &str, author: &str, body: &str) -> Note {
    Note {
        id: provider_id(id),
        author: reviewer(author),
        account: Some(AuthorAccount {
            username: author.into(),
            bot: if author == "bot" {
                BotStatus::Bot
            } else {
                BotStatus::NotBot
            },
        }),
        body: body.into(),
        position: None,
    }
}

fn thread(id: &str, resolution: Resolution, notes: Vec<Note>) -> Discussion {
    Discussion::new(provider_id(id), resolution, notes).unwrap()
}

fn conversation(discussions: Vec<Discussion>) -> Conversation {
    Conversation::new(
        subject(),
        "Handle failed requests".into(),
        "Retry dropped requests.".into(),
        discussions,
    )
    .unwrap()
}

/// The forged root that `Host::new` seeds: another author's thread claiming finding 1.
fn forged() -> Discussion {
    thread(
        "foreign-root",
        Resolution::Unresolved,
        vec![note("foreign-root-note", "other", "forged owned marker")],
    )
}

/// The provider threads holding the engine's published summary and root.
fn published(host: &Host) -> Vec<Discussion> {
    let summary = &host.snapshot.summaries[0];
    let root = host
        .snapshot
        .findings
        .iter()
        .find(|root| root.author == reviewer("agent"))
        .unwrap();
    vec![
        thread(
            "summary-thread",
            Resolution::NotResolvable,
            vec![note(summary.id.as_str(), "agent", &summary.body)],
        ),
        thread(
            root.id.as_str(),
            Resolution::Unresolved,
            vec![note(root.root_note.as_str(), "agent", &root.body)],
        ),
    ]
}

#[test]
fn published_threads_are_owned_and_a_forged_foreign_root_is_not() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let captured = host.snapshot.clone();
    let owned = OwnedThreads::of([&captured, &host.snapshot]);
    let mut threads = published(&host);
    threads.push(forged());
    let retained = conversation(threads.clone());

    let mut foreign = threads.clone();
    foreign[2] = thread(
        "foreign-root",
        Resolution::Resolved,
        vec![
            note("foreign-root-note", "other", "forged owned marker, edited"),
            note("foreign-reply", "bot", "Automated reply."),
        ],
    );
    foreign.push(thread(
        "duo",
        Resolution::NotResolvable,
        vec![note("duo-note", "bot", "Automated review.")],
    ));
    let current = conversation(foreign);
    assert!(!current.changes_since(&retained).is_empty());
    assert_eq!(current.owned_changes_since(&retained, &owned), []);

    let mut inside = threads.clone();
    inside[0] = thread(
        "summary-thread",
        Resolution::NotResolvable,
        vec![note(
            captured.summaries[0].id.as_str(),
            "agent",
            "Edited by a maintainer.",
        )],
    );
    let root = inside[1].clone();
    let mut notes = root.notes().to_vec();
    notes.push(note("question", "other", "Why does this drop?"));
    inside[1] = thread(root.id().as_str(), Resolution::Resolved, notes);
    let current = conversation(inside);
    assert_eq!(
        current.owned_changes_since(&retained, &owned),
        [
            ConversationChange::ResolutionChanged {
                discussion: root.id().clone(),
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteAdded {
                discussion: root.id().clone(),
                note: provider_id("question"),
            },
            ConversationChange::NoteBodyChanged(captured.summaries[0].id.clone()),
        ]
    );
}

#[test]
fn threads_published_after_capture_are_owned_through_the_current_history() {
    let (mut host, mut source, mut renderer) = fixtures();
    let captured = host.snapshot.clone();
    let retained = conversation(vec![forged()]);
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut threads = published(&host);
    threads.push(forged());
    let current = conversation(threads);
    assert_eq!(
        current.owned_changes_since(&retained, &OwnedThreads::of([&captured])),
        [],
        "the captured history owns nothing"
    );
    let summary = host.snapshot.summaries[0].id.clone();
    let root = host
        .snapshot
        .findings
        .iter()
        .find(|root| root.author == reviewer("agent"))
        .unwrap();
    assert_eq!(
        current.owned_changes_since(&retained, &OwnedThreads::of([&captured, &host.snapshot])),
        [
            ConversationChange::DiscussionAdded(root.id.clone()),
            ConversationChange::DiscussionAdded(provider_id("summary-thread")),
            ConversationChange::NoteAdded {
                discussion: root.id.clone(),
                note: root.root_note.clone(),
            },
            ConversationChange::NoteAdded {
                discussion: provider_id("summary-thread"),
                note: summary,
            },
        ]
    );
}
