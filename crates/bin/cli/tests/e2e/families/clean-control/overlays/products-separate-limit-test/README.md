# Unit Counter

Unit Counter keeps counts of packaged units.

## Contract

- `Units` holds any `u32` value, including zero.
- Arithmetic on `Units` never wraps around and never panics. Each checked
  operation returns `None` when the exact result does not fit in a `u32`.
- Available operations: `checked_add` and `checked_mul`. `checked_mul`
  multiplies by a plain `u32` factor.

## Checks

Run `cargo test`. The crate has no dependencies and its tests need no network.
