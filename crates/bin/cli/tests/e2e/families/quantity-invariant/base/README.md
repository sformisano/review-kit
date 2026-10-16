# Quantity Ledger

Quantity Ledger records how many items each warehouse bin holds.

## Contract

- A `Quantity` always holds a value greater than zero. Every public way to
  obtain a `Quantity` keeps this guarantee: construction, text parsing and
  arithmetic return an error or `None` instead of producing zero.
- `Quantity` arithmetic never wraps around and never panics.
- Quantity text contains ASCII decimal digits only, without a sign, spaces or
  separators.
- A `Ledger` holds at most one quantity per bin. A bin that becomes empty has
  no entry; the ledger never stores a zero quantity.
- A failed ledger operation leaves every bin unchanged.

## Checks

Run `cargo test`. The crate has no dependencies and its tests need no network.
