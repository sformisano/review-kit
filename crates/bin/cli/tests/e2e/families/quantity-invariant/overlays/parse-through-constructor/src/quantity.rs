use std::fmt;
use std::str::FromStr;

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

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.value
            .checked_sub(other.value)
            .map(|value| Self { value })
    }
}

impl FromStr for Quantity {
    type Err = QuantityError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(QuantityError::Invalid);
        }
        let value = text.parse::<u32>().map_err(|_| QuantityError::Invalid)?;
        Self::new(value)
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
    fn subtraction_keeps_the_difference() {
        let five = Quantity::new(5).unwrap();
        let three = Quantity::new(3).unwrap();
        assert_eq!(five.checked_sub(three).map(Quantity::get), Some(2));
        assert_eq!(three.checked_sub(five), None);
    }

    #[test]
    fn text_contains_decimal_digits_only() {
        assert_eq!("12".parse::<Quantity>().map(Quantity::get), Ok(12));
        for text in ["", "+3", "-3", " 3", "1x", "99999999999"] {
            assert_eq!(
                text.parse::<Quantity>(),
                Err(QuantityError::Invalid),
                "{text:?}"
            );
        }
    }

    #[test]
    fn zero_text_is_rejected() {
        assert_eq!("0".parse::<Quantity>(), Err(QuantityError::Zero));
        assert_eq!("000".parse::<Quantity>(), Err(QuantityError::Zero));
    }
}
