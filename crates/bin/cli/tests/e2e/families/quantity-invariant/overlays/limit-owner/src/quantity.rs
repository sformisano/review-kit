use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Quantity {
    value: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuantityError {
    Zero,
    Invalid,
    TooLarge,
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => f.write_str("quantity must be greater than zero"),
            Self::Invalid => f.write_str("quantity text must contain decimal digits only"),
            Self::TooLarge => f.write_str("quantity must not exceed the limit"),
        }
    }
}

impl std::error::Error for QuantityError {}

const PALLET_THRESHOLD: u32 = 10_000;

impl Quantity {
    pub const LIMIT: u32 = 1_000_000;

    pub fn new(value: u32) -> Result<Self, QuantityError> {
        if value == 0 {
            Err(QuantityError::Zero)
        } else if value > Self::LIMIT {
            Err(QuantityError::TooLarge)
        } else {
            Ok(Self { value })
        }
    }

    pub fn get(self) -> u32 {
        self.value
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        let value = self.value.checked_add(other.value)?;
        Self::new(value).ok()
    }

    pub fn checked_mul(self, factor: u32) -> Option<Self> {
        let value = self.value.checked_mul(factor)?;
        Self::new(value).ok()
    }

    pub fn needs_pallet(self) -> bool {
        self.value > PALLET_THRESHOLD
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantity(value: u32) -> Quantity {
        Quantity::new(value).unwrap()
    }

    #[test]
    fn constructor_enforces_the_range() {
        assert_eq!(Quantity::new(0), Err(QuantityError::Zero));
        assert_eq!(Quantity::new(4).map(Quantity::get), Ok(4));
        assert_eq!(Quantity::new(1_000_000).map(Quantity::get), Ok(1_000_000));
        assert_eq!(Quantity::new(1_000_001), Err(QuantityError::TooLarge));
    }

    #[test]
    fn addition_stays_within_the_limit() {
        let sum = quantity(400_000).checked_add(quantity(500_000));
        assert_eq!(sum.map(Quantity::get), Some(900_000));
        assert_eq!(quantity(999_999).checked_add(quantity(2)), None);
        assert_eq!(quantity(1_000_000).checked_add(quantity(1)), None);
    }

    #[test]
    fn multiplication_stays_within_the_limit() {
        let product = quantity(250_000).checked_mul(4);
        assert_eq!(product.map(Quantity::get), Some(1_000_000));
        assert_eq!(quantity(250_001).checked_mul(4), None);
        assert_eq!(quantity(3).checked_mul(0), None);
    }

    #[test]
    fn quantities_above_the_threshold_need_a_pallet() {
        assert!(!quantity(10_000).needs_pallet());
        assert!(quantity(10_001).needs_pallet());
    }
}
