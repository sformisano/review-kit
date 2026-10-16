# Retry Schedule

Retry Schedule computes how long a client waits before each retry of a failed
request.

## Contract

- `Backoff::new(initial_ms, limit_ms)` requires an initial delay greater than
  zero and a limit greater than the initial delay. It returns
  `BackoffError::ZeroInitial` or `BackoffError::LimitTooSmall` otherwise.
- `Backoff::delay_ms(attempt)` returns the initial delay for attempt 0 and
  doubles it for each later attempt, up to the limit. It never panics, even for
  very large attempt numbers.

## Example

```rust
use retry_schedule::Backoff;

let backoff = Backoff::new(100, 1_000)?;
assert_eq!(backoff.delay_ms(0), 100);
assert_eq!(backoff.delay_ms(3), 800);
assert_eq!(backoff.delay_ms(10), 1_000);
```

Each retry waits for the delay of its attempt number, so a client that receives
three failures in a row waits 100, 200 and 400 milliseconds before its retries.

## Checks

Run `cargo test`. The crate has no dependencies and its tests need no network.
