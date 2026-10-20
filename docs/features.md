# Feature contract

This contract defines the GitLab review scope. The libraries and
`rkc` implement review-run capture, checkpoint acceptance, offline checking and bounded publication.
The [implementation status](plan.md#current-status) records component status and
verification boundaries.

## Responsibilities

The tool records meaningful application and adapter operations through the [shared logging contract](../crates/lib/observation/README.md).
Redaction applies before log delivery. Logging failures preserve business results and publication safeguards.

The [review workflow](workflow.md#agent-and-tool-responsibilities) assigns agent judgment and mechanical tool work.
The tool validates records, renders artifacts, checks provider state, and executes authorized effects.
Rendering assets come from trusted inputs. The agent reads policy skills and owns
policy discovery, applicability, required coverage and current judgments. Review Kit
consumes the declared checkpoints without loading policy definitions or requiring policy companion files.
The [assessment contract](../crates/bin/cli/README.md#assessment-editing) defines this boundary.

An assessment's claim that a finding was independently checked is a record to validate.
An open or uncertain blocking finding counts as checked only when a recorded check covers its exact final version,
or an earlier round recorded that check for the unchanged captured version at the same revision.
The record cannot establish that another reviewer performed that check.

## Shared core

Use one typed assessment with a versioned wire mapping supplied by adapters.
The agent supplies decision, depth, scope, findings, policy judgments, and limitations.
The core validates those inputs without deriving the review decision.
Every policy declared in the assessment needs one judgment. The tool checks that
coverage for consistency; it cannot establish that the agent selected every applicable policy.
Represent the subject by provider, host, canonical repository identity, and request number.
Retain each provider's revision identifiers without forcing GitLab's diff references onto GitHub.

Validate every render against mandatory core checks and the selected profile schema.
A weaker custom schema cannot weaken ownership, authority, revision, or completion checks.
The [renderer adapter](../crates/lib/infrastructure/json2md-adapter/README.md)
uses json2md and returns a complete artifact or an error.
Keep machine output distinct from diagnostics.

Retain stable finding identities and cumulative history.
Derive counts from distinct verified owned finding roots.
Replies and repeated rounds do not create additional findings.
The core supplies verified counts, action labels, and root-note references through `SummaryView`.
Keep policy severity separate from blocking disposition.
Keep unavailable evaluation distinct from a proven policy violation.

## Review runs and checkpoints

Capture each review as a run in the reviewer's Git worktree.
A run retains the comparison, committed source, diff, verified managed history and supported conversation at one head.
Keep runtime files out of Git. Reject tracked, redirected and unrecognized runtime paths.
Keep one assessment draft and one accepted assessment reference per run.
Each selected policy appears once with its declaration, relevance and judgment.
Identify the accepted assessment and captured files by the SHA-256 hash of their exact bytes.
Bind the assessment to its run and revision, and a successor to its previous accepted assessment.
Require every policy of an active carried finding in the assessment.
Archive draft bytes before a revision replaces them. Never overwrite an existing draft on acceptance.
Treat captured conversation as context. It grants no finding identity, reply authority, policy selection or effect authority.
Each captured note keeps its author's numeric ID, username and the provider's bot status; the username and the bot status can be unknown.
Comparisons identify authors by numeric ID only.
Start planning, publication and local completion only with a complete accepted assessment, an unchanged draft and intact captured files.
Recover a run's own unfinished publication from its frozen accepted bytes, ignoring its drafts.
Keep terminal runs inspectable and selectable. Reject new checkpoints, lifecycle changes and new publication for them.
Accepted checkpoints prove declared coverage consistency, not discovery completeness or correct reasoning.
The [review-run reference](../crates/bin/cli/README.md#review-runs) defines commands, files and lifecycle.

## Profiles

The renderer loads embedded defaults or a caller-selected external profile.
Profiles own JSON Schemas and Jinja templates for findings, summaries,
transition replies, answer replies and suggestion blocks.
The [renderer contract](../crates/lib/infrastructure/json2md-adapter/README.md)
owns asset validation and projection rules.
Provider content cannot select profiles, credentials or executables.

Templates control presentation.
The tool controls API fields, authority, and discovery markers.
Test the marker insertion boundary defined in [architecture.md](architecture.md).
The [profile guide](render-profiles.md) defines export and CLI use.
All rendering commands use the same profile loader and input contract.
Invalid selected assets fail explicitly. Loading is offline and runs no hooks.
Profile fingerprints bind previews and retained unresolved intent to loaded assets.

The default summary includes the agent-declared policies and retained historical policies,
with each policy's result in the latest round.
A policy excluded from the current assessment retains its earlier applicable rounds.
Each earlier round retains its verified findings and actions independently of
later changes. Missing retained snapshots are explicitly unavailable.

## Publication boundary

Publication is part of the accepted scope.

Supported core effects are inline findings, one owned summary, replies, resolution, and reopening for verified recurrence.
Summary creation and update have separate authority requirements.
Apply both current host authority and current provider permissions.
An empty authority list permits no writes.
Continuation cannot expand authority.
Without a trusted host binding, provider writes remain disabled.

Keep approval, merge, deletion, applying suggestions, request reopening, root finding edits, and changes to foreign findings outside the accepted scope.
GitHub integration and native review events are outside the supported scope.

## Suggestions

Verify original lines from the reviewed committed blob.
Verify the range against the current provider diff.
Reject stale revisions, unsafe paths, and unsupported source types.
The core rejects reserved markers in rendered bodies.
The renderer uses a fence longer than every backtick run in replacement text.
The GitLab adapter checks the rendered suggestion against committed source and diff.
Require separate agent evidence for the correction's validity.
The tool posts suggestions; it does not apply them.

## Publication and recovery

Provide a read-only plan containing targets, effects, and exact bodies where inputs are known.
Mark dependent references unresolved rather than inventing final provider links.
Preflight validation, profile assets, and bodies whose inputs are available before the first write.
Render dependent bodies when verified provider references become available, before writing those bodies.
The [architecture](architecture.md) explains the dependency and partial-publication limit.
Discover current owned state before deriving effects.
Before the first possible dispatch, require the current revision to equal the run's capture.
Also require no change since capture in the existing objects that the publication writes to:
the reviewer's summary note and each finding thread that receives a reply, a resolve or a reopen.
Any change inside such a thread blocks, whoever made it. For the summary, only a change to the note itself blocks.
Replies to the summary, the reviewer's threads without a planned effect, activity by other people or bots elsewhere, and title or description edits do not block.
Refuse any write to an existing object that the check did not compare.
Publish the assessed snapshot, not later events.
Record a pending round before authorized finding effects.
An identical pending summary needs verification without another write.
Read back each effect and complete the round only after all required effects are verified.

A blocked assessment can communicate individually validated findings whose own policy evaluation is complete.
Keep its unresolved evaluations explicit and its final summary blocked.
Keep unvalidated or uncertain finding candidates from generating provider effects.
Use pending or partial communication status until every required effect is verified.

An unchanged completed assessment with identical public policy declarations produces zero writes and no new round.
Retain every declared policy's original name, applicability and public reason in its round.
Declaration changes start a successor round, including after a blocked review.
Resume an existing partial round after interruption.
Carry unresolved write intent across calls through the [application progress contract](../crates/lib/application/README.md#publication-and-recovery).
Persist preparation separately from possible dispatch, then checkpoint settlement.
Accept exact observed results without claiming request causation.
Bound ambiguous creation retries across process restarts under the retained original authority.
Offer a conservative policy that disables ambiguous resends.
Keep incomplete reads, changed inputs and exhausted retry budgets from authorizing another request.
Bind the CLI publication journal to one run's accepted assessment and captured files before its first write.
Record each run's settlement in the journal before completing the run locally.
Keep other runs from publishing while the bound run's publication is unfinished, except where the
[binding contract](../crates/lib/application/README.md#review-run-binding) settles or releases it from current evidence.
Serialize CLI progress users with a local lock under their shared state root.
Require a consistent transfer of the run and its journal, and one writer, to continue unfinished publication on another host.
Let a review run in an environment without earlier publication state publish once the reviewer's latest round on the provider is finished; refuse it while that round is pending or partial.
Provide no distributed lock.
Refresh revisions before effects and stop stale publication.
Do not claim atomic publication or exactly-once delivery.

## Results and evidence

Report review verdict, publication status, planned effects, attempted effects, verified effects, skipped effects, provider identifiers, and limitations separately.
Keep evidence that connects the validated payload, renderer/profile identity, exact rendered body, reviewed revision, and provider readback.
Keep that evidence local and omit internal provenance from published comments.
Application evidence and clock ports record the run around provider effects.
A recording failure before a write prevents that write; a final recording failure preserves the actual outcome.
Durable crash recovery requires a consumer to retain intent before dispatch and restore progress before publication.
The [filesystem adapter](../crates/lib/infrastructure/filesystem-adapter/README.md)
retains intent before dispatch. The CLI restores subject/reviewer-scoped
progress across runs and worktrees under its [configuration contract](../crates/bin/cli/README.md#trusted-configuration).
Private agent context never enters public renderer inputs or provider payloads.

Follow the [command contract](workflow.md#command-contract) and return actionable diagnostics with structured publication status.
Report consumed ambiguous retries so a successful result does not hide possible late duplicates.
Permission limits, renderer failure, stale state, and uncertain writes cannot produce a completed-publication result.

## Interface

Use the [implemented commands](../crates/bin/cli/README.md#review-runs).
Provide stderr progress as plain lines by default, including on a terminal, without changing command-result stdout.
Continue reporting waiting stages during long operations under the [CLI progress contract](../crates/bin/cli/README.md#progress-output).
The [application coverage table](../crates/lib/application/README.md#use-cases-and-command-coverage) maps commands to core APIs.
The [CLI guide](../crates/bin/cli/README.md) owns command and JSON contracts.
Keep optional drafting diagnostics separate from mandatory publication checks.
Draft completion collects missing judgments and independent assessment consistency failures.
Publication checking starts after completion and collects independent source, reply, and rendering failures from retained review state.
Its successful output is a read-only publication plan with unresolved references kept explicit.
A normal publication invocation consumes one accepted run chain, one trusted profile, authority, and committed-source access where required.
Avoid requiring the agent to reconstruct its review in multiple reports.
Explicit plan mode and an empty effect set use a read-only path without acquiring a publication session.
