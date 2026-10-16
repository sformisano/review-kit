# Payout Ledger

Payout Ledger keeps partner account balances for payouts.

- `Cents` is an amount in minor currency units.
- `Ledger::deposit` and `Ledger::withdraw` accept positive amounts only.
- A failed operation returns a `LedgerError` and leaves the balance unchanged.
- `Ledger::refund` returns a positive amount to an account.
- `Ledger::audit` exposes the `AuditLog` of balance changes.
- `percentage_fee` computes the fee for a percentage rate.

Run `cargo test`. The crate has no dependencies and its tests need no network.
