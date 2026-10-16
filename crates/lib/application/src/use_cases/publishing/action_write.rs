use review_kit_domain::{FindingId, ReviewerId, Revision};

use crate::publication::{ActionId, PlannedAction, Snapshot, Write};

#[review_kit_observation::operations]
#[helper]
pub(super) fn action_write(
    action: &PlannedAction,
    round: u64,
    revision: &Revision,
    snapshot: &Snapshot,
    reviewer: &ReviewerId,
) -> Result<Write, FindingId> {
    let root = |finding| {
        snapshot
            .findings
            .iter()
            .find(|root| &root.author == reviewer && root.finding.id() == finding)
            .ok_or(finding)
    };
    Ok(match action {
        PlannedAction::Finding { finding, body } => Write::Finding {
            round,
            revision: revision.clone(),
            finding: finding.clone(),
            body: body.clone(),
        },
        PlannedAction::Reply {
            finding,
            kind,
            body,
        } => Write::Reply {
            thread: root(*finding)?.id.clone(),
            action: ActionId {
                round,
                finding: *finding,
                kind: kind.clone(),
            },
            body: body.clone(),
        },
        PlannedAction::SetResolved { finding, resolved } => Write::SetResolved {
            thread: root(*finding)?.id.clone(),
            resolved: *resolved,
        },
    })
}
