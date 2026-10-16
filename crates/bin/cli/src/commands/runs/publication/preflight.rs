use review_kit_application::{
    conversation::ConversationChange,
    publication::Snapshot,
    review_run::RunId,
    use_cases::{
        discovery::discover,
        publishing::{ManagedHistory, OwnedHistory},
    },
};
use review_kit_gitlab_adapter::GitLab;
use serde_json::{Value, json};

use crate::{
    commands::{
        history,
        runs::{
            journal::{JournalReading, pending_owner},
            loading::LoadedRun,
        },
    },
    error::CliError,
};

/// Whether preflight compares the fresh merge request with the run's retained snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Before any write of this run's binding could have reached the provider.
    Required,
    /// After possible dispatch, recovery uses the original snapshot and per-effect checks.
    Skipped,
}

/// Discover the merge request's current provider state.
#[review_kit_observation::operations]
#[helper]
pub fn discover_current(gitlab: &GitLab, run: &LoadedRun) -> Result<Snapshot, CliError> {
    discover(
        &mut gitlab.clone(),
        &run.manifest.subject,
        &run.manifest.reviewer,
    )
    .map_err(|error| history::discovery_error(error, gitlab))
}

/// Reject a merge request whose source or conversation changed since the run's capture.
///
/// Revision identity covers the head and full comparison. Conversation equality
/// ignores ordering and observation time.
#[review_kit_observation::operations]
#[helper]
pub fn require_fresh(
    gitlab: &GitLab,
    run: &LoadedRun,
    snapshot: &Snapshot,
) -> Result<(), CliError> {
    let subject = &run.manifest.subject;
    if snapshot.revision != run.manifest.revision {
        return Err(CliError::state(
            "source_changed",
            3,
            "the merge request revision differs from this run's captured revision; start a new run with rkc run start --new",
        )
        .with_details(json!({
            "captured_head": run.manifest.revision.head(),
            "current_head": snapshot.revision.head(),
        })));
    }
    let current = gitlab
        .capture_conversation(subject)
        .map_err(CliError::GitLab)?;
    let changes = current.changes_since(&run.conversation);
    if !changes.is_empty() {
        return Err(CliError::state(
            "conversation_changed",
            3,
            "the merge-request conversation changed since this run captured it; start a new run with rkc run start --new",
        )
        .with_details(json!({"changes": changes.iter().map(change).collect::<Vec<_>>()})));
    }
    Ok(())
}

/// Refuse a new publication while another run owns unfinished publication.
///
/// Reads the journal as this run's admission would: an owner that sent
/// nothing is released, and an owner whose bound round the provider shows
/// terminal is settled. Returns that settled owner. Any other unsettled owner
/// is `publication_pending_owner`, reported before freshness because only the
/// owner's recovery or abandonment can move the merge request forward. Its
/// `next` lists the documented options instead when the owner's uncertain
/// creation can no longer be resent.
#[review_kit_observation::operations]
#[helper]
pub fn admit_owner(
    reading: &JournalReading,
    run: &LoadedRun,
    snapshot: &Snapshot,
) -> Result<Option<RunId>, CliError> {
    let JournalReading::Journal(stored) = reading else {
        return Ok(None);
    };
    let journal = stored
        .released_for_admission(run.run_id())
        .unwrap_or_else(|| stored.as_ref().clone());
    let Some(binding) = &journal.coverage_binding else {
        return Ok(None);
    };
    if &binding.run_id == run.run_id()
        || journal
            .settled_runs
            .iter()
            .any(|entry| entry.run_id == binding.run_id)
    {
        return Ok(None);
    }
    if journal.unresolved.is_some()
        || ManagedHistory::from(snapshot).state() != OwnedHistory::Finished(binding.round)
    {
        let refused = pending_owner(&binding.run_id, false);
        return Err(
            match super::guidance::stranded_creation(&journal, &snapshot.revision) {
                Some(stranded) => refused.with_detail(
                    "next",
                    json!(super::guidance::stranded_guidance(
                        &stranded,
                        Some(&binding.run_id)
                    )),
                ),
                None => refused,
            },
        );
    }
    Ok(Some(binding.run_id.clone()))
}

/// A conversation change described by kind and identifiers only.
#[review_kit_observation::operations]
#[helper]
fn change(change: &ConversationChange) -> Value {
    match change {
        ConversationChange::Subject => json!({"kind": "subject"}),
        ConversationChange::Title => json!({"kind": "title"}),
        ConversationChange::Description => json!({"kind": "description"}),
        ConversationChange::DiscussionAdded(id) => {
            json!({"kind": "discussion_added", "discussion": id.as_str()})
        }
        ConversationChange::DiscussionRemoved(id) => {
            json!({"kind": "discussion_removed", "discussion": id.as_str()})
        }
        ConversationChange::ResolutionChanged { discussion, .. } => {
            json!({"kind": "resolution_changed", "discussion": discussion.as_str()})
        }
        ConversationChange::NoteAdded { discussion, note } => {
            json!({"kind": "note_added", "discussion": discussion.as_str(), "note": note.as_str()})
        }
        ConversationChange::NoteRemoved { discussion, note } => {
            json!({"kind": "note_removed", "discussion": discussion.as_str(), "note": note.as_str()})
        }
        ConversationChange::NoteMoved { note, .. } => {
            json!({"kind": "note_moved", "note": note.as_str()})
        }
        ConversationChange::NoteAuthorChanged(note) => {
            json!({"kind": "note_author_changed", "note": note.as_str()})
        }
        ConversationChange::NoteBodyChanged(note) => {
            json!({"kind": "note_body_changed", "note": note.as_str()})
        }
        ConversationChange::NotePositionChanged(note) => {
            json!({"kind": "note_position_changed", "note": note.as_str()})
        }
    }
}
