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

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
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
    fn subtraction_keeps_exact_differences() {
        assert_eq!(
            Units::new(5).checked_sub(Units::new(3)),
            Some(Units::new(2))
        );
        assert_eq!(
            Units::new(4).checked_sub(Units::new(4)),
            Some(Units::new(0))
        );
        assert_eq!(
            Units::new(u32::MAX).checked_sub(Units::new(0)),
            Some(Units::new(u32::MAX))
        );
    }

    #[test]
    fn subtraction_reports_underflow() {
        assert_eq!(Units::new(3).checked_sub(Units::new(4)), None);
        assert_eq!(Units::new(0).checked_sub(Units::new(1)), None);
    }

    #[test]
    fn multiplication_keeps_exact_products() {
        assert_eq!(Units::new(6).checked_mul(7), Some(Units::new(42)));
        assert_eq!(Units::new(u32::MAX).checked_mul(0), Some(Units::new(0)));
        assert_eq!(
            Units::new(u32::MAX).checked_mul(1),
            Some(Units::new(u32::MAX))
        );
        assert_eq!(
            Units::new(u32::MAX / 2).checked_mul(2),
            Some(Units::new(u32::MAX - 1))
        );
    }

    #[test]
    fn multiplication_reports_overflow() {
        assert_eq!(Units::new(u32::MAX / 2 + 1).checked_mul(2), None);
        assert_eq!(Units::new(65_536).checked_mul(65_536), None);
    }
}
