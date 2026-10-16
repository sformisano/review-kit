use crate::observation;
use review_kit_observation::Fields;

use std::collections::BTreeSet;

use review_kit_application::{
    conversation::{self, Conversation, Resolution},
    publication::ProviderId,
};
use review_kit_domain::Subject;
use serde_json::Value;

use crate::{
    GitLab, GitLabError,
    discovery::records::{Discussion, Note},
};

#[review_kit_observation::operations]
impl GitLab {
    /// Capture the bound merge request's title, description and every
    /// discussion with at least one non-system note.
    ///
    /// Bot notes are ordinary API-visible notes and are kept. Discussion pages
    /// are read to completion; an incomplete or over-limit collection fails.
    /// The reads are not an atomic provider snapshot.
    #[operation(
        name = "gitlab.capture_conversation",
        input = Fields::new(),
        output = |result| {
            observation::result(result, |conversation| {
                Fields::new()
                    .count("discussions", conversation.discussions().len())
                    .count("notes", conversation.note_count())
            })
        },
    )]
    pub fn capture_conversation(&self, subject: &Subject) -> Result<Conversation, GitLabError> {
        self.check_subject(subject)?;
        let request = self.refresh()?;
        let (title, description) = request.text()?;
        let discussions: Vec<Discussion> = self
            .api
            .pages(&format!("{}/discussions", self.mr_path()), &[])?;
        let mut ids = BTreeSet::new();
        let mut captured = Vec::new();
        for discussion in &discussions {
            discussion.check_scope(&self.binding)?;
            if !ids.insert(discussion.id.as_str()) {
                return Err(GitLabError::Protocol);
            }
            if let Some(discussion) = supported_discussion(discussion)? {
                captured.push(discussion);
            }
        }
        Conversation::new(self.binding.subject.clone(), title, description, captured)
            .map_err(|_| GitLabError::Protocol)
    }
}

/// `None` when every note in the discussion is a system note.
///
/// Resolution covers every provider note in the discussion; each note must
/// state `resolvable`, and a resolvable note must state `resolved`.
#[review_kit_observation::operations]
#[helper]
fn supported_discussion(
    discussion: &Discussion,
) -> Result<Option<conversation::Discussion>, GitLabError> {
    let mut resolvable = false;
    let mut resolved = true;
    for note in &discussion.notes {
        if note.resolvable.ok_or(GitLabError::Protocol)? {
            resolvable = true;
            resolved &= note.resolved.ok_or(GitLabError::Protocol)?;
        }
    }
    let notes = discussion
        .notes
        .iter()
        .filter(|note| !note.system)
        .map(supported_note)
        .collect::<Result<Vec<_>, _>>()?;
    if notes.is_empty() {
        return Ok(None);
    }
    let resolution = Resolution::from_flags(resolvable, resolvable.then_some(resolved))
        .map_err(|_| GitLabError::Protocol)?;
    let id = ProviderId::new(discussion.id.clone()).map_err(|_| GitLabError::Protocol)?;
    conversation::Discussion::new(id, resolution, notes)
        .map(Some)
        .map_err(|_| GitLabError::Protocol)
}

#[review_kit_observation::operations]
#[helper]
fn supported_note(note: &Note) -> Result<conversation::Note, GitLabError> {
    let position = match &note.position {
        None => None,
        Some(Value::Object(position)) => Some(
            review_kit_json_adapter::canonical_position(position)
                .map_err(|_| GitLabError::Protocol)?,
        ),
        Some(_) => return Err(GitLabError::Protocol),
    };
    Ok(conversation::Note {
        id: ProviderId::new(note.id.to_string()).map_err(|_| GitLabError::Protocol)?,
        author: note.reviewer()?,
        body: note.body.clone(),
        position,
    })
}
