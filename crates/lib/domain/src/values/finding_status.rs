/// The reviewer's judgment about one finding; provider thread state is separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FindingStatus {
    /// The defect remains present; its disposition determines whether it blocks readiness.
    Open,
    /// The reviewer judges that the defect has been corrected.
    Fixed,
    /// The reviewer no longer maintains the finding.
    Withdrawn,
    /// The reviewer cannot establish the finding's state; the assessment must be blocked.
    Uncertain,
}
