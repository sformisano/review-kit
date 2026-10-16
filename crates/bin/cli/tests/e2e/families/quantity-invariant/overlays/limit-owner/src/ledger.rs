use std::collections::BTreeMap;
use std::fmt;

use crate::Quantity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LedgerError {
    Overflow,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("bin quantity would exceed the quantity limit"),
        }
    }
}

impl std::error::Error for LedgerError {}

#[derive(Debug, Default)]
pub struct Ledger {
    bins: BTreeMap<String, Quantity>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn quantity(&self, bin: &str) -> Option<Quantity> {
        self.bins.get(bin).copied()
    }

    pub fn add(&mut self, bin: &str, amount: Quantity) -> Result<(), LedgerError> {
        let updated = match self.bins.get(bin) {
            Some(current) => current.checked_add(amount).ok_or(LedgerError::Overflow)?,
            None => amount,
        };
        self.bins.insert(bin.to_owned(), updated);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantity(value: u32) -> Quantity {
        Quantity::new(value).unwrap()
    }

    #[test]
    fn adding_accumulates_per_bin() {
        let mut ledger = Ledger::new();
        ledger.add("a1", quantity(2)).unwrap();
        ledger.add("a1", quantity(3)).unwrap();
        ledger.add("b2", quantity(1)).unwrap();
        assert_eq!(ledger.quantity("a1"), Some(quantity(5)));
        assert_eq!(ledger.quantity("b2"), Some(quantity(1)));
        assert_eq!(ledger.quantity("c3"), None);
    }

    #[test]
    fn overflow_leaves_the_bin_unchanged() {
        let mut ledger = Ledger::new();
        ledger.add("a1", quantity(Quantity::LIMIT)).unwrap();
        assert_eq!(ledger.add("a1", quantity(1)), Err(LedgerError::Overflow));
        assert_eq!(ledger.quantity("a1"), Some(quantity(Quantity::LIMIT)));
    }
}
