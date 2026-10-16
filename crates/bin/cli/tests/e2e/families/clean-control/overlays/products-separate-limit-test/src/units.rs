#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Units(u32);

impl Units {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_mul(self, factor: u32) -> Option<Self> {
        self.0.checked_mul(factor).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addition_keeps_exact_sums() {
        assert_eq!(
            Units::new(2).checked_add(Units::new(3)),
            Some(Units::new(5))
        );
        assert_eq!(
            Units::new(0).checked_add(Units::new(0)),
            Some(Units::new(0))
        );
        assert_eq!(
            Units::new(u32::MAX - 1).checked_add(Units::new(1)),
            Some(Units::new(u32::MAX))
        );
    }

    #[test]
    fn addition_reports_overflow() {
        assert_eq!(Units::new(u32::MAX).checked_add(Units::new(1)), None);
    }

    #[test]
    fn multiplication_scales_counts() {
        assert_eq!(Units::new(6).checked_mul(7), Some(Units::new(42)));
        assert_eq!(Units::new(9).checked_mul(0), Some(Units::new(0)));
        assert_eq!(Units::new(0).checked_mul(5), Some(Units::new(0)));
    }

    #[test]
    fn multiplication_reports_overflow() {
        assert_eq!(Units::new(u32::MAX / 2 + 1).checked_mul(2), None);
        assert_eq!(Units::new(u32::MAX).checked_mul(u32::MAX), None);
    }
}
