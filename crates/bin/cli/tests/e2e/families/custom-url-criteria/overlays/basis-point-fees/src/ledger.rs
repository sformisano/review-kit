use std::collections::BTreeMap;
use std::fmt;

use crate::{AuditLog, Cents};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LedgerError {
    InvalidAmount,
    InsufficientFunds,
    Overflow,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount => f.write_str("amount must be positive"),
            Self::InsufficientFunds => f.write_str("balance is too low"),
            Self::Overflow => f.write_str("balance would overflow"),
        }
    }
}

impl std::error::Error for LedgerError {}

#[derive(Debug, Default)]
pub struct Ledger {
    balances: BTreeMap<String, Cents>,
    audit: AuditLog,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn balance(&self, account: &str) -> Cents {
        self.balances.get(account).copied().unwrap_or_default()
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn deposit(&mut self, account: &str, amount: Cents) -> Result<(), LedgerError> {
        if amount.0 <= 0 {
            return Err(LedgerError::InvalidAmount);
        }
        let updated = self
            .balance(account)
            .checked_add(amount)
            .ok_or(LedgerError::Overflow)?;
        self.balances.insert(account.to_owned(), updated);
        self.audit.record(account, amount.0);
        Ok(())
    }

    pub fn withdraw(&mut self, account: &str, amount: Cents) -> Result<(), LedgerError> {
        if amount.0 <= 0 {
            return Err(LedgerError::InvalidAmount);
        }
        let current = self.balance(account);
        if current < amount {
            return Err(LedgerError::InsufficientFunds);
        }
        let updated = current.checked_sub(amount).ok_or(LedgerError::Overflow)?;
        self.balances.insert(account.to_owned(), updated);
        self.audit.record(account, -amount.0);
        Ok(())
    }

    pub fn refund(&mut self, account: &str, amount: Cents) -> Result<(), LedgerError> {
        self.deposit(account, amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposits_and_withdrawals_change_the_balance() {
        let mut ledger = Ledger::new();
        ledger.deposit("p-1", Cents(1_000)).unwrap();
        ledger.withdraw("p-1", Cents(250)).unwrap();
        assert_eq!(ledger.balance("p-1"), Cents(750));
        assert_eq!(ledger.audit().entries().len(), 2);
    }

    #[test]
    fn failed_operations_leave_the_balance_unchanged() {
        let mut ledger = Ledger::new();
        ledger.deposit("p-1", Cents(100)).unwrap();
        assert_eq!(
            ledger.withdraw("p-1", Cents(101)),
            Err(LedgerError::InsufficientFunds)
        );
        assert_eq!(
            ledger.deposit("p-1", Cents(0)),
            Err(LedgerError::InvalidAmount)
        );
        assert_eq!(
            ledger.deposit("p-1", Cents(i64::MAX)),
            Err(LedgerError::Overflow)
        );
        assert_eq!(ledger.balance("p-1"), Cents(100));
        assert_eq!(ledger.audit().entries().len(), 1);
    }

    #[test]
    fn refunds_return_money_to_the_account() {
        let mut ledger = Ledger::new();
        ledger.deposit("p-1", Cents(500)).unwrap();
        ledger.withdraw("p-1", Cents(200)).unwrap();
        ledger.refund("p-1", Cents(50)).unwrap();
        assert_eq!(ledger.balance("p-1"), Cents(350));
        assert_eq!(
            ledger.refund("p-1", Cents(-5)),
            Err(LedgerError::InvalidAmount)
        );
        assert_eq!(ledger.balance("p-1"), Cents(350));
        assert_eq!(
            ledger.audit().entries().last().map(|entry| entry.change),
            Some(50)
        );
        assert_eq!(ledger.audit().entries().len(), 3);
    }
}
