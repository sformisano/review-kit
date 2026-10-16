# Compact review profile

This complete profile declares `review-kit-render-input/v1` and changes all
five templates. It places policy names above findings, changes summary headings,
and labels transition, answer and suggestion replies. Its summary input has no
policy results; the [input reference](../../../crates/lib/infrastructure/json2md-adapter/README.md#input-contract-versions)
describes the move to v2.

Copy this directory, then select its manifest from a worktree with a selected review run:

```sh
rkc --config config.json --render-profile ./compact/profile.json \
  check
```

Use the same override for publication, or configure `render_profile` once.
The [profile guide](../../../docs/render-profiles.md) covers the full workflow.
The [input reference](../../../crates/lib/infrastructure/json2md-adapter/README.md#rendering-input-contract)
defines the structured template fields.

Schemas and templates remain local files. Publication checks still reject
invalid source ranges, changed suggestions, reserved markers and unsafe quick actions.
