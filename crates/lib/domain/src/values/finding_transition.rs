use crate::FindingStatus;

/// A local status change and its explanation, returned by [`crate::Finding::transition`].
/// It carries no finding identity and does not prove that a reply was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingTransition {
    from: FindingStatus,
    to: FindingStatus,
    evidence: String,
}

impl FindingTransition {
    /// The caller checks that statuses differ and the explanation is nonblank.
    pub(crate) fn new(from: FindingStatus, to: FindingStatus, evidence: String) -> Self {
        Self { from, to, evidence }
    }

    pub fn from(&self) -> FindingStatus {
        self.from
    }
    pub fn to(&self) -> FindingStatus {
        self.to
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
    /// Only reopening a fixed or withdrawn finding counts as recurrence.
    pub fn is_recurrence(&self) -> bool {
        matches!(self.from, FindingStatus::Fixed | FindingStatus::Withdrawn)
            && self.to == FindingStatus::Open
    }
}
