use crate::observation::json_error;
use review_kit_observation::{Completion, Fields};

use review_kit_application::{
    conversation::{Conversation, Discussion, Note, Position, Resolution},
    publication::ProviderId,
};
use review_kit_domain::ReviewerId;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::{
    JsonError,
    domain::{SubjectData, valid},
    validate_version,
};

document_kind!(ConversationKind, ReviewConversation, "review_conversation");

/// Retained supported merge-request conversation.
///
/// `observed_at` is the caller's RFC 3339 UTC capture time. It is not part of
/// the application model and never participates in freshness comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationDocument {
    pub version: u64,
    pub kind: ConversationKind,
    pub subject: SubjectData,
    pub observed_at: String,
    pub title: String,
    pub description: String,
    pub discussions: Vec<ConversationDiscussionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationDiscussionData {
    pub id: String,
    pub resolvable: bool,
    /// Present in every document; `null` exactly when not resolvable.
    #[serde(deserialize_with = "required_nullable")]
    pub resolved: Option<bool>,
    pub notes: Vec<ConversationNoteData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationNoteData {
    pub id: String,
    pub author: String,
    pub body: String,
    /// The provider position object, or `null` for a note without an anchor.
    /// Every key and value is retained; key order and spacing are not significant.
    #[serde(deserialize_with = "required_nullable")]
    pub position: Option<Map<String, Value>>,
}

#[review_kit_observation::operations]
impl ConversationDocument {
    /// Version 1 document for `conversation` captured at `observed_at`, an
    /// RFC 3339 UTC time such as `2026-10-13T12:00:00Z`.
    #[helper]
    pub fn new(conversation: &Conversation, observed_at: String) -> Result<Self, JsonError> {
        validate_observed_at(&observed_at)?;
        Ok(Self {
            version: 1,
            kind: ConversationKind::ReviewConversation,
            subject: conversation.subject().into(),
            observed_at,
            title: conversation.title().into(),
            description: conversation.description().into(),
            discussions: conversation
                .discussions()
                .iter()
                .map(ConversationDiscussionData::from_discussion)
                .collect::<Result<_, _>>()?,
        })
    }

    /// Validates the document and restores the application conversation in
    /// stored order.
    #[operation(
        name = "json.conversation",
        input = Fields::new().count("discussions", self.discussions.len()),
        output = |result| {
            Completion::result(
                result,
                |conversation| Fields::new().count("notes", conversation.note_count()),
                json_error,
            )
        },
    )]
    pub fn into_conversation(self) -> Result<Conversation, JsonError> {
        validate_version(self.version)?;
        validate_observed_at(&self.observed_at)?;
        let subject = self.subject.into_subject()?;
        let discussions = self
            .discussions
            .into_iter()
            .map(ConversationDiscussionData::into_discussion)
            .collect::<Result<_, _>>()?;
        valid(Conversation::new(
            subject,
            self.title,
            self.description,
            discussions,
        ))
    }
}

#[review_kit_observation::operations]
impl ConversationDiscussionData {
    #[helper]
    fn from_discussion(discussion: &Discussion) -> Result<Self, JsonError> {
        Ok(Self {
            id: discussion.id().as_str().into(),
            resolvable: discussion.resolution().resolvable(),
            resolved: discussion.resolution().resolved(),
            notes: discussion
                .notes()
                .iter()
                .map(ConversationNoteData::from_note)
                .collect::<Result<_, _>>()?,
        })
    }

    #[helper]
    fn into_discussion(self) -> Result<Discussion, JsonError> {
        let notes = self
            .notes
            .into_iter()
            .map(ConversationNoteData::into_note)
            .collect::<Result<_, _>>()?;
        valid(Discussion::new(
            valid(ProviderId::new(self.id))?,
            valid(Resolution::from_flags(self.resolvable, self.resolved))?,
            notes,
        ))
    }
}

#[review_kit_observation::operations]
impl ConversationNoteData {
    #[helper]
    fn from_note(note: &Note) -> Result<Self, JsonError> {
        Ok(Self {
            id: note.id.as_str().into(),
            author: note.author.as_str().into(),
            body: note.body.clone(),
            position: note
                .position
                .as_ref()
                .map(|position| serde_json::from_str(position.as_str()))
                .transpose()
                .map_err(JsonError::Syntax)?,
        })
    }

    #[helper]
    fn into_note(self) -> Result<Note, JsonError> {
        Ok(Note {
            id: valid(ProviderId::new(self.id))?,
            author: valid(ReviewerId::try_from(self.author))?,
            body: self.body,
            position: self.position.as_ref().map(canonical_position).transpose()?,
        })
    }
}

/// Canonical text of a provider position object for freshness comparison.
///
/// Object keys are sorted by byte order at every depth, scalars use serde_json
/// formatting and no insignificant whitespace is written. Provider adapters and
/// retained documents share this function so equal objects compare equal.
#[review_kit_observation::operations]
#[helper]
pub fn canonical_position(position: &Map<String, Value>) -> Result<Position, JsonError> {
    let mut text = String::new();
    write_object(position, &mut text)?;
    valid(Position::from_canonical_json(text))
}

#[review_kit_observation::operations]
#[helper]
fn write_object(object: &Map<String, Value>, text: &mut String) -> Result<(), JsonError> {
    let mut entries: Vec<_> = object.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    text.push('{');
    for (index, (key, value)) in entries.into_iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&serde_json::to_string(key).map_err(JsonError::Syntax)?);
        text.push(':');
        write_value(value, text)?;
    }
    text.push('}');
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
fn write_value(value: &Value, text: &mut String) -> Result<(), JsonError> {
    match value {
        Value::Object(object) => write_object(object, text),
        Value::Array(items) => {
            text.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                write_value(item, text)?;
            }
            text.push(']');
            Ok(())
        }
        scalar => {
            text.push_str(&serde_json::to_string(scalar).map_err(JsonError::Syntax)?);
            Ok(())
        }
    }
}

/// Accepts `YYYY-MM-DDTHH:MM:SS[.fraction]Z` with in-range fields.
#[review_kit_observation::operations]
#[helper]
fn validate_observed_at(text: &str) -> Result<(), JsonError> {
    let bytes = text.as_bytes();
    let field = |start: usize, end: usize| -> Option<u32> {
        let digits = bytes.get(start..end)?;
        digits.iter().try_fold(0u32, |total, byte| {
            byte.is_ascii_digit()
                .then(|| total * 10 + u32::from(byte - b'0'))
        })
    };
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]
        .into_iter()
        .all(|(index, expected)| bytes.get(index) == Some(&expected));
    let fraction = match bytes.get(19..) {
        Some([b'Z']) => true,
        Some([b'.', digits @ .., b'Z']) => {
            !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    };
    let in_range = field(0, 4).is_some()
        && field(5, 7).is_some_and(|month| (1..=12).contains(&month))
        && field(8, 10).is_some_and(|day| (1..=31).contains(&day))
        && field(11, 13).is_some_and(|hour| hour <= 23)
        && field(14, 16).is_some_and(|minute| minute <= 59)
        && field(17, 19).is_some_and(|second| second <= 60);
    if separators && fraction && in_range {
        Ok(())
    } else {
        Err(JsonError::InvalidValue)
    }
}

/// Makes an `Option` field required while still accepting `null`.
#[review_kit_observation::operations]
#[helper]
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
