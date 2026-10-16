use std::collections::HashSet;

use review_kit_domain::{Disposition, Finding, FindingId, FindingStatus, ReviewerId};

use crate::publication::Snapshot;

/// An independent checker's verdict on one finding version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationResult {
    Confirmed,
    Rejected,
    EvidenceGap,
}

/// One recorded verdict in the current assessment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    /// An isolated checker examined exactly the `checked` version.
    Checked {
        result: ConfirmationResult,
        checked: Box<Finding>,
        basis: String,
    },
    /// The agent reuses the recorded confirmation of the unchanged captured version.
    Retained { finding: FindingId, basis: String },
}

#[review_kit_observation::operations]
impl Confirmation {
    /// The finding this verdict concerns.
    #[helper]
    pub fn finding(&self) -> FindingId {
        match self {
            Self::Checked { checked, .. } => checked.id(),
            Self::Retained { finding, .. } => *finding,
        }
    }
    #[helper]
    fn basis(&self) -> &str {
        match self {
            Self::Checked { basis, .. } | Self::Retained { basis, .. } => basis,
        }
    }
}

/// A confirmation that cannot support the current assessment.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display)]
pub enum ConfirmationIssue {
    /// confirmation names finding {0}, which this assessment does not contain
    UnknownFinding(FindingId),
    /// finding {0} has more than one confirmation
    DuplicateConfirmation(FindingId),
    /// confirmation of finding {0} needs a nonblank basis
    BlankBasis(FindingId),
    /// confirmation of finding {0} checked a different version than the final finding
    StaleConfirmation(FindingId),
    /// retained confirmation of finding {0} needs an unchanged captured version with a reusable confirmation
    InvalidRetainedConfirmation(FindingId),
    /// open or uncertain blocking finding {0} claims validation without a matching confirmation
    UnconfirmedBlockingFinding(FindingId),
}

/// Captured findings whose recorded confirmation a retained entry can reuse.
///
/// A captured open or uncertain blocking finding qualifies when one of the
/// reviewer's rounds that was accepted under confirmation enforcement reviewed the
/// snapshot's revision and validated exactly the captured content. Validation from
/// any other round, or for another revision, records no reusable confirmation.
#[review_kit_observation::operations]
#[helper]
pub fn reusable_confirmations(
    snapshot: &Snapshot,
    reviewer: &ReviewerId,
    captured: &[Finding],
) -> Vec<Finding> {
    let rounds: Vec<_> = snapshot
        .summaries
        .iter()
        .filter(|summary| &summary.author == reviewer)
        .flat_map(|summary| {
            summary
                .history
                .iter()
                .chain(std::iter::once(&summary.current))
        })
        .filter(|round| {
            round.confirmed_validation && round.assessment.revision() == &snapshot.revision
        })
        .collect();
    captured
        .iter()
        .filter(|finding| requires_confirmation(finding) && finding.validated())
        .filter(|finding| {
            rounds.iter().any(|round| {
                round.assessment.findings().iter().any(|recorded| {
                    recorded.id() == finding.id()
                        && recorded.validated()
                        && recorded.same_content(finding)
                })
            })
        })
        .cloned()
        .collect()
}

/// Apply recorded confirmations to the final findings.
///
/// An open or uncertain blocking finding is validated exactly when a confirmed
/// verdict covers its final content: a checker examined that exact version, or
/// the finding is unchanged from a captured version in `reusable`, whose recorded
/// confirmation is reused. Such a finding cannot claim validation otherwise. Other
/// findings keep their authored validation. Each finding has at most one verdict
/// with a basis. A confirmed verdict must name a final finding and, for a finding
/// that requires confirmation, cover its exact version; rejections and evidence
/// gaps are records that admit nothing.
#[review_kit_observation::operations]
#[operation(
    name = "application.checkpoints.confirmations",
    input = crate::observation::Fields::new().count("findings", findings.len()).count("confirmations", confirmations.len()),
    output = |result| crate::observation::Completion::result(result, |_| crate::observation::Fields::new(), |issues| crate::observation::ErrorInfo::new("ConfirmationIssues", "validation_failed").with_fields(crate::observation::Fields::new().count("count", issues.len()))),
)]
pub fn admit_findings(
    findings: Vec<Finding>,
    reusable: &[Finding],
    confirmations: &[Confirmation],
) -> Result<Vec<Finding>, Vec<ConfirmationIssue>> {
    let mut issues = Vec::new();
    let mut seen = HashSet::new();
    let mut confirmed = HashSet::new();
    for confirmation in confirmations {
        let id = confirmation.finding();
        if !seen.insert(id) {
            issues.push(ConfirmationIssue::DuplicateConfirmation(id));
            continue;
        }
        if confirmation.basis().trim().is_empty() {
            issues.push(ConfirmationIssue::BlankBasis(id));
        }
        let current = findings.iter().find(|finding| finding.id() == id);
        match confirmation {
            Confirmation::Checked {
                result: ConfirmationResult::Confirmed,
                checked,
                ..
            } => match current {
                None => issues.push(ConfirmationIssue::UnknownFinding(id)),
                Some(current)
                    if requires_confirmation(current) && !checked.same_content(current) =>
                {
                    issues.push(ConfirmationIssue::StaleConfirmation(id));
                }
                Some(_) => {
                    confirmed.insert(id);
                }
            },
            Confirmation::Checked { .. } => {}
            Confirmation::Retained { .. } => {
                let covered = current.is_some_and(|current| {
                    reusable
                        .iter()
                        .any(|captured| captured.id() == id && captured.same_content(current))
                });
                if covered {
                    confirmed.insert(id);
                } else {
                    issues.push(ConfirmationIssue::InvalidRetainedConfirmation(id));
                }
            }
        }
    }
    let admitted = findings
        .into_iter()
        .map(|finding| {
            if !requires_confirmation(&finding) {
                return finding;
            }
            let covered = confirmed.contains(&finding.id());
            if finding.validated() && !covered {
                issues.push(ConfirmationIssue::UnconfirmedBlockingFinding(finding.id()));
            }
            finding.with_validated(covered)
        })
        .collect();
    if issues.is_empty() {
        Ok(admitted)
    } else {
        Err(issues)
    }
}

/// Whether a finding is admitted only through a confirmation: open and uncertain blocking findings.
#[review_kit_observation::operations]
#[helper]
pub fn requires_confirmation(finding: &Finding) -> bool {
    finding.disposition() == Disposition::Blocking
        && matches!(
            finding.status(),
            FindingStatus::Open | FindingStatus::Uncertain
        )
}
