# review-kit-application

This crate takes the agent's review from an unfinished draft to checked, verified communication.
It coordinates initial reviews, follow-ups, and interrupted publication.
The agent supplies the judgments; the application checks the record and carries out permitted effects.

Start with the [domain product model](../domain/README.md) for policies, findings, assessments, and review decisions.
[Follow one review](../../../docs/guide.md) gives a complete example; the
[CLI guide](../../bin/cli/README.md) explains how to run it.

## Follow a review

A reviewer finds an ownership-check defect in a change and records it as finding `F1`.
The agent concludes that changes are required.
The application checks the assessment, prepares the comments, and publishes them through the provider adapter.
Successful publication means the intended review was communicated and verified.
The change still requires a fix.

After a new revision, discovery supplies the existing finding and its discussion.
The agent reassesses `F1`, records it as fixed, and supplies a transition explanation.
Publication can reply in the existing discussion and resolve it when authorized.
The finding keeps its identity across the follow-up.

The review decision and the communication outcome answer different questions.
A ready assessment can fail to publish; a changes-required or blocked assessment can be communicated successfully.
An interrupted attempt retains the effects already verified and the intent needed to account for unresolved writes.
The [publication contract](#publication-and-recovery) defines how recovery and abandonment work.

## Choose a use case

Each guide describes the product need, the resulting information, an example, and the operation's limits.
The same guide appears in its module's generated Rust documentation.

| Product need | Local guide |
| --- | --- |
| Observe the provider's review records and reviewer | [Discovery](src/use_cases/discovery/README.md) |
| Gather the diff, existing findings, and questions for reassessment | [Discovery bundle](src/use_cases/discovery_bundle/README.md) |
| Fill a draft with fresh judgments and validate the assessment | [Assessment](src/use_cases/assessment/README.md) |
| Check a review run's combined policy declaration and assessment | [Checkpoints](src/use_cases/checkpoints/README.md) |
| Create a review draft while preserving an existing destination | [Initialization](src/use_cases/initialization/README.md) |
| Check, preview, publish, recover, or abandon review communication | [Publishing](src/use_cases/publishing/README.md) |
| Plan or publish with a timed report and recorded evidence | [Publication run](src/use_cases/publication_run/README.md) |

## Layer boundary

The application depends on `review-kit-domain` and coordinates external work through adapter ports.
Error derives follow the [inner-layer dependency choices](../../../docs/decisions.md#inner-layer-choices).
It contains no HTTP client, filesystem implementation, async execution runtime,
database, Markdown renderer, or provider-specific serialization.

## Use cases and command coverage

Each use case lives in a directory with `mod.rs` as its entry file.
Keep its helpers and private submodules in that directory.
The [`use_cases` module](src/use_cases/mod.rs) declares the public use cases.

The [CLI](../../bin/cli/README.md#review-runs) implements the command names below.
Its adapters supply concrete input, storage, rendering and provider operations.
Tests exercise Rust contracts through fake implementations of the public ports.
The [GitLab adapter's HTTP tests](../infrastructure/gitlab-adapter/tests/http_workflow.rs) also exercise discovery, publication and run orchestration with real request mapping.

| Command behavior | Core API | Current verification | Integration |
| --- | --- | --- | --- |
| `run start` | `discovery_bundle::discover_bundle`; the GitLab adapter also captures source and conversation | [Discovery tests](tests/discovery_initialization.rs): subject, reviewer, revision, files, diff, findings, replies, next ID | [GitLab source export](../infrastructure/gitlab-adapter/README.md#retained-source-for-offline-checking), [run storage](../infrastructure/filesystem-adapter/README.md#review-runs) |
| `check`, `accept` | `checkpoints::validate_declarations`, `checkpoints::reusable_confirmations`, `checkpoints::admit_findings`; [`publishing::check_review_with_progress`](src/use_cases/publishing/checking.rs) | [Checkpoint tests](tests/checkpoints.rs), [confirmation tests](tests/confirmations.rs), [assessment tests](tests/assessment.rs), [offline review tests](tests/review_workflow.rs): combined policies, run/revision/successor, source, bodies and transitions | [Assessment document](../infrastructure/json-adapter/README.md#review-run-documents), [run storage](../infrastructure/filesystem-adapter/README.md#review-runs) |
| `publish --plan`, `--effects none` | `publication_run::run_bound` with `RunMode::Plan` | [Run tests](tests/publication_run.rs): no publication session or writes; known bodies and unresolved references | [CLI publication modes](../../bin/cli/README.md#publication-and-recovery), [file evidence](../infrastructure/filesystem-adapter/README.md) |
| `publish` | `publication_run::run_bound` with a `BindingRequest` and `RunMode::Publish`; `publishing` engine | [Workflow tests](tests/review_workflow.rs): ownership, authority, ordering, readbacks, recovery, history, unchanged review, run binding and settlement | [GitLab adapter](../infrastructure/gitlab-adapter/README.md), [run-bound journal](../infrastructure/filesystem-adapter/README.md#run-bound-journal) |
| `publish --abandon` | `publishing::abandon_bound` | [Recovery regressions](tests/review_workflow/recovery_regressions.rs): exact round, unresolved progress, retained effects, fresh round; [binding tests](tests/review_workflow/coverage_binding.rs): bound abandonment and its settlement | [CLI abandonment](../../bin/cli/README.md#publication-and-recovery), [result storage](../infrastructure/filesystem-adapter/README.md) |

Initial reviews, follow-ups, and interrupted attempts share these use cases.
The assessment and observed provider state determine the required effects.
Modules follow capabilities and keep dedicated ports beside their sole consumer.
Shared source, rendering, and publication ports live in `ports`.

## Assessment boundary

The agent supplies its decision, depth, scope, policy judgments, findings, and explanations.
Domain construction checks their consistency without replacing the decision.
The [domain README](../domain/README.md#findings-and-assessments) defines these invariants.

`AssessmentPolicies::new` accepts the agent's policy-set display name and policy
IDs with display names. It rejects blank names and duplicate IDs. It contains
no policy definitions, dependencies or applicability rules.
The agent reads linked skills, chooses coverage and supplies the evaluations.
Completion, checking and publication require the assessment's results to match
that declared coverage. This verifies record consistency, not coverage sufficiency.
Closed historical findings retain their prior policy without adding a current evaluation.
The application verifies that an unselected policy belongs to that owned finding's communicated history.

`AssessmentTemplate` leaves decision, depth, scope, policy judgments, and carried finding
reassessments unfilled. Prior findings provide context only.
Preparation reconciles exact verified replies, including communication completed within an unfinished round.
`complete` collects missing inputs and independent domain errors together.
`initialize` asks `AssessmentDraftWriter::create_new` to create the draft atomically.
An existing destination remains unchanged.
Initialization receives validated coverage before calling the writer.

Status changes and same-state reclassifications require explicit `TransitionReply` inputs.
Each names its finding, prior status, desired status, and explanation.
The application checks the transition against verified communication before rendering it.
Repeated transition inputs are compared with the latest verified transition for that finding.
An already communicated transition does not add another reply or restore an old resolution action.
Finding evidence does not substitute for an authored transition explanation.
Each finding permits one answer action per round, even when its thread contains several questions.
An answer retains the exact observed question as its target.
`local_context` remains local; it never enters domain assessments, provider writes, or renderer inputs.

`publishing::check_review` checks a complete review against a retained snapshot.
It does not rediscover provider records or acquire a publication session.
It performs no provider writes. Source and rendering adapters can perform I/O, including network reads.
It collects independent coverage, shape, source, finding, reply, and summary diagnostics.
Successful checking returns a `Plan`; failures return `Vec<PlanError>`.
Snapshot conflicts stop checking because the remaining checks require trustworthy review context.
Successful source and renderer results are reused during the invocation to avoid repeated adapter work.
Blocked reviews retain the same finding-input validation in checking and publication.
Their [publication rules](#publication-and-recovery) determine which finding effects remain eligible.

## Checkpoint boundary

A review run freezes one agent-authored assessment.
The [declaration use case](src/use_cases/checkpoints/README.md) validates selection, relevance and judgments together against the captured run and revision.
It returns a typed declaration or independent issues without I/O.
`review_run` supplies run, content-reference and captured-file identities.
Full assessment and publication checks apply to that same record.

## Public policy declarations

`PublishRequest.policy_declarations` and `Round.policy_declarations` retain every declared policy in assessment order.
Each `PublicPolicyDeclaration` contains its trusted `PolicyName`, `ApplicabilityStatus` and exact public reason.
Completion derives public applicability and judgments from each combined policy outcome.
Checkpoint references and private context are absent from these public rows.

`validate_policy_declarations` checks unique IDs, nonblank names and reasons, and exact assessed-slot names and IDs.
Applicable and unresolved rows own those slots; excluded rows have no judgment.
Unresolved applicability requires a blocked judgment; an applicable policy can also have blocked proof.
Closed carried findings can retain a policy excluded or absent from the current declaration.
This validates declared consistency; it does not establish that policy discovery was complete.

A change to declaration order, ID, name, status or reason starts a successor round even when findings and judgments repeat.
Pending and partial recovery require the exact retained declaration before any effect.
A blocked round without finding actions can complete in place with identical declarations.
Changed communication after a blocked round with finding actions starts a successor.
A declaration change starts a successor and retains the blocked round.
Original per-round declarations remain unchanged in history and abandonment.

## Discovery and source

`ReviewReader` supplies complete pagination, resolved discussions, authenticated reviewer,
revision references, API authors, and provider permissions.
Discovery checks the requested subject and reviewer before accepting the snapshot.
Markers alone never establish ownership. Adapters reject malformed reserved records.

The discovery bundle adds changed-file roles, exact diff text, reconciled findings,
owned history, relevant replies, and a checked next finding ID.
Changed or missing answer targets in the current unfinished revision appear in `DiscoveryBundle::recovery_conflicts`.
The bundle remains readable; publication still rejects replay against those targets.
Round and finding allocation include retained summaries, roots, and action markers.
Roots proven published by terminal rounds remain required across revisions, including eligible findings communicated by blocked rounds.
Carried findings whose current assessment permits their effects also require their existing roots.
Missing required roots and missing or changed immutable completed effects are conflicts.
Completed assessments cannot establish structural finding changes by themselves.
Reconciliation applies each change in round order only after its exact transition
reply is verified against the prior communicated state.
Provider discovery alone cannot distinguish a deleted unfinished root from one never committed.
Retained intent still reserves its finding ID.

`CommittedSource` receives the exact subject, revision, and path.
It reads committed regular text blobs and verifies provider diff positions.
The application checks line bounds and exact suggestion originals without whitespace normalization.
New findings and changed active positions or suggestions require source checks during publication.
Refreshing only a suggestion's revision requires source verification without making the suggestion a new semantic judgment.
The aggregate offline checker uses the same source-check selection.
Unchanged carried roots keep their existing thread targets.
Closed findings can retain historical source evidence without a current diff anchor.

## Conversation context and freshness

`conversation::Conversation` holds the merge-request title, description and every discussion with a supported non-system note.
It is review context; it grants no finding identity, reply authority, policy selection or effect authority.
Each `Note` keeps its author's provider user identity in `author` and, when the capture has it, an `AuthorAccount` in `account`: the username when the provider sent one, and a `BotStatus` of `Bot`, `NotBot` or `Unknown`.
`Unknown` means the capture could not establish the status; it shows neither a bot nor a person.
`Conversation::changes_since` lists every difference from a retained capture, ignoring order and observation time.
It identifies authors by `author` alone, so a renamed account, a missing username, a changed bot status or a capture without accounts is no change.

Publication freshness uses `Conversation::owned_changes_since` with `OwnedThreads`.
`OwnedThreads::of` collects each history's owned summary notes and finding root discussions that its reviewer authored.
Discovery verifies those records from the API author and corroborated markers, so a copied marker in another author's note adds nothing.
In scope are the owned finding discussions and every discussion that holds an owned summary note in either conversation.
Every change in scope is reported, whoever made it: publication updates, answers, resolves and reopens those threads.
A different subject is always reported. Title, description and other discussions are out of scope.
Callers pass the history captured with the retained conversation and the current discovery.
A thread owned at capture then stays in scope after it disappears, and a thread the reviewer created after capture enters scope.
The [conversation tests](src/conversation/tests.rs) and [published-history tests](tests/review_workflow/conversation_freshness.rs) cover these rules.

## Rendering and summary projection

`ReviewRenderer` validates its trusted profile and projected inputs against their schemas.
The provider adapter validates safe suggestion syntax.
The application rejects blank rendered bodies and reserved `review-` HTML comments.
The provider adapter inserts tool-owned markers outside rendered bodies.

The core builds `SummaryView` from verified provider observations.
It supplies distinct root counts, last communicated finding states, observed action labels,
history, and unpublished finding IDs. Replies do not inflate finding counts.
Per-policy rows retain applicable round numbers and count distinct published roots under their original policy.
They include every current inventory row, original per-round declarations and historical names when a policy leaves the inventory. Only Applicable declarations contribute applicable-round numbers.
Each row retains the root-note identity used for links separately from its discussion identity.
Templates format this projection without deciding counts or publication state.

`PublishRequest.review_workflow` carries the caller's optional public workflow name.
Each round retains that name, the declared set name, and its policy names.
Contextual renderer inputs carry those identities, scope and depth separately, and verified answer targets.
The renderer never receives `local_context`.
Library callers use `assessment::check_with_render_context` when checking individual findings with an identified rendering profile.
`assessment::check` calls the renderer without public round context; it cannot supply a round identity or publication status.

Summary writes retain compact `HistoricalFinding` rows in `Round.communicated_findings`.
Each row preserves the communicated finding, root-note and discussion references, and verified actions at that summary write.
Resolution labels require receipts bound to the retained round and action.
Historical sections use these retained rows. A missing snapshot means history is unavailable.
`PublicationProgress.verified_resolutions` retains confirmed resolution and reopening receipts across invocations.
Each receipt binds the subject, reviewer, round and discussion to verified state.
Its provenance distinguishes an acknowledged write from an observed recovery result.
Observed resolution labels do not claim that the original request caused the state.
`ExecutionEvent::Progress` checkpoints these receipts before publication continues.
Progress-aware checking and planning merge them into matching current and historical rounds.
This preserves actions when final-summary publication fails or the next assessment reviews new commits.

Offline plans contain exact bodies where inputs are known.
Final links remain unresolved until creation produces verified provider references.
Known bodies and the terminal summary shape are checked before publication.
A later summary render can still fail after findings exist; that failure produces partial publication.

## Publication and recovery

Ports use synchronous calls with borrowed implementations.
A runtime that cannot block schedules the use case on a worker.
`PublicationHost::acquire` binds the subject and credential's authenticated reviewer.
It does not exclude other writers. The caller serializes publication for one subject and reviewer.

`CreateSummary` and `UpdateSummary` are separate capabilities.
Before each write, the engine checks requested effects, initial host authority, and current provider permissions.
Authority is refreshed before effects and can only narrow during an invocation.
`PublicationProgress.authorization` preserves the original round's effect ceiling across invocations.
The first authorized prepared write establishes that ceiling; a denied attempt before preparation does not pin an empty grant.
Each invocation intersects that ceiling with requested effects and current authority.
Temporarily narrower authority does not erase the original ceiling.
The engine does not reject an entire plan because a later effect lacks authority.
Earlier authorized writes remain verified when a later effect is denied; the result reports partial publication.
No port exposes approval, merge, deletion, finding-root editing, or suggestion application.

The engine verifies a pending summary before finding effects.
An already identical pending summary is read back without another write.
Finding creates and transitions run in numeric finding order, with transition replies before resolution changes.
Answer replies follow those actions in request order.
Fresh checks cover revision, relevant discussions, ownership, expected targets, and authority.
Readbacks corroborate author, body, target, round, position revision, and effect state.
Observed conflicting edits stop progression. Revision drift stops finding effects and permits a truthful superseded summary.
Fresh checks cannot prevent another writer from changing state between a read and a write.
Expected summary checks are best-effort comparisons, without atomic compare-and-swap.
`Round::publication_limitation` carries a safe tool-owned reason for partial or superseded publication.
It stays separate from the agent's assessment limitation and excludes raw transport diagnostics.

`PublicationProgress` is application-owned data carried by the caller across publication and abandonment calls.
Its `unresolved` field holds an optional `UnresolvedWrite`: subject, reviewer, exact typed `Write`, profile, phase and baseline observation.
`publish` and `publish_recorded` accept `&mut PublicationProgress` after the renderer argument.
`abandon` and `abandon_recorded` accept it in the same position.
`RunPorts::progress` supplies it to `publication_run::run`.

The engine checkpoints `Prepared` before adapter work.
`PublicationSession::write_with_checkpoint` records `DispatchPossible` immediately before transport can send the request.
Adapters call that checkpoint after their preflight reads and request construction.
A failed checkpoint prevents dispatch. Default port implementations checkpoint before `write`; adapters with preflight work override this boundary.
Settlement and definitive rejection also checkpoint progress before publication continues.
Every effect requires exact readback before success.

Recovery settles an exact observed owned effect, including matching summary updates and resolution changes.
Observation establishes the desired result; it does not prove request causation or exclude a late original request.
A prepared request can resume after fresh validation because dispatch was not reached.
When fresh validation finds that it can no longer resume, such as after its answer target changed or its revision became stale, the engine records `ExecutionEvent::DiscardedUnsent`, checkpoints progress without it and continues from the current plan, whose own checks stop stale effects.
Every other field, including the binding and its dispatch flag, stays unchanged.
`DispatchPossible` and `LegacyAmbiguous` intent is never discarded.
`RecoveryPolicy::BoundedRetry`, the default, permits one extra attempt for an absent ambiguous creation.
Eligible creations are initial summaries, findings and replies.
The engine performs two additional complete observations before resending.
Exact retained intent, matching revision, unchanged relevant provider state and original authority remain required.
Failed or incomplete reads cannot establish absence.
The retry is consumed durably at the dispatch checkpoint and retained across invocations.
Rejecting the retry cannot settle the original unknown request.
`RecoveryPolicy::Conservative` disables these ambiguous resends.
Ambiguous updates are observed again but never automatically resent.
Progress bound to another subject or reviewer is rejected.

`LegacyAmbiguous` preserves older intent without assuming whether it was sent.
It supports exact observed settlement but cannot authorize a resend.
An unfinished round without a retained original grant cannot publish remaining effects.
After unresolved writes settle, explicit abandonment can close that round under current summary-update authority.

The caller retains progress between calls and preserves it before dispatch when crash recovery is required.
`PublicationProgress::default()` asserts that no earlier unresolved attempt exists; provider listings cannot establish that assertion.
[Empty journal admission](#empty-journal-admission) states when a durable caller with one authoritative state root may make it.
The crate supplies no filesystem storage or automated writer exclusion.
Discarding progress loses the unresolved-attempt safeguard.
There is no exactly-once or transactional publication guarantee.
`PublicationResult.retried_creations` reports resends that can produce late duplicates.
Recovery never deletes duplicate provider objects.

The tool compares completed communication mechanically; there is no agent-assigned assessment key.
Comparison includes decision, depth, scope, overall limitation, selected policies, policy result kinds, and structured finding judgments.
Cosmetic title, body, or evidence rewording alone does not create a round.
Pending recovery requires the retained assessment, answers, transitions, target observations, action bodies and the round's `confirmed_validation` mark.
It repairs missing effects without changing that intent.
Rounds and unresolved writes retain the renderer's profile identity.
A changed profile stops pending or partial recovery before writes or unresolved-intent reconciliation.
An unfinished record without profile identity cannot establish recovery compatibility with an identified renderer.
Completed communication remains unchanged when only profile assets change.
Previews use changed selected assets and policy display names without editing the completed provider body or allocating a round.
An unchanged completed assessment rejects changes to known public workflow or policy-set identities.
Missing retained identities remain unknown. A new assessment can record the caller's current identities.
Duplicate answer suppression compares authored answers and verified original replies independently from template formatting.
Finding prose changes become communicated state only through a verified transition; an assessment intention alone does not replace retained prose.
Completed history verifies owned roots and action replies independently of old foreign questions.
New answers require current targets to match their retained observations.
Callers supply those observations in `PublishRequest.answer_targets` before live discovery.
Planning compares the complete reply record, including its author, discussion, action identity, and body, before any publication write.
An absent observation rejects a new answer. Exact verified answers remain eligible for duplicate suppression.
Unfinished resolution or reopening intent carries across revisions only while its desired state matches the latest verified communicated finding.
A later completed round cancels that intent.
Completed history alone does not authorize changes to later manual thread state.
Blocked assessments can publish validated findings with determinate status and completed evaluations of their own selected policies.
Fixed or withdrawn historical findings retain their existing policy and transition checks.
Unvalidated candidates, uncertain findings and findings whose own policy evaluation is blocked generate no finding effects.
The initial summary stays pending or partial until required finding, reply and resolution effects are verified.
The terminal summary stays blocked and retains unavailable evaluations.
Verified communication does not establish review readiness.
Repeating identical blocked communication writes nothing.
A blocked round without finding actions can finish in place when its assessment becomes complete.
Changed communication after a blocked round with finding actions starts a successor and preserves the blocked history.

`publishing::abandon` closes an exact pending or partial round with an explicit public reason.
It updates only the owned summary and requires `UpdateSummary` authority.
This explicit request uses current requested, host and provider authority independently of the original round's ceiling.
Unknown write outcomes prevent abandonment.
An exact prepared abandonment summary can resume when its round, reason, profile and expected target still match.
Other `Prepared` intent is discarded the same way, since abandonment cannot resume it.
`DispatchPossible` and `LegacyAmbiguous` intent must settle before abandonment can proceed.
Repeating an already observed matching abandonment performs no writes.
The abandoned round retains its original assessment, actions, reserved finding IDs, and the subset of observed verified actions.
Abandonment does not delete published content, invent missing evidence, or replay unresolved actions.
A replacement assessment starts a fresh round and carries the findings already communicated.
`abandon_recorded` records observations, writes, and readbacks through the existing evidence port.

### Review-run binding

A durable journal can bind publication to one review run's accepted inputs.
`PublicationProgress.coverage_binding` holds a `RunBinding`: the `RunId`, the accepted assessment `ContentRef`, the retained `SnapshotFiles`, the round and `prior_authorization`, the round grant that existed immediately before installation.
`settled_runs` keeps one verified `SettledRun` per run ID with its outcome (`Published`, `NoChanges` or `Abandoned`), abandonment reason, round and observation time.
`dispatch_may_have_started` reports whether a write of the current binding could have reached the provider.
Default progress has no binding, no settlement and a false flag.

`publication_run::run_bound` and `publishing::abandon_bound` take an optional `BindingRequest`.
`run`, `publish`, `abandon` and their recorded forms are the unbound path.
Without a binding, they apply the unbound publication and abandonment rules.
Their publication and abandonment refuse progress that has one with `BindingError::BindingRequired`.
Binding refusals return `PlanError::Binding` before any provider write.

A binding has no possible provider effect when its dispatch flag is false and the journal has no unresolved write, or only a `Prepared` one.
The dispatch checkpoint persists `DispatchPossible` and the flag before any provider call, so a `Prepared` intent was never sent.
`PublicationProgress::unsent_binding` reports this fact.

Admission uses the journal and the invocation's first provider observation:

| Journal state | Admission |
| --- | --- |
| The requesting run is in `settled_runs` | `AlreadySettled` |
| Bound to the requesting run with the same inputs | Resume. Different inputs, or a plan for another round, are `Mismatch`. |
| Bound to another settled run | Replace |
| Bound to another unsettled run whose binding has no possible provider effect | Release that binding without settlement, checkpoint the release, then replace |
| Bound to another unsettled run with a `DispatchPossible` or `LegacyAmbiguous` write | `PendingOwner` with that run's ID |
| Bound to another unsettled run with a set flag and no unresolved write | Replace only when the reviewer's only summary shows the bound round `Completed`, `Blocked`, `Superseded` or `Abandoned`. That run is first settled as published, or abandoned with the provider's reason, and checkpointed. Otherwise `PendingOwner`. |
| No binding with an unresolved write | `Unresolved`; keep the current generic intent unchanged |
| No binding | Replace |

Release drops the `Prepared` intent and restores `authorization` to the released binding's `prior_authorization`.
An unused grant of the attempt therefore neither caps nor widens the replacing run, and a grant that predates the binding stays.
The binding becomes the one described by the most recent settlement, with a set flag, or none with a clear flag when nothing is settled.
Release never changes retries, receipts or settlements; a binding that sent nothing consumed no retry.
Only another run's admission releases a binding. The owner's own publication, abandonment and lifecycle commands keep it.
`PublicationProgress::released_for_admission(run_id)` returns a copy of the journal as the admission of `run_id` would leave it after that release, and `None` when admission would not release the binding or `run_id` owns it.
It changes and persists nothing; a plan for the replacing run uses it so that the plan matches the publication that follows.

A replacing binding is installed immediately before the invocation's first prepared write, after authority checks.
It records the grant held before that write establishes its own.
It starts with a false dispatch flag and is persisted in the same checkpoint as the prepared intent.
The dispatch checkpoint sets the flag together with `DispatchPossible`.
The flag stays true across later prepared writes, including after earlier effects of the binding settle.
A definite rejection of the binding's first write restores the previous binding and flag when the binding was installed by that write and no effect is verified in the invocation.
Otherwise a definite rejection clears the intent and returns the flag to its value before that write.
A rejected first dispatch therefore leaves a binding without a possible provider effect, whichever invocation installed it.
A later verified readback of the same write reinstates the binding and the flag.
Admission and refusal never consume, reset or replace another run's retries, grant or receipts.
Plan mode ignores the binding request and never changes the binding or the flag.

The caller records the run's settlement after the invocation with `PublicationProgress::settle_publication`.
`settled_outcome` maps `Completed`, `Blocked` and `Superseded` to `Published` and `Abandoned` to `Abandoned`.
`Unchanged` maps to `Published` when the binding dispatched and to `NoChanges` otherwise.
Other statuses leave the run unsettled.
A mapped outcome settles only when the result's final snapshot shows the reviewer's only summary at the result round as `Completed`, `Blocked`, `Superseded` or `Abandoned`, and no write is unresolved.
A superseded result also needs its superseded summary verified.
An abandonment takes its reason from the observed abandoned summary.
An abandonment settles only the run that owns the binding: the call resumed that binding or installed it with the verified abandonment summary.
`abandon_bound` refuses a run that installs no binding and finds the round already abandoned with `NotAbandonable`.
An `Unchanged` outcome reached without a write of the run installs its binding first.
Another run's unsettled binding prevents that unless it has no possible provider effect, in which case it is released first.
Entries are unique by run ID. An identical retry is accepted; a different entry is `SettlementConflict`.
`record_settlement` adds an entry that describes the current binding.
Settlement keeps the binding. A later admitted run replaces it, and earlier entries remain for local repair.

`run_publication` reports a run for local reconciliation:

| State | Meaning |
| --- | --- |
| `Settled` | The run has a verified outcome |
| `BoundUnsettled` | The run owns the binding and one of its writes could have reached the provider |
| `BoundUnsent` | The run owns the binding and none of its writes could have reached the provider. It is diagnostic: the run keeps the binding until another run's admission releases it. |
| `Unbound` | The run neither owns the binding nor has a settlement |

#### Empty journal admission

A durable caller without retained progress starts from `PublicationProgress::default()`, which asserts that no earlier write is unresolved.
`ManagedHistory::observe` classifies the reviewer's owned provider state, and `admits_empty_journal` decides whether a storage adapter may create an empty journal:

| Provider state | `OwnedHistory` | Empty journal |
| --- | --- | --- |
| No owned summary, finding root or managed reply | `Absent` | Admitted |
| The reviewer's only summary shows its current round `Completed`, `Blocked`, `Superseded`, or `Abandoned` with a public reason | `Finished(round)` | Admitted |
| The reviewer's only summary shows its current round `Pending` or `Partial` | `Unfinished(round)` | Refused |
| Owned state without exactly one owned summary, or a current round abandoned without a public reason | `Ambiguous` | Refused |

`Finished` uses the terminal states that settle another run's binding in the admission table above.
After a finished round, the next publication needs nothing that the earlier journal held:

- Unresolved write: progress holds at most one, and a round's terminal summary is its last write, so nothing of that round remains unresolved.
- Effect ceiling: the engine caps a round only with a retained grant for that round, or with an empty grant for a pending or partial round that has none. A new round records its ceiling with its first authorized write. A blocked round without finding actions that finishes in place is capped by its original grant only while its journal exists; from an empty journal its first write records a new ceiling.
- Consumed retries: a retry matters only while its own write is unresolved. `PublicationResult.retried_creations` reports the resends recorded in the current journal.
- Settlements: they describe runs of the environment that kept the journal, and only those runs need them for local repair.
- Resolution receipts: every summary write carries its round's receipts and merges retained ones into history, so the finished summary already holds them.

A finished round cannot show that no other state root holds a write still in flight, such as the pending summary of a later round started elsewhere.
The caller keeps one authoritative state root, as it does for absent history.

## Run evidence and results

`publication_run::run` combines planning or publication with caller-carried progress, `Clock`, and `ExecutionEvidence` ports.
An empty effect set forces the read-only plan path, even when publish mode was requested.
The host records start and finish times, observations, plans, intended writes, and readbacks.
A failed evidence write before a provider mutation prevents that mutation.
`ExecutionEvent::Progress` persists phase changes, grants, consumed retries and settlements.
For bound publication it also persists binding installation, the dispatch flag and provider-verified owner settlement.
`ExecutionEvent::BeforeWrite` records the exact attempted write in execution evidence.
Durable recovery requires a consumer to persist that intent and restore progress before the next publication call.
A final evidence failure remains separate from the actual publication outcome.
Concrete clock reads, files, raw HTTP payloads, and transport response storage belong to adapters.
Configured adapters and evidence sinks retain renderer/profile identity with the typed core evidence.

`PublicationResult` separates review decision from communication status.
Preflight failure can return `Err(PlanError)` before a `PublicationResult` exists.
Within an execution result, `NotStarted` covers evidence or read failures before any attempted write, or a confirmed initial rejection without a verified effect.
`Denied` identifies an authority refusal; `Uncertain` identifies an outcome that provider evidence cannot settle.
The retained snapshot preserves any earlier round state when the current invocation cannot start.
It retains planned, attempted, verified, and skipped effects, exact writes, readbacks,
the final snapshot, the original stop, and any finalization failure.
`RunReport` retains the final assessment and outcome even when saving final evidence fails.

## Operation logging

Application operations emit structured logs when the caller installs an
observer with `review_kit_observation::with_observer`. Direct library calls
share the same observation boundaries as CLI calls. The
[observation crate](../observation/README.md) defines the JSON format and sink
contract. Generated operation declarations supply the logging lifecycle. Public
entry points bind caller-owned ports to managed facades; internal stages reuse
those handles.

Records cover discovery, assessment preparation and completion, validation,
planning, initialization, publication and abandonment.
Internal records cover source reads, position checks, rendering, publication
history and progress reconciliation, provider session acquisition, authority
checks, writes, readback checks and evidence checkpoints. Caller-supplied port
implementations receive this application-level coverage too.

Application log projections disclose operation categories, counts, booleans,
effect names and outcome states. Subject identities, reviewer identities,
revision references, paths and rendered text are redacted. Assessment prose,
local context and provider objects are not serialized into logs. Unknown port
error labels and cause text are opaque; known error categories remain structured.

Provider write records distinguish an attempted call, acknowledgment, rejection
and uncertain response. A separate verification record reports whether the
readback matches. Publication records retain the actual result, including
observed settlement and possible duplicate creations, and separate original,
finalization and final evidence errors. A verified existing summary can complete
an effect without another write attempt.

Log failures do not change application return values or publication progress.
Execution evidence keeps its existing authority: failure to persist a checkpoint
can prevent a provider write. Logs do not replace retained publication progress.

## Verification

Run the [workspace checks](../../../README.md#workspace-checks).
Application tests cover the use cases through public ports with deterministic failure injection.
They exercise lost responses, source and rendering failures, authority changes, unchanged reviews,
blocked recovery, relevant discussion edits, history, and revision drift without network access.

The [GitLab adapter](../infrastructure/gitlab-adapter/README.md) implements provider, committed-source and trusted host-binding ports.
Its controlled fixtures verify normal publication and safe refusal of unresolved
writes. The [local GitLab CE suite](../../../tests/gitlab/README.md) also exercises
these use cases with real provider operations on its pinned image.
The [renderer](../infrastructure/json2md-adapter/README.md),
[JSON](../infrastructure/json-adapter/README.md) and
[filesystem](../infrastructure/filesystem-adapter/README.md) adapters complete
the local integration. The [CLI](../../bin/cli/README.md) wires them.
The [delivery plan](../../../docs/plan.md#current-status) owns implementation
status and remaining compatibility boundaries.
