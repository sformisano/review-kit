use std::num::NonZeroU64;

/// A one-based source line number.
///
/// Every number from one through [`u64::MAX`] is valid.
/// Construction does not prove that the line exists in a file or appears in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineNumber(NonZeroU64);

impl LineNumber {
    /// The one-based coordinate; no conversion to a zero-based index is applied.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for LineNumber {
    type Error = InvalidLineNumber;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(InvalidLineNumber)
    }
}

/// Zero is not a one-based source coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
#[displaydoc("source line number must be greater than zero")]
pub struct InvalidLineNumber;

#[cfg(test)]
mod tests {
    use super::{InvalidLineNumber, LineNumber};

    #[test]
    fn zero_is_not_a_source_line() {
        assert_eq!(LineNumber::try_from(0), Err(InvalidLineNumber));
    }

    #[test]
    fn positive_boundaries_remain_one_based() -> Result<(), InvalidLineNumber> {
        for number in [1, u64::MAX] {
            assert_eq!(LineNumber::try_from(number)?.get(), number);
        }
        Ok(())
    }
}
