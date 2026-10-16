use crate::observation;
use review_kit_observation::Fields;
mod mapping;
pub(crate) mod records;

use std::collections::{BTreeMap, BTreeSet};

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::{Snapshot, SummaryRecord},
};
use review_kit_domain::{FindingId, ReviewerId, Subject};

use crate::source::ObservedPosition;
use crate::{
    FindingConflict, GitLab, GitLabError, NoteId,
    http::segment,
    wire::{self, Marker},
};
use mapping::{map_finding, map_reply, provider_id, retained_rounds};
use records::{Discussion, Note, index_notes};

#[review_kit_observation::operations]
impl ReviewReader for GitLab {
    #[operation(
        name = "gitlab.discover",
        input = Fields::new(),
        output = |result| {
            observation::port_result(result, |snapshot| {
                Fields::new()
                    .count("findings", snapshot.findings.len())
                    .count("summaries", snapshot.summaries.len())
                    .count("replies", snapshot.replies.len())
            })
        },
    )]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
        self.check_subject(subject)
            .and_then(|()| self.snapshot())
            .map_err(|error| PortError::new("GitLab discovery", error))
    }
}

#[review_kit_observation::operations]
impl GitLab {
    #[operation(
        name = "gitlab.snapshot",
        input = Fields::new(),
        output = |result| {
            observation::result(result, |snapshot| {
                Fields::new()
                    .count("findings", snapshot.findings.len())
                    .count("summaries", snapshot.summaries.len())
                    .count("replies", snapshot.replies.len())
            })
        },
    )]
    pub(crate) fn snapshot(&self) -> Result<Snapshot, GitLabError> {
        self.discover_with_outdated_positions()
            .map(|(snapshot, _)| snapshot)
    }

    /// Discover verified history and the findings whose tracked source changed.
    /// Outdated positions retain their creation evidence and grant no inline-write authority.
    #[operation(
        name = "gitlab.discover_positions",
        input = Fields::new(),
        output = |result| observation::result(result, |(_, outdated)| Fields::new().count("outdated_positions", outdated.len())),
    )]
    pub fn discover_with_outdated_positions(
        &self,
    ) -> Result<(Snapshot, Vec<FindingId>), GitLabError> {
        let request = self.refresh()?;
        let revision = request.revision()?;
        let path = self.mr_path();
        let notes: Vec<Note> = self.api.pages(&format!("{path}/notes"), &[])?;
        let discussions: Vec<Discussion> = self.api.pages(&format!("{path}/discussions"), &[])?;
        let indexed = index_notes(notes, &discussions, &self.binding)?;
        let mut markers = BTreeMap::new();
        let mut summaries = Vec::new();
        for note in indexed.values() {
            let Some((body, marker)) = owned_marker(note, &self.binding.reviewer)? else {
                continue;
            };
            if marker == Marker::Summary {
                if note.position.is_some() {
                    return Err(GitLabError::State);
                }
                let Some((body, current, history)) = wire::decode_summary(&note.body)? else {
                    return Err(GitLabError::State);
                };
                let observed: Note = self.api.get(&format!("{path}/notes/{}", note.id), &[])?;
                observed.check_scope(&self.binding)?;
                if observed.position.is_some() {
                    return Err(GitLabError::State);
                }
                note.matches_readback(&observed)?;
                summaries.push(SummaryRecord {
                    id: provider_id(note.id)?,
                    author: self.binding.reviewer.clone(),
                    body,
                    current,
                    history,
                });
            }
            markers.insert(note.id, (body, marker));
        }
        let rounds = retained_rounds(&summaries, &self.binding)?;
        let mut findings = Vec::new();
        let mut replies = Vec::new();
        let mut consumed = BTreeSet::new();
        let mut finding_ids = BTreeSet::new();
        let mut outdated = Vec::new();
        for discussion in &discussions {
            let root = discussion.notes.first().ok_or(GitLabError::Protocol)?;
            let Some((
                body,
                marker @ Marker::Finding {
                    finding: number, ..
                },
            )) = markers.get(&root.id)
            else {
                continue;
            };
            let claimed = FindingId::try_from(*number).map_err(|_| GitLabError::Marker)?;
            let observed: Discussion = self.api.get(
                &format!("{path}/discussions/{}", segment(&discussion.id)),
                &[],
            )?;
            observed.check_scope(&self.binding)?;
            if observed != *discussion {
                return Err(GitLabError::Conflict);
            }
            let finding = map_finding(discussion, root, body.clone(), marker, &rounds)
                .map_err(|error| inconsistent(error, claimed))?;
            let position = self.verify_observed_position(
                root.position
                    .as_ref()
                    .ok_or(GitLabError::UnsupportedPosition)?,
                &finding.revision,
                &finding.finding,
            )?;
            let contents = self.suggestion_contents(&finding.revision, &finding.finding)?;
            match position {
                ObservedPosition::Verified { range } => {
                    root.verify_suggestions(&finding.finding, contents.as_ref(), range)?;
                }
                ObservedPosition::Outdated => outdated.push(finding.finding.id()),
            }
            if !finding_ids.insert(finding.finding.id()) {
                return Err(GitLabError::FindingHistory {
                    finding: finding.finding.id(),
                    conflict: FindingConflict::DuplicateRoot,
                });
            }
            consumed.insert(root.id);
            for note in discussion.notes.iter().skip(1) {
                if note.system {
                    continue;
                }
                let marked = markers.get(&note.id).cloned();
                let reply = map_reply(note, &finding, &self.binding.reviewer, marked, &rounds)
                    .map_err(|error| inconsistent(error, finding.finding.id()))?;
                if reply.action.is_some()
                    && replies.iter().any(
                        |previous: &review_kit_application::publication::ReplyRecord| {
                            previous.action == reply.action
                        },
                    )
                {
                    return Err(GitLabError::FindingHistory {
                        finding: finding.finding.id(),
                        conflict: FindingConflict::Inconsistent,
                    });
                }
                replies.push(reply);
                consumed.insert(note.id);
            }
            findings.push(finding);
        }
        for (id, (_, marker)) in markers {
            if !consumed.contains(&id) {
                unconsumed(&marker)?;
            }
        }
        if self.refresh()?.revision()? != revision {
            return Err(GitLabError::RevisionChanged);
        }
        findings.sort_by_key(|root| root.finding.id());
        outdated.sort();
        Ok((
            Snapshot {
                subject: self.binding.subject.clone(),
                reviewer: self.binding.reviewer.clone(),
                revision,
                summaries,
                findings,
                replies,
                permissions: self.provider_permissions(&request),
            },
            outdated,
        ))
    }
}

