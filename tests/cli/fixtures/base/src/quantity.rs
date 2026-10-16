use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Quantity {
    value: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantityError;

impl std::fmt::Display for QuantityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("quantity must be positive")
    }
}

impl std::error::Error for QuantityError {}

impl Quantity {
    pub fn new(value: u32) -> Result<Self, QuantityError> {
        if value == 0 {
            Err(QuantityError)
        } else {
            Ok(Self { value })
        }
    }

    pub fn get(self) -> u32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_and_serialization_keep_positive_values() {
        assert!(Quantity::new(0).is_err());
        let quantity = Quantity::new(3).unwrap();
        assert_eq!(quantity.get(), 3);
        assert_eq!(serde_json::to_value(quantity).unwrap(), serde_json::json!({"value": 3}));
    }
}
