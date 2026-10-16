/// The agent's severity classification for a concern, from the highest tier P1
/// to the lowest P5.
///
/// [`crate::Disposition`] separately records whether an open finding blocks
/// readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    P1,
    P2,
    P3,
    P4,
    P5,
}