/// A finding root note owned by the reviewer, as GitLab lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRoot {
    pub note: NoteId,
    /// GitLab's `created_at` text, verbatim; `None` when GitLab omits it.
    pub created_at: Option<String>,
}

#[review_kit_observation::operations]
impl GitLab {
    /// Every discussion root owned by the reviewer whose finding marker claims
    /// `finding`, in GitLab's discussion order.
    ///
    /// It diagnoses `FindingConflict::DuplicateRoot`, which stops discovery at
    /// the second root. Only note scope and markers are checked; the rest of
    /// the managed history is not reconciled. Nothing is written.
    #[operation(
        name = "gitlab.finding_roots",
        input = Fields::new().number("finding", finding.get()),
        output = |result| observation::result(result, |roots| Fields::new().count("roots", roots.len())),
    )]
    pub fn finding_roots(&self, finding: FindingId) -> Result<Vec<ManagedRoot>, GitLabError> {
        let path = self.mr_path();
        let notes: Vec<Note> = self.api.pages(&format!("{path}/notes"), &[])?;
        let discussions: Vec<Discussion> = self.api.pages(&format!("{path}/discussions"), &[])?;
        let indexed = index_notes(notes, &discussions, &self.binding)?;
        let mut roots = Vec::new();
        for discussion in &discussions {
            let Some(root) = discussion
                .notes
                .first()
                .and_then(|root| indexed.get(&root.id))
            else {
                continue;
            };
            if let Some((
                _,
                Marker::Finding {
                    finding: number, ..
                },
            )) = owned_marker(root, &self.binding.reviewer)?
                && FindingId::try_from(number).is_ok_and(|claimed| claimed == finding)
            {
                roots.push(ManagedRoot {
                    note: NoteId::new(root.id)?,
                    created_at: root.created_at.clone(),
                });
            }
        }
        Ok(roots)
    }
}

/// An owned record outside every finding thread: a reply whose root is gone, or a
/// finding marker that is not a thread root. Only the summary stands alone.
#[review_kit_observation::operations]
#[helper]
fn unconsumed(marker: &Marker) -> Result<(), GitLabError> {
    let (finding, conflict) = match marker {
        Marker::Summary => return Ok(()),
        Marker::Finding { finding, .. } => (finding, FindingConflict::Inconsistent),
        Marker::Transition { finding, .. } | Marker::Answer { finding, .. } => {
            (finding, FindingConflict::MissingRoot)
        }
    };
    let finding = FindingId::try_from(*finding).map_err(|_| GitLabError::Marker)?;
    Err(GitLabError::FindingHistory { finding, conflict })
}

/// A retained-state or conflict failure while mapping one finding's thread
/// becomes a history conflict of that finding; other failures keep their category.
#[review_kit_observation::operations]
#[helper]
fn inconsistent(error: GitLabError, finding: FindingId) -> GitLabError {
    match error {
        GitLabError::State | GitLabError::Conflict => GitLabError::FindingHistory {
            finding,
            conflict: FindingConflict::Inconsistent,
        },
        error => error,
    }
}

#[review_kit_observation::operations]
#[helper]
fn owned_marker(
    note: &Note,
    reviewer: &ReviewerId,
) -> Result<Option<(String, Marker)>, GitLabError> {
    if !note.owned_by(reviewer) {
        return Ok(None);
    }
    let marker = wire::parse_marker(&note.body)?;
    if note.system && marker.is_some() {
        return Err(GitLabError::Marker);
    }
    Ok(marker)
}

#[cfg(test)]
mod tests;
