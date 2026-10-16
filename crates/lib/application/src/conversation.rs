//! Supported merge-request conversation retained as review context.
//!
//! A conversation holds the request title and description and every discussion
//! with at least one supported non-system note. It is context only: it grants no
//! managed finding identity, reply authority, policy selection or effect
//! authority.
//!
//! Freshness compares the normalized view. Discussions and notes are ordered by
//! identifier and positions compare as canonical JSON text. Observation time and
//! provider ordering are not part of the model.

use std::collections::{BTreeMap, BTreeSet};

use review_kit_domain::{ReviewerId, Subject};

use crate::publication::ProviderId;

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
    pub author: ReviewerId,
    pub body: String,
    /// `None` for a note without a diff anchor.
    pub position: Option<Position>,
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
/// all discussions. Structural equality (`==`) also compares retained order;
/// freshness checks use [`Conversation::changes_since`] or the normalized view.
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
    ///
    /// Two conversations are equivalent exactly when their normalized views are
    /// equal.
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
    /// An empty result means the normalized views are equal. Note membership
    /// compares the containing discussion of each note ID.
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

    /// True when the normalized views are equal.
    pub fn matches(&self, other: &Self) -> bool {
        self.changes_since(other).is_empty()
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
