# Record a planning or publication run

A publication run combines review planning or publication with timing and
execution evidence. It gives the caller a report even when the operation
fails or saving the final report fails.

Use `run` when the host needs an auditable account of a review invocation.
The caller supplies the `PublishRequest`, requested `RunMode`, and `RunPorts`.
Those ports provide discovery, publication, source checks, rendering, clock,
evidence recording, and caller-retained publication progress.

## Modes and result

`RunMode::Plan` discovers the current snapshot and produces a read-only `Plan`.
It never acquires a publication session. An empty requested effect set forces
this mode even when the caller requests `RunMode::Publish`.

Publish mode delegates to `publishing::publish_recorded`. It retains the same
authority, verification, and unresolved-write safeguards as direct publication.

`run_bound` adds an optional `BindingRequest` that binds publication to one
review run's accepted inputs. `run` passes none; its publication refuses
progress that is already bound to a run. The clock also timestamps a provider-verified settlement
of another run's binding. Plan mode never changes the binding. The
[binding contract](../../../README.md#review-run-binding) defines admission and settlement.

`RunReport` retains the actual mode, start and finish times, submitted assessment,
and either a planned result, publication result, or `PlanError`.
Its `evidence_failure` separately reports failure to save the finished report.
The review decision remains separate from the publication status.

## Example

A host requests publication and records the intended write before dispatch.
The provider effects are verified and publication completes. Saving the final
report then fails. The returned report keeps the completed publication outcome
and exposes the recording failure separately.

## Boundaries

The evidence port receives start, observation, plan, intended-write, readback,
and finish events as the selected path reaches them. A recording failure
before a provider write prevents that write. An initial recording failure
prevents discovery and publication.

`ExecutionEvent::BeforeWrite` provides exact intent for storage before dispatch.
Crash recovery requires the caller to persist it and restore `PublicationProgress`
before another publication call. The caller also serializes publication for
one review and reviewer.
`ExecutionEvent::DiscardedUnsent` names a prepared write that never reached the
provider and is dropped because it can no longer resume.

Read-only mode can still call source, renderer, clock, and evidence ports. Those
adapters can perform I/O. The use case provides no filesystem storage or
raw HTTP evidence; adapters own those details.
