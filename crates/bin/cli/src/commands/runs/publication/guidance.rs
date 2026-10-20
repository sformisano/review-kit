//! What a run whose publication stopped unfinished can do next.

use review_kit_application::{
    publication::{Effect, RoundStatus, Snapshot},
    review_run::RunId,
    use_cases::publishing::{PlanError, PublicationProgress, ResendBlocked},
};
use review_kit_domain::{ReviewerId, Revision};
use serde_json::{Value, json};

/// The number of the reviewer's pending or partial round on the provider.
#[review_kit_observation::operations]
#[helper]
pub fn unfinished_round(snapshot: &Snapshot, reviewer: &ReviewerId) -> Option<u64> {
    snapshot
        .summaries
        .iter()
        .find(|summary| &summary.author == reviewer)
        .filter(|summary| {
            matches!(
                summary.current.status,
                RoundStatus::Pending | RoundStatus::Partial
            )
        })
        .map(|summary| summary.current.number)
}

/// How to move on from a round that this run can no longer complete.
#[review_kit_observation::operations]
#[helper]
pub fn close_round(round: Option<u64>) -> String {
    match round {
        Some(round) => format!(
            "Close round {round} with rkc publish --abandon --reason TEXT once no write is uncertain, then start a fresh run with rkc run start --new."
        ),
        None => "Start a fresh run with rkc run start --new.".to_owned(),
    }
}

/// The top-level diagnostic of a publication stopped by a new head or base.
#[review_kit_observation::operations]
#[helper]
pub fn changed_revision(round: Option<u64>) -> Value {
    json!({
        "code": "changed_revision",
        "message": "the merge request revision changed after this run's capture, so publication stopped before completing its round",
        "next": format!(
            "This run cannot complete its publication on the new revision. {}",
            close_round(round)
        ),
    })
}

/// The engine refused a round that would write to an existing summary note
/// or finding thread outside the targets that the freshness preflight
/// compared; it stopped before recording any write intent.
#[review_kit_observation::operations]
#[helper]
pub fn uncompared_write() -> Value {
    json!({
        "code": "uncompared_write",
        "message": "the publication would write to an existing summary note or finding thread that the freshness preflight did not compare, so it stopped before any write",
        "next": "The reviewer's history on the merge request changed after the freshness preflight. Start a fresh run with rkc run start --new.",
    })
}

/// Whether a refused recovery comes from the run's frozen inputs, which no
/// retry can change: the current provider state no longer admits them.
///
/// Provider reads, evidence storage, rendering, profile, source checks and
/// journal binding refusals can clear on retry or carry their own guidance.
#[review_kit_observation::operations]
#[helper]
pub fn stale_inputs(error: &PlanError) -> bool {
    match error {
        PlanError::Discovery(_)
        | PlanError::Evidence(_)
        | PlanError::Render(_)
        | PlanError::Binding(_)
        | PlanError::WrongReviewer
        | PlanError::ProgressBinding
        | PlanError::ChangedRenderProfile
        | PlanError::UnknownPendingRenderProfile
        | PlanError::Checks(_) => false,
        PlanError::ShapeChecks(errors) | PlanError::ReplyChecks(errors) => {
            errors.iter().all(stale_inputs)
        }
        _ => true,
    }
}

/// A creation that may have reached GitLab, is not observed there, and that
/// recovery no longer resends.
///
/// Recovery resends such a creation at most once and only on the revision it
/// was prepared for. Until GitLab shows the write, the round can neither
/// complete nor be abandoned, and the reviewer's other runs cannot publish.
pub struct StrandedCreation {
    effect: Effect,
    /// The head the write was prepared for, when only a changed revision
    /// prevents its one resend.
    reviewed_head: Option<String>,
}

/// The journal's uncertain write when recovery can never resend it at `current`.
///
/// The application owns the resend rule; this only shapes its answer for guidance.
#[review_kit_observation::operations]
#[helper]
pub fn stranded_creation(
    progress: &PublicationProgress,
    current: &Revision,
) -> Option<StrandedCreation> {
    let (pending, blocked) = progress.creation_resend_blocked(current)?;
    Some(StrandedCreation {
        effect: pending.write.effect(),
        reviewed_head: match blocked {
            ResendBlocked::RetryConsumed => None,
            ResendBlocked::RevisionChanged { reviewed } => Some(reviewed.head().to_owned()),
        },
    })
}

/// The documented ways forward from a stranded creation of `owner`, or of
/// the selected run when `owner` is `None`.
#[review_kit_observation::operations]
#[helper]
pub fn stranded_guidance(stranded: &StrandedCreation, owner: Option<&RunId>) -> String {
    let write = match stranded.effect {
        Effect::CreateSummary => "summary creation",
        Effect::Finding => "finding creation",
        _ => "reply creation",
    };
    let (whose, recover, after) = match owner {
        Some(owner) => (
            format!("Run {owner} owns the unfinished publication. Its"),
            format!(
                "recover run {owner} with rkc run select {owner} and rkc publish with its original effects"
            ),
            format!(
                " After run {owner} settles, select this run and publish it, or start a fresh run with rkc run start --new when the merge request changed."
            ),
        ),
        None => (
            "This run's".to_owned(),
            "run rkc publish with the original effects".to_owned(),
            String::new(),
        ),
    };
    let (cause, restore) = match &stranded.reviewed_head {
        Some(head) => (
            format!("the merge request no longer has the reviewed revision, head {head}"),
            format!(
                "restore the reviewed revision, head {head}, on the merge request so that recovery can resend the write once, then {recover}; "
            ),
        ),
        None => ("its one resend was already used".to_owned(), String::new()),
    };
    format!(
        "{whose} {write} may have reached GitLab but has not been observed there, and it cannot be resent because {cause}. The write stays uncertain: until GitLab shows it, neither recovery nor rkc publish --abandon can settle the round, and no other run of this reviewer can publish on this merge request. Options: {restore}wait until a late arrival of the write is visible on GitLab, then {recover} to reconcile it; or review with a different reviewer identity.{after} The rkc guide describes this limit in crates/bin/cli/README.md#uncertain-writes."
    )
}
