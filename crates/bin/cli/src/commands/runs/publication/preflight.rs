use review_kit_application::{
    conversation::{ConversationChange, WriteTargets},
    publication::{Plan, Snapshot},
    review_run::RunId,
    use_cases::{
        discovery::discover,
        publishing::{
            ManagedHistory, OwnedHistory, PublicationProgress, check_review_with_progress,
        },
    },
};
use review_kit_gitlab_adapter::{BotLookup, GitLab};
use review_kit_json2md_adapter::Json2MdRenderer;
use serde_json::{Value, json};

use crate::{
    commands::{
        history,
        runs::{
            chain::AcceptedReview,
            journal::{JournalReading, pending_owner},
            loading::{LoadedRun, retained_source},
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

/// Reject a merge request whose source changed since the run's capture, or
/// whose conversation changed where this publication writes; return those
/// write targets for the engine's scope check.
///
/// Revision identity covers the head and full comparison. The targets come
/// from the plan that publication executes: the accepted assessment planned
/// against `snapshot`, the current discovery, with the run's own `progress`,
/// and the retained unresolved write of that progress, which recovery resumes
/// first. When that plan cannot be built, the targets of `preview`, the plan
/// against the captured history, are compared instead, so a change that broke
/// the plan is reported as a conversation change. In either case the engine
/// refuses any write outside the returned targets. The captured and the
/// current history name the targets' threads. Conversation changes count only
/// inside them; ordering and observation time are ignored.
#[review_kit_observation::operations]
#[helper]
pub fn require_fresh(
    gitlab: &GitLab,
    run: &LoadedRun,
    snapshot: &Snapshot,
    chain: &AcceptedReview,
    renderer: &mut Json2MdRenderer,
    progress: &PublicationProgress,
    preview: &Plan,
) -> Result<WriteTargets, CliError> {
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
    // The check is an observed operation: its log record keeps the reason
    // when the plan cannot be built.
    let executing = check_review_with_progress(
        &chain.request,
        snapshot,
        &mut retained_source(&run.dir)?,
        renderer,
        progress,
    )
    .ok();
    let plan = executing.as_ref().unwrap_or(preview);
    let pending = progress.unresolved.as_ref().map(|pending| &pending.write);
    let current = gitlab
        .capture_conversation(subject, BotLookup::Skip)
        .map_err(CliError::GitLab)?;
    let captured = run.context.snapshot.clone().into_snapshot()?;
    let targets = WriteTargets::of(plan, &[&captured, snapshot], pending);
    let changes = current.changes_to(&run.conversation, &targets);
    if !changes.is_empty() {
        return Err(CliError::state(
            "conversation_changed",
            3,
            "a thread that this publication writes to changed since this run captured it; start a new run with rkc run start --new",
        )
        .with_details(json!({"changes": changes.iter().map(change).collect::<Vec<_>>()})));
    }
    Ok(targets)
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
