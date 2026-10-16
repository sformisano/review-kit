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
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => f.write_str("quantity must be greater than zero"),
            Self::Invalid => f.write_str("quantity text must contain decimal digits only"),
        }
    }
}

impl std::error::Error for QuantityError {}

impl Quantity {
    pub fn new(value: u32) -> Result<Self, QuantityError> {
        if value == 0 {
            Err(QuantityError::Zero)
        } else {
            Ok(Self { value })
        }
    }

    pub fn get(self) -> u32 {
        self.value
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.value
            .checked_add(other.value)
            .map(|value| Self { value })
    }

    pub fn total(quantities: &[Quantity]) -> Option<Self> {
        let (first, rest) = quantities.split_first()?;
        let mut sum = *first;
        for quantity in rest {
            sum = Self::new(sum.value.wrapping_add(quantity.value)).ok()?;
        }
        Some(sum)
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
    fn constructor_rejects_zero() {
        assert_eq!(Quantity::new(0), Err(QuantityError::Zero));
        assert_eq!(Quantity::new(4).unwrap().get(), 4);
    }

    #[test]
    fn addition_detects_overflow() {
        let large = Quantity::new(u32::MAX).unwrap();
        assert_eq!(large.checked_add(Quantity::new(1).unwrap()), None);
        let sum = Quantity::new(2)
            .unwrap()
            .checked_add(Quantity::new(3).unwrap());
        assert_eq!(sum.map(Quantity::get), Some(5));
    }

    #[test]
    fn total_adds_quantities() {
        let total = Quantity::total(&[quantity(2), quantity(3), quantity(4)]);
        assert_eq!(total.map(Quantity::get), Some(9));
        assert_eq!(Quantity::total(&[quantity(7)]), Some(quantity(7)));
    }

    #[test]
    fn total_of_no_quantities_is_none() {
        assert_eq!(Quantity::total(&[]), None);
    }

    #[test]
    fn total_reports_overflow() {
        assert_eq!(Quantity::total(&[quantity(u32::MAX), quantity(1)]), None);
    }
}
