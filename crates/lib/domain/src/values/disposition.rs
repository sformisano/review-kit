/// Whether an open finding requires correction before the review can be ready.
///
/// This judgment is independent of [`crate::Severity`]. It does not establish
/// a policy result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Disposition {
    /// While the finding is open, correction is required before readiness.
    Blocking,
    /// The concern does not itself prevent readiness.
    Advisory,
}
