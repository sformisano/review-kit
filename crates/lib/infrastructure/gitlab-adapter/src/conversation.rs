use crate::observation;
use review_kit_observation::Fields;

use std::collections::{BTreeMap, BTreeSet};

use review_kit_application::{
    conversation::{self, AuthorAccount, BotStatus, Conversation, Resolution},
    publication::ProviderId,
};
use review_kit_domain::Subject;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    GitLab, GitLabError,
    discovery::records::{Discussion, Note},
};

/// The most distinct note authors whose bot status one capture asks GitLab for.
///
/// Each lookup is one `GET /users/:id` request, which GitLab rate limits per
/// user; authors beyond the limit keep an unknown bot status.
pub const BOT_LOOKUP_LIMIT: usize = 50;

/// Whether conversation capture asks GitLab which note authors are bots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotLookup {
    /// Ask once per distinct author, for at most [`BOT_LOOKUP_LIMIT`] authors.
    Resolve,
    /// Ask nothing, for a comparison that ignores accounts; every bot status is unknown.
    Skip,
}

/// The part of a `GET /users/:id` response that capture reads.
#[derive(Deserialize)]
struct UserRecord {
    id: u64,
    #[serde(default)]
    bot: Option<bool>,
}

#[review_kit_observation::operations]
impl GitLab {
    /// Capture the bound merge request's title, description and every
    /// discussion with at least one non-system note.
    ///
    /// Bot notes are ordinary API-visible notes and are kept. Each note keeps
    /// its author's username from the discussion and, with
    /// [`BotLookup::Resolve`], the bot status that GitLab reports for the
    /// account. Discussion pages are read to completion; an incomplete or
    /// over-limit collection fails. The reads are not an atomic provider
    /// snapshot.
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
    pub fn capture_conversation(
        &self,
        subject: &Subject,
        lookup: BotLookup,
    ) -> Result<Conversation, GitLabError> {
        self.check_subject(subject)?;
        let request = self.refresh()?;
        let (title, description) = request.text()?;
        let discussions: Vec<Discussion> = self
            .api
            .pages(&format!("{}/discussions", self.mr_path()), &[])?;
        let mut ids = BTreeSet::new();
        for discussion in &discussions {
            discussion.check_scope(&self.binding)?;
            if !ids.insert(discussion.id.as_str()) {
                return Err(GitLabError::Protocol);
            }
        }
        let bots = match lookup {
            BotLookup::Resolve => self.bot_statuses(&discussions),
            BotLookup::Skip => BTreeMap::new(),
        };
        let mut captured = Vec::new();
        for discussion in &discussions {
            if let Some(discussion) = supported_discussion(discussion, &bots)? {
                captured.push(discussion);
            }
        }
        Conversation::new(self.binding.subject.clone(), title, description, captured)
            .map_err(|_| GitLabError::Protocol)
    }

    /// Bot status of each distinct non-system note author that GitLab reports.
    ///
    /// Authors are asked in discussion order, at most [`BOT_LOOKUP_LIMIT`]
    /// of them. A missing, hidden or malformed account stays unknown. Any
    /// other failure, such as a rate limit or transport loss, stops the
    /// lookups and leaves the remaining authors unknown; capture never fails
    /// for a lookup.
    #[helper]
    fn bot_statuses(&self, discussions: &[Discussion]) -> BTreeMap<u64, BotStatus> {
        let mut statuses = BTreeMap::new();
        let mut asked = BTreeSet::new();
        let authors = discussions
            .iter()
            .flat_map(|discussion| &discussion.notes)
            .filter(|note| !note.system)
            .map(|note| note.author.id);
        for author in authors {
            if asked.contains(&author) {
                continue;
            }
            if asked.len() == BOT_LOOKUP_LIMIT {
                break;
            }
            asked.insert(author);
            match self.api.get::<UserRecord>(&format!("users/{author}"), &[]) {
                Ok(UserRecord { id, bot: Some(bot) }) if id == author => {
                    statuses.insert(
                        author,
                        if bot {
                            BotStatus::Bot
                        } else {
                            BotStatus::NotBot
                        },
                    );
                }
                Ok(_)
                | Err(GitLabError::NotFound | GitLabError::Denied | GitLabError::Protocol) => {}
                Err(_) => break,
            }
        }
        statuses
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
    bots: &BTreeMap<u64, BotStatus>,
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
        .map(|note| supported_note(note, bots))
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

/// A note requires its author's nonblank username.
#[review_kit_observation::operations]
#[helper]
fn supported_note(
    note: &Note,
    bots: &BTreeMap<u64, BotStatus>,
) -> Result<conversation::Note, GitLabError> {
    let position = match &note.position {
        None => None,
        Some(Value::Object(position)) => Some(
            review_kit_json_adapter::canonical_position(position)
                .map_err(|_| GitLabError::Protocol)?,
        ),
        Some(_) => return Err(GitLabError::Protocol),
    };
    let username = note
        .author
        .username
        .clone()
        .filter(|username| !username.trim().is_empty())
        .ok_or(GitLabError::Protocol)?;
    Ok(conversation::Note {
        id: ProviderId::new(note.id.to_string()).map_err(|_| GitLabError::Protocol)?,
        author: note.reviewer()?,
        account: Some(AuthorAccount {
            username,
            bot: bots
                .get(&note.author.id)
                .copied()
                .unwrap_or(BotStatus::Unknown),
        }),
        body: note.body.clone(),
        position,
    })
}
