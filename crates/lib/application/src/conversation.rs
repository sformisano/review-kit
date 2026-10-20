//! Supported merge-request conversation retained as review context.
//!
//! A conversation holds the request title and description and every discussion
//! with at least one supported non-system note. It is context only: it grants no
//! managed finding identity, reply authority, policy selection or effect
//! authority.
//!
//! Comparison ignores order: discussions and notes match by identifier and
//! positions compare as canonical JSON text. Note authors compare by provider
//! user identity only; account details such as the username are context.
//! Observation time and provider ordering are not part of the model.
//!
//! Publication freshness considers only [`WriteTargets`]: the summary note and
//! the finding threads that one publication writes to. Activity elsewhere,
//! including the reviewer's other threads and title and description edits,
//! does not make an assessed snapshot stale.

use std::collections::{BTreeMap, BTreeSet};

use review_kit_domain::{FindingId, ReviewerId, Subject};

use crate::publication::{Plan, PlannedAction, ProviderId, Snapshot, Write};

/// Canonical JSON text of one provider position object.
///
/// The decoding adapter produces the text: object keys sorted by byte order at
/// every depth and no insignificant whitespace. Equal positions therefore have
/// equal text. The application compares the text and never parses it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position(String);

impl Position {
    /// Wraps adapter-produced canonical text of a JSON object.
    ///
    /// Only the object delimiters are checked; canonical form is the adapter's
    /// contract.
    pub fn from_canonical_json(text: String) -> Result<Self, InvalidConversation> {
        if text.starts_with('{') && text.ends_with('}') {
            Ok(Self(text))
        } else {
            Err(InvalidConversation::Position)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Resolution state of a whole discussion.
///
/// A discussion is resolvable when any of its provider notes is resolvable. It
/// is resolved only when every resolvable note is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Resolution {
    NotResolvable,
    Unresolved,
    Resolved,
}

impl Resolution {
    /// Maps the stored `resolvable` and `resolved` pair.
    ///
    /// A resolved state requires a resolvable discussion, and a resolvable
    /// discussion requires a resolved state.
    pub fn from_flags(
        resolvable: bool,
        resolved: Option<bool>,
    ) -> Result<Self, InvalidConversation> {
        match (resolvable, resolved) {
            (false, None) => Ok(Self::NotResolvable),
            (true, Some(false)) => Ok(Self::Unresolved),
            (true, Some(true)) => Ok(Self::Resolved),
            _ => Err(InvalidConversation::Resolution),
        }
    }

    pub fn resolvable(self) -> bool {
        self != Self::NotResolvable
    }

    /// `None` when the discussion is not resolvable.
    pub fn resolved(self) -> Option<bool> {
        match self {
            Self::NotResolvable => None,
            Self::Unresolved => Some(false),
            Self::Resolved => Some(true),
        }
    }
}

/// One supported non-system note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: ProviderId,
    /// Provider user identity of the API author, including bot users.
    /// Comparisons identify the author by this value alone.
    pub author: ReviewerId,
    /// The author's account when the conversation was captured; `None` when
    /// the capture predates account details.
    pub account: Option<AuthorAccount>,
    pub body: String,
    /// `None` for a note without a diff anchor.
    pub position: Option<Position>,
}

/// A note author's provider account at capture time.
///
/// Usernames can change, so an account never identifies the author in
/// comparisons; [`Note::author`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorAccount {
    /// Nonblank provider username; `None` when the provider sent none.
    pub username: Option<String>,
    pub bot: BotStatus,
}

/// Whether the provider marks an account as automated.
///
/// `Unknown` means the capture could not establish it; it shows neither a bot
/// nor a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotStatus {
    Bot,
    NotBot,
    Unknown,
}

/// A provider discussion and its supported notes in retained order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discussion {
    id: ProviderId,
    resolution: Resolution,
    notes: Vec<Note>,
}

impl Discussion {
    /// Requires at least one note and unique note identifiers.
    pub fn new(
        id: ProviderId,
        resolution: Resolution,
        notes: Vec<Note>,
    ) -> Result<Self, InvalidConversation> {
        if notes.is_empty() {
            return Err(InvalidConversation::EmptyDiscussion(id));
        }
        let mut seen = BTreeSet::new();
        for note in &notes {
            if !seen.insert(&note.id) {
                return Err(InvalidConversation::DuplicateNote(note.id.clone()));
            }
        }
        Ok(Self {
            id,
            resolution,
            notes,
        })
    }

    pub fn id(&self) -> &ProviderId {
        &self.id
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }
}

/// Title, description and supported discussions of one review subject.
///
/// Discussion identifiers are unique, and note identifiers are unique across
/// all discussions. Structural equality (`==`) also compares retained order
/// and account details; comparisons use [`Conversation::changes_since`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    subject: Subject,
    title: String,
    description: String,
    discussions: Vec<Discussion>,
}

