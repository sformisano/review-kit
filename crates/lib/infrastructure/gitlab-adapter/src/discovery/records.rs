use std::collections::{BTreeMap, BTreeSet};

use review_kit_domain::{Finding, ReviewerId};
use serde::Deserialize;
use serde_json::Value;

use crate::{Binding, GitLabError};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Author {
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Note {
    pub id: u64,
    pub body: String,
    pub author: Author,
    pub system: bool,
    pub noteable_id: u64,
    pub noteable_type: String,
    pub project_id: u64,
    pub noteable_iid: Option<u64>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub position: Option<Value>,
    pub resolvable: Option<bool>,
    pub resolved: Option<bool>,
    pub suggestions: Option<Vec<ObservedSuggestion>>,
    /// GitLab's creation time, kept verbatim for diagnostics only.
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ObservedSuggestion {
    pub from_line: u64,
    pub to_line: u64,
    pub from_content: String,
    pub to_content: String,
    pub applied: bool,
}

impl Note {
    pub fn check_scope(&self, binding: &Binding) -> Result<(), GitLabError> {
        if self.id == 0
            || self.author.id == 0
            || self.project_id != binding.target_project.get()
            || self.noteable_id != binding.mr_id
            || self.noteable_type != "MergeRequest"
            || self
                .noteable_iid
                .is_some_and(|iid| iid != binding.iid.get())
        {
            return Err(GitLabError::Identity);
        }
        Ok(())
    }

    pub fn owned_by(&self, reviewer: &ReviewerId) -> bool {
        self.author.id.to_string() == reviewer.as_str()
    }

    pub fn reviewer(&self) -> Result<ReviewerId, GitLabError> {
        self.author
            .id
            .to_string()
            .try_into()
            .map_err(|_| GitLabError::Identity)
    }

    pub fn verify_suggestions(
        &self,
        finding: &Finding,
        contents: Option<&(String, String)>,
        range: Option<(u64, u64)>,
    ) -> Result<(), GitLabError> {
        match (finding.suggestion(), self.suggestions.as_deref()) {
            (None, None | Some([])) => Ok(()),
            (Some(_), Some([observed]))
                if range == Some((observed.from_line, observed.to_line))
                    && contents.is_some_and(|(from, to)| {
                        &observed.from_content == from && &observed.to_content == to
                    }) =>
            {
                Ok(())
            }
            (Some(_), None) => Err(GitLabError::Protocol),
            _ => Err(GitLabError::Conflict),
        }
    }

    pub fn matches_readback(&self, observed: &Self) -> Result<(), GitLabError> {
        let mut combined = self.clone();
        combined.merge(observed)
    }

    fn merge(&mut self, other: &Self) -> Result<(), GitLabError> {
        if self.id != other.id
            || self.body != other.body
            || self.author != other.author
            || self.system != other.system
            || self.noteable_id != other.noteable_id
            || self.noteable_type != other.noteable_type
            || self.project_id != other.project_id
        {
            return Err(GitLabError::Conflict);
        }
        merge_optional(&mut self.noteable_iid, &other.noteable_iid)?;
        merge_optional(&mut self.kind, &other.kind)?;
        merge_optional(&mut self.position, &other.position)?;
        merge_optional(&mut self.resolvable, &other.resolvable)?;
        merge_optional(&mut self.resolved, &other.resolved)?;
        merge_optional(&mut self.suggestions, &other.suggestions)
    }
}

fn merge_optional<T: Clone + Eq>(
    value: &mut Option<T>,
    other: &Option<T>,
) -> Result<(), GitLabError> {
    match (&*value, other) {
        (Some(first), Some(second)) if first != second => Err(GitLabError::Conflict),
        (None, Some(_)) => {
            *value = other.clone();
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Discussion {
    pub id: String,
    pub individual_note: bool,
    pub notes: Vec<Note>,
}

impl Discussion {
    pub fn check_scope(&self, binding: &Binding) -> Result<(), GitLabError> {
        if self.id.is_empty() || self.id.chars().any(char::is_control) || self.notes.is_empty() {
            return Err(GitLabError::Protocol);
        }
        if self.individual_note && self.notes.len() != 1 {
            return Err(GitLabError::Protocol);
        }
        let mut note_ids = BTreeSet::new();
        for note in &self.notes {
            note.check_scope(binding)?;
            if !note_ids.insert(note.id) {
                return Err(GitLabError::Protocol);
            }
        }
        Ok(())
    }

    pub fn resolved(&self) -> Result<bool, GitLabError> {
        let root = self.notes.first().ok_or(GitLabError::Protocol)?;
        if root.resolvable != Some(true) {
            return Err(GitLabError::UnsupportedPosition);
        }
        let mut resolved = true;
        for note in &self.notes {
            if note.resolvable == Some(true) {
                resolved &= note.resolved.ok_or(GitLabError::Protocol)?;
            }
        }
        Ok(resolved)
    }
}

pub(super) fn index_notes(
    notes: Vec<Note>,
    discussions: &[Discussion],
    binding: &Binding,
) -> Result<BTreeMap<u64, Note>, GitLabError> {
    let mut indexed = BTreeMap::new();
    for note in notes {
        note.check_scope(binding)?;
        if indexed.insert(note.id, note).is_some() {
            return Err(GitLabError::Protocol);
        }
    }
    let mut discussion_ids = BTreeSet::new();
    let mut threaded_ids = BTreeSet::new();
    for discussion in discussions {
        discussion.check_scope(binding)?;
        if !discussion_ids.insert(&discussion.id) {
            return Err(GitLabError::Protocol);
        }
        for note in &discussion.notes {
            if !threaded_ids.insert(note.id) {
                return Err(GitLabError::Protocol);
            }
            match indexed.get_mut(&note.id) {
                Some(existing) => existing.merge(note)?,
                None => {
                    indexed.insert(note.id, note.clone());
                }
            }
        }
    }
    Ok(indexed)
}
