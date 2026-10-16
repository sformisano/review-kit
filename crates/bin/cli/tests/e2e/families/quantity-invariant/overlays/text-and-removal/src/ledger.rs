use std::collections::BTreeMap;
use std::fmt;

use crate::Quantity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LedgerError {
    Overflow,
    Insufficient,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("bin quantity would overflow"),
            Self::Insufficient => f.write_str("bin holds fewer items than requested"),
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

    pub fn remove(&mut self, bin: &str, amount: Quantity) -> Result<(), LedgerError> {
        let current = self.quantity(bin).ok_or(LedgerError::Insufficient)?;
        if current == amount {
            self.bins.remove(bin);
            return Ok(());
        }
        let remaining = current
            .checked_sub(amount)
            .ok_or(LedgerError::Insufficient)?;
        self.bins.insert(bin.to_owned(), remaining);
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
        ledger.add("a1", quantity(u32::MAX)).unwrap();
        assert_eq!(ledger.add("a1", quantity(1)), Err(LedgerError::Overflow));
        assert_eq!(ledger.quantity("a1"), Some(quantity(u32::MAX)));
    }

    #[test]
    fn removing_part_of_a_bin_keeps_the_rest() {
        let mut ledger = Ledger::new();
        ledger.add("a1", quantity(5)).unwrap();
        ledger.remove("a1", quantity(2)).unwrap();
        assert_eq!(ledger.quantity("a1"), Some(quantity(3)));
    }

    #[test]
    fn removing_every_item_deletes_the_bin() {
        let mut ledger = Ledger::new();
        ledger.add("a1", quantity(5)).unwrap();
        ledger.remove("a1", quantity(5)).unwrap();
        assert_eq!(ledger.quantity("a1"), None);
    }

    #[test]
    fn removing_too_many_items_leaves_the_bin_unchanged() {
        let mut ledger = Ledger::new();
        ledger.add("a1", quantity(2)).unwrap();
        assert_eq!(
            ledger.remove("a1", quantity(3)),
            Err(LedgerError::Insufficient)
        );
        assert_eq!(
            ledger.remove("b2", quantity(1)),
            Err(LedgerError::Insufficient)
        );
        assert_eq!(ledger.quantity("a1"), Some(quantity(2)));
    }
}