impl Conversation {
    pub fn new(
        subject: Subject,
        title: String,
        description: String,
        discussions: Vec<Discussion>,
    ) -> Result<Self, InvalidConversation> {
        let mut discussion_ids = BTreeSet::new();
        let mut note_ids = BTreeSet::new();
        for discussion in &discussions {
            if !discussion_ids.insert(&discussion.id) {
                return Err(InvalidConversation::DuplicateDiscussion(
                    discussion.id.clone(),
                ));
            }
            for note in &discussion.notes {
                if !note_ids.insert(&note.id) {
                    return Err(InvalidConversation::DuplicateNote(note.id.clone()));
                }
            }
        }
        Ok(Self {
            subject,
            title,
            description,
            discussions,
        })
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn discussions(&self) -> &[Discussion] {
        &self.discussions
    }

    pub fn note_count(&self) -> usize {
        self.discussions
            .iter()
            .map(|discussion| discussion.notes.len())
            .sum()
    }

    /// The same conversation with discussions and their notes ordered by ID.
    pub fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        normalized
            .discussions
            .sort_by(|left, right| left.id.cmp(&right.id));
        for discussion in &mut normalized.discussions {
            discussion
                .notes
                .sort_by(|left, right| left.id.cmp(&right.id));
        }
        normalized
    }

    /// Differences from `retained` to this conversation, in a stable order.
    ///
    /// An empty result means the conversations are equivalent. Note membership
    /// compares the containing discussion of each note ID. Account details are
    /// not compared, so a renamed author or a changed bot status is no change.
    pub fn changes_since(&self, retained: &Self) -> Vec<ConversationChange> {
        let mut changes = Vec::new();
        if self.subject != retained.subject {
            changes.push(ConversationChange::Subject);
        }
        if self.title != retained.title {
            changes.push(ConversationChange::Title);
        }
        if self.description != retained.description {
            changes.push(ConversationChange::Description);
        }
        let before = retained.discussions_by_id();
        let after = self.discussions_by_id();
        for (id, discussion) in &before {
            match after.get(id) {
                None => changes.push(ConversationChange::DiscussionRemoved((*id).clone())),
                Some(current) if current.resolution != discussion.resolution => {
                    changes.push(ConversationChange::ResolutionChanged {
                        discussion: (*id).clone(),
                        retained: discussion.resolution,
                        current: current.resolution,
                    });
                }
                Some(_) => {}
            }
        }
        for id in after.keys().filter(|id| !before.contains_key(*id)) {
            changes.push(ConversationChange::DiscussionAdded((*id).clone()));
        }
        let before = retained.notes_by_id();
        let after = self.notes_by_id();
        for (id, (discussion, note)) in &before {
            let Some((current_discussion, current)) = after.get(id) else {
                changes.push(ConversationChange::NoteRemoved {
                    discussion: (*discussion).clone(),
                    note: (*id).clone(),
                });
                continue;
            };
            if current_discussion != discussion {
                changes.push(ConversationChange::NoteMoved {
                    note: (*id).clone(),
                    retained: (*discussion).clone(),
                    current: (*current_discussion).clone(),
                });
            }
            if current.author != note.author {
                changes.push(ConversationChange::NoteAuthorChanged((*id).clone()));
            }
            if current.body != note.body {
                changes.push(ConversationChange::NoteBodyChanged((*id).clone()));
            }
            if current.position != note.position {
                changes.push(ConversationChange::NotePositionChanged((*id).clone()));
            }
        }
        for (id, (discussion, _)) in after.iter().filter(|(id, _)| !before.contains_key(*id)) {
            changes.push(ConversationChange::NoteAdded {
                discussion: (*discussion).clone(),
                note: (*id).clone(),
            });
        }
        changes.sort();
        changes
    }

    /// True when [`Conversation::changes_since`] finds no difference.
    pub fn matches(&self, other: &Self) -> bool {
        self.changes_since(other).is_empty()
    }

    /// The changes from `retained` that touch `targets`, in a stable order.
    ///
    /// Any change inside a target thread is reported, whoever made it,
    /// including its removal and notes moved into or out of it. A target
    /// summary note counts as a note only: its appearance, an edit, a move or
    /// its removal is reported, while replies in its discussion and that
    /// discussion's resolution are not. A different review subject is always
    /// reported. Title, description and every other discussion are out of
    /// scope.
    pub fn changes_to(&self, retained: &Self, targets: &WriteTargets) -> Vec<ConversationChange> {
        let before = retained.notes_by_id();
        let after = self.notes_by_id();
        let in_thread = |note: &ProviderId| {
            [before.get(note), after.get(note)]
                .into_iter()
                .flatten()
                .any(|(discussion, _)| targets.threads.contains(*discussion))
        };
        let in_scope = |note: &ProviderId| targets.summary_notes.contains(note) || in_thread(note);
        self.changes_since(retained)
            .into_iter()
            .filter(|change| match change {
                ConversationChange::Subject => true,
                ConversationChange::Title | ConversationChange::Description => false,
                ConversationChange::DiscussionAdded(discussion)
                | ConversationChange::DiscussionRemoved(discussion)
                | ConversationChange::ResolutionChanged { discussion, .. } => {
                    targets.threads.contains(discussion)
                }
                ConversationChange::NoteAdded { discussion, note }
                | ConversationChange::NoteRemoved { discussion, note } => {
                    targets.threads.contains(discussion) || targets.summary_notes.contains(note)
                }
                ConversationChange::NoteMoved {
                    note,
                    retained,
                    current,
                } => {
                    targets.threads.contains(retained)
                        || targets.threads.contains(current)
                        || targets.summary_notes.contains(note)
                }
                ConversationChange::NoteAuthorChanged(note)
                | ConversationChange::NoteBodyChanged(note)
                | ConversationChange::NotePositionChanged(note) => in_scope(note),
            })
            .collect()
    }

