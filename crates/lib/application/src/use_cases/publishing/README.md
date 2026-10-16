# Communicate and recover a review

Publishing turns a validated agent assessment into verified provider communication.
The same path handles an initial review, a follow-up, and recovery of a partial round.
The agent supplies the decision; publication reports whether communication succeeded.

## Prepare the request

`PublishRequest` combines the assessment, assessed-slot `AssessmentPolicies`, full public policy declarations, requested effects,
authored answers, and explicit `TransitionReply` explanations.
An answer targets the observed question. A transition names the previous and desired `FindingStatus`.
Status changes and same-state reclassifications require an explanation. Finding evidence cannot substitute for it.
The [declaration contract](../../../README.md#public-policy-declarations) binds inventory names, applicability and public reasons to assessed slots.
Private `local_context` never enters rounds, rendered bodies, or provider writes.

The caller serializes publication for one subject and reviewer.
It carries the same `PublicationProgress` across publication and abandonment calls.
Default progress asserts that no earlier unresolved attempt exists.
Durable recovery requires retaining exact intent before dispatch and restoring
progress before the next call.

## Check and plan

`check_review` collects independent coverage, shape, source, reply, and rendering
failures against a retained `Snapshot`. An invalid snapshot stops further checks.
Success returns a `Plan`. `plan_from_snapshot` performs publication preflight
against retained observations; `plan` first discovers current observations.

These operations perform no provider writes and acquire no publication session.
Source and renderer ports can perform I/O. A plan contains exact available bodies
and identifies finding references that remain unresolved until creation is verified.
A successful plan does not guarantee current authority or successful publication.

## Publish and verify

`publish` acquires a host-bound session, discovers owned state, and performs preflight.
It verifies a pending summary before finding effects, reads back each effect,
and completes the round only after the required effects are verified.
`publish_recorded` adds observation, plan, intent, and readback evidence recording.

Each write needs the requested effect, host authority, and current provider permission.
Checks refresh revision, relevant targets, ownership, and authority before effects.
An unchanged completed assessment with identical public declarations creates no round and sends no writes.
A blocked assessment can communicate individually validated findings with determinate status and a completed evaluation of their own selected policy.
Closed historical findings retain their existing policy and transition checks.
Unvalidated or uncertain candidates generate no finding effects; all inputs still require consistency validation.
The initial summary stays pending or partial while required effects remain.
After verification, the final summary is blocked and retains its unresolved evaluations.

`PublicationResult` keeps decision, communication status, attempted writes, verified
effects, skipped actions, final observations, and stop reasons separate.
Completed publication can communicate `ChangesRequired`. A fixed finding judgment
and a provider thread's resolved flag remain separate states.

## Recovery and abandonment

A partial round retains its exact assessment, public declarations, replies, targets, and rendered actions.
Recovery reuses verified effects and repairs missing effects without replacing that intent.
Recovery distinguishes prepared requests from possible dispatch and reuses exact observed effects.
A prepared request that can no longer resume never reached the provider; it is discarded with a recorded event and publication continues from the current plan.
The [application recovery contract](../../../README.md#publication-and-recovery)
defines observed settlement, original authority ceilings, the one-retry creation budget and conservative recovery.

`abandon` closes the exact pending or partial round with a nonblank public reason.
`abandon_recorded` also records observations, intent, and readback.
Only the owned summary changes, under `UpdateSummary` authority.
Finding IDs stay reserved; published content remains; missing actions are not replayed.
Abandonment refuses unknown write outcomes. Its exact prepared summary can resume under the [recovery contract](../../../README.md#publication-and-recovery); other prepared intent is discarded unsent.

For example, discovery of the exact verified root settles a lost create response without another POST.

## Review-run binding

A durable journal can bind publication to one review run's accepted inputs with a `BindingRequest`.
`abandon_bound` and `publication_run::run_bound` admit the request before any write and install the binding with the first prepared write.
The dispatch checkpoint marks that a write of the binding may have reached the provider.
Another run's unfinished publication refuses the request with `BindingError::PendingOwner`.
Another run's binding that never reached the provider is released instead, restoring the grant that preceded it.
After the call, `PublicationProgress::settle_publication` records the run's verified outcome when the provider shows its round finished, and `run_publication` reports it.
The [application binding contract](../../../README.md#review-run-binding) defines admission, rollback and settlement.

## Limits

Preflight failures return `PlanError`; execution failures remain in `PublicationResult`.
Earlier verified effects survive later denials or failures.
Publication is not atomic. Fresh observations cannot exclude concurrent edits.
No operation approves, merges, deletes, edits finding roots, or applies suggestions.
