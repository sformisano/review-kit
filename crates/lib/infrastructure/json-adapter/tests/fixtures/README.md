# Retained assessment fixtures

`assessment-v1.json` is a version 1 assessment draft for
`rust-codebase-policies` with its two mandatory policies and no optional policies.
`context-v1.json` is its matching context, supplied through `latest.json` as `context.json`.

The assessment has no display names or judgments and retains `requested_policies`.
The compatibility test fills the review results, then completes the draft without
loading definitions. Keep these bytes independent of the current writer.

# Run-bound journal fixtures

`progress-v3-bootstrap.json`, `progress-v3-interrupted.json` and
`progress-v3-settled.json` are authored version 3 journals for an empty first-use
store, a finding write that may have been sent, and a run settled as published.
They model document shape. The interrupted journal has no baseline observation,
so the domain decoder refuses to restore it for recovery.