    fn discussions_by_id(&self) -> BTreeMap<&ProviderId, &Discussion> {
        self.discussions
            .iter()
            .map(|discussion| (&discussion.id, discussion))
            .collect()
    }

    fn notes_by_id(&self) -> BTreeMap<&ProviderId, (&ProviderId, &Note)> {
        self.discussions
            .iter()
            .flat_map(|discussion| {
                discussion
                    .notes
                    .iter()
                    .map(move |note| (&note.id, (&discussion.id, note)))
            })
            .collect()
    }
}

/// The existing provider objects that one publication invocation writes to:
/// the reviewer's summary notes and the finding threads that receive a reply,
/// a resolve or a reopen.
///
/// New finding threads are absent because they do not exist before the
/// invocation writes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteTargets {
    summary_notes: BTreeSet<ProviderId>,
    threads: BTreeSet<ProviderId>,
}

impl WriteTargets {
    /// The targets of executing `plan`, after the retained `pending` write.
    ///
    /// Each history is a verified discovery of the plan's subject. A plan
    /// with any write updates the reviewer's summary, so every history's
    /// summary note by its own reviewer is a target; passing the history
    /// captured with the assessment keeps a summary that later disappeared in
    /// scope. Reply and resolution actions name findings; their threads are
    /// the matching owned roots of every history. An unchanged plan writes
    /// nothing and has no targets of its own.
    pub fn of(plan: &Plan, histories: &[&Snapshot], pending: Option<&Write>) -> Self {
        let mut targets = Self::default();
        let findings: BTreeSet<FindingId> = plan
            .actions
            .iter()
            .filter_map(|action| match action {
                PlannedAction::Reply { finding, .. }
                | PlannedAction::SetResolved { finding, .. } => Some(*finding),
                PlannedAction::Finding { .. } => None,
            })
            .collect();
        for history in histories {
            if !plan.unchanged {
                targets.summary_notes.extend(
                    history
                        .summaries
                        .iter()
                        .filter(|summary| summary.author == history.reviewer)
                        .map(|summary| summary.id.clone()),
                );
            }
            targets.threads.extend(
                history
                    .findings
                    .iter()
                    .filter(|root| {
                        root.author == history.reviewer && findings.contains(&root.finding.id())
                    })
                    .map(|root| root.id.clone()),
            );
        }
        match pending {
            Some(Write::Summary {
                expected: Some(summary),
                ..
            }) => {
                targets.summary_notes.insert(summary.id.clone());
            }
            Some(Write::Reply { thread, .. } | Write::SetResolved { thread, .. }) => {
                targets.threads.insert(thread.clone());
            }
            Some(Write::Summary { expected: None, .. } | Write::Finding { .. }) | None => {}
        }
        targets
    }

    /// True when every target of `other` is also a target here.
    pub fn covers(&self, other: &Self) -> bool {
        other.summary_notes.is_subset(&self.summary_notes) && other.threads.is_subset(&self.threads)
    }
}

/// One difference between a retained and a current conversation.
///
/// Values identify the changed objects without copying titles, descriptions or
/// note bodies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConversationChange {
    /// The conversations describe different review subjects.
    Subject,
    Title,
    Description,
    DiscussionAdded(ProviderId),
    DiscussionRemoved(ProviderId),
    ResolutionChanged {
        discussion: ProviderId,
        retained: Resolution,
        current: Resolution,
    },
    NoteAdded {
        discussion: ProviderId,
        note: ProviderId,
    },
    NoteRemoved {
        discussion: ProviderId,
        note: ProviderId,
    },
    /// The note ID now belongs to a different discussion.
    NoteMoved {
        note: ProviderId,
        retained: ProviderId,
        current: ProviderId,
    },
    NoteAuthorChanged(ProviderId),
    NoteBodyChanged(ProviderId),
    NotePositionChanged(ProviderId),
}

/// The values cannot form a supported conversation.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum InvalidConversation {
    /// a conversation discussion has no supported notes
    EmptyDiscussion(ProviderId),
    /// a conversation discussion ID appears more than once
    DuplicateDiscussion(ProviderId),
    /// a conversation note ID appears more than once
    DuplicateNote(ProviderId),
    /// a note position is not a JSON object
    Position,
    /// discussion resolution flags are inconsistent
    Resolution,
}

#[cfg(test)]
mod tests;
