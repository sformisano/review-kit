use std::num::NonZeroU64;

/// A positive finding number, local to one review subject and reviewer.
///
/// Equality compares only the local number. The enclosing review must retain
/// its subject and reviewer; this value does not establish global identity.
/// Every number from one through [`u64::MAX`] is valid, with numeric ordering.
/// Display writes the decimal number without a prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, displaydoc::Display)]
#[displaydoc("{0}")]
pub struct FindingId(NonZeroU64);

impl FindingId {
    /// The numeric identity, without a presentation prefix such as `F`.
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Advances the local number without reserving it in a review.
    ///
    /// When allocating, the caller supplies the current maximum from the
    /// review's known history.
    ///
    /// # Errors
    ///
    /// Returns [`FindingIdExhausted`] at [`u64::MAX`]; numbering never wraps.
    pub fn next(self) -> Result<Self, FindingIdExhausted> {
        let next = self.get().checked_add(1).ok_or(FindingIdExhausted)?;
        NonZeroU64::new(next).map(Self).ok_or(FindingIdExhausted)
    }
}

/// No positive `u64` finding number follows the current number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("no finding number remains after the current maximum")]
pub struct FindingIdExhausted;

impl TryFrom<u64> for FindingId {
    type Error = InvalidFindingId;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(InvalidFindingId)
    }
}

/// Finding numbering starts at one; zero cannot identify a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("finding ID must be greater than zero")]
pub struct InvalidFindingId;

#[cfg(test)]
mod tests {
    use super::{FindingId, FindingIdExhausted, InvalidFindingId};

    #[test]
    fn zero_cannot_identify_a_finding() {
        assert_eq!(FindingId::try_from(0), Err(InvalidFindingId));
    }

    #[test]
    fn positive_boundaries_preserve_the_number() -> Result<(), InvalidFindingId> {
        for number in [1, u64::MAX] {
            assert_eq!(FindingId::try_from(number)?.get(), number);
        }
        Ok(())
    }

    #[test]
    fn finding_order_is_numeric() -> Result<(), InvalidFindingId> {
        let mut findings = [
            FindingId::try_from(10)?,
            FindingId::try_from(2)?,
            FindingId::try_from(1)?,
        ];
        findings.sort();
        assert_eq!(findings.map(FindingId::get), [1, 2, 10]);
        Ok(())
    }

    #[test]
    fn advancing_an_identity_never_wraps() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(FindingId::try_from(1)?.next()?.get(), 2);
        assert_eq!(
            FindingId::try_from(u64::MAX)?.next(),
            Err(FindingIdExhausted)
        );
        Ok(())
    }
}
