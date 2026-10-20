# review-kit-json-adapter

This crate maps local JSON documents to the domain and application models.
It owns provider-independent serialization.
The GitLab adapter uses its assessment and round mappings for retained summary state.
GitLab owns markers, transport fields, and provider body restrictions.

## Local input contract

Every local input has a fixed `kind`. Contexts and assessments use `version: 1`;
[review-run documents](#review-run-documents) define their own versions.
Legacy publication progress writes version 2 and reads versions 1 and 2.
The run-bound journal reads and writes version 3.
Unknown fields and unsupported enum values fail parsing.
Conversion rejects unsupported versions and invokes the domain constructors.
The [input schema](schemas/review-input-v1.schema.json) describes document structure.
The [journal schema](schemas/publication-progress-v3.schema.json) describes version 3 progress.
Core validation also checks declared policy coverage, assessment consistency, and retained review history.

| Document | Kind | Conversion |
| --- | --- | --- |
| `ContextDocument` | `context` | `bundle()` reconstructs and validates discovery through application use cases |
| `DraftDocument` | `assessment` | `complete()` validates context and agent-declared coverage, then completes the assessment |
| `ProgressDocument` | `progress` | `into_progress()` restores versions 1 and 2 and `into_journal()` restores version 3; both validate subject and reviewer binding and restore exact write intent |
| `ConversationDocument` | `review_conversation` | `into_conversation()` validates structure and identifiers and restores the [supported conversation](#supported-conversation), version 2 or 1 |

The [examples](examples/) contain empty discovery context, unassessed draft, and empty publication progress.
Their host, reviewer, and revision values are synthetic.

### Supported conversation

`ConversationDocument` retains a captured application `Conversation`.
`ConversationDocument::new(&conversation, observed_at)` writes version 2
(`CONVERSATION_VERSION`), described by the
[version 2 schema](schemas/review-conversation-v2.schema.json). `into_conversation()`
also reads version 1, described by the [version 1 schema](schemas/review-conversation-v1.schema.json).
The document holds the subject, the caller's `observed_at` capture time, the
title, the description and the discussions in captured order. The
[CLI guide](../../../bin/cli/README.md#captured-conversation) defines each field's
meaning for review agents.

`ConversationDocument::new` requires an RFC 3339 UTC time in the form
`YYYY-MM-DDTHH:MM:SS[.fraction]Z` and every note's author account. Each
position is written as the provider's JSON object.

`into_conversation()` rejects unknown fields, other versions and kinds.
Every discussion needs `resolved` and every note needs `position`; either can be
`null`. `resolved` is `null` exactly when `resolvable` is false. IDs and authors
are nonblank, each discussion has a note, and discussion and note IDs are
unique across the document. A version 2 note also needs an `author_username`
that is a nonblank string or `null` for unknown, and an `author_bot` of `true`,
`false` or `null`, which restore as `BotStatus::Bot`, `NotBot` and `Unknown`. A version 1 note has neither field
and restores without an account (`Note::account` is `None`). A version 1
conversation cannot be written again as version 2.

`canonical_position` converts a position object to canonical text: keys sorted
by byte order at every depth, serde_json scalar formatting and no whitespace.
Reading a document and capturing from GitLab both use it, so key order and
spacing in the file do not affect comparison; every key and value does.
`observed_at`, the stored order and author accounts are not part of the
comparison, and `Conversation::changes_since` ignores all three.
The conversation is review context. It carries no managed finding identity and
no reply, answer, policy or effect authority.

### Editable assessment

`DraftDocument::from_initialized` stores the unassessed application template.
`context` names an immutable discovery document relative to the assessment's parent directory.
The path must contain normal relative components without traversal.
Subject, reviewer, and revision must match that document exactly.

`policy_set` records the agent's named policy-set display identity.
`policies` declares coverage directly with IDs, optional display names and nullable judgments.
Completion constructs validated `AssessmentPolicies` from those rows. Blank names
and duplicate IDs fail validation. An absent name uses the policy ID for display.
The agent reads the linked skills and owns applicability, evaluations and gaps.
The adapter loads no policy definitions or catalog.

Retained version 1 assessments may contain `requested_policies`. The reader
accepts that field without using it; new assessments omit it.

Judgments use these shapes:

```json
{"kind": "satisfied"}
```

```json
{"kind": "violated", "evidence": "Established violation evidence"}
```

```json
{"kind": "blocked", "limitation": "Missing evaluation context"}
```

`findings` contains immutable `previous` context and a nullable `reassessment` for each carried finding.
The reader verifies previous context against retained discovery.
`new_findings` contains new finding records.
Both use the domain finding shape in the input schema.
Suggestions retain exact original lines, replacement lines, source location, revision, and correction evidence.

The agent fills `decision`, `depth`, and `scope`.
A blocked decision requires `limitation`.
Missing judgments remain missing after round-trip serialization.
`answers` and `transitions` hold explicit communication inputs.
Completion derives `PublishRequest.answer_targets` from the assessment's referenced context for the [application's answer freshness check](../../application/README.md#publication-and-recovery).
The editable assessment supplies no separate question copy.
`local_context` never enters completed assessments, rounds, or public rendering inputs.
Draft debug output and local JSON serialization include it.

Completion returns independent draft problems as `Diagnostic` records.
Each record has a stable `code`, optional finding and policy references, and a repair message.
Conversion failures also identify the input array position.
Checking diagnostics flatten independent coverage, source, rendering, shape, and reply failures.

### Publication progress

Progress preserves the complete unresolved `Write`.
Version 2 also retains its `phase` and baseline observation, the original round's
`authorization`, and `retried_creations` that have consumed their one retry.
An explicitly requested abandonment summary can omit the original review grant;
its exact terminal intent remains bound to the baseline round and expected summary.
Version 1 unresolved intent becomes `legacy_ambiguous`; missing metadata never establishes that dispatch did not happen.
Summary writes include the expected summary, intended round, history, and exact body.
Unresolved writes also retain their rendering profile identity.
Progress retains confirmed resolution receipts independently from unresolved write intent.
Receipt provenance distinguishes `acknowledged` from `observed` recovery.
Older receipts without provenance decode as `acknowledged`.
Finding writes retain revision, finding, and body.
Reply writes retain thread, action identity, and body.
Resolution writes retain thread and intended resolution state.

Version 3 adds `coverage_binding`, `settled_runs` and `dispatch_may_have_started`.
These fields are required in version 3 and rejected in earlier versions.
`ProgressDocument::journal` always writes version 3. `ProgressDocument::new` writes version 2
unless the progress has a binding, a settlement or a set dispatch flag, which only version 3 represents.
A binding names the run ID, round, accepted assessment reference and the four retained snapshot hashes by file name.
Its optional `prior_authorization` holds the round grant that existed before installation, in the `authorization` format; the writer omits it when no grant existed.
A settlement adds its outcome (`published`, `no_changes` or `abandoned`), reason and RFC 3339 `verified_at` time.
Writing uses UTC with nanosecond precision, so every written time restores exactly.
Settlements have unique run IDs, and an abandoned settlement needs a nonblank reason.
A settlement for the current binding describes that binding and excludes an unresolved write.
A null binding excludes an unresolved write, settlements and a set dispatch flag.
It can retain a grant, consumed retries and resolution receipts.
The generic progress and current run-journal methods use their explicit document contracts; no automatic adoption or consolidation occurs.

The progress document and unresolved attempt must match the supplied subject and reviewer.
Stored intent supports recovery; it does not establish request settlement.
The recovery policy is an invocation option, not a grant stored in the document.
Every retained `RoundData` requires the current public declaration, including inside an earlier progress envelope.
Follow the [retained-state compatibility contract](../../../../docs/render-profiles.md#retained-state-compatibility) when changing builds.
The [application recovery contract](../../application/README.md#publication-and-recovery) defines continuation limits.

### Retained provider state

[`rounds::State`](src/rounds/state.rs) reads and writes version 3 provider state.
Its header stores the subject and reviewer once. Shared tables store distinct
complete findings and distinct policy ID/name pairs. Round assessments,
creation actions, communicated rows and abandonment receipts reference findings
by zero-based table index. Assessed-slot names and every public declaration reference the same policy-name table.
Declaration status and full public reason remain in their original round.
Different versions of the same finding ID remain separate table entries.
Historical policy renames remain separate rows.

The writer inserts table rows on first use, starting with the current round,
then history in stored order. Within each round, finding references follow the
assessment, intended actions, communicated rows and abandonment receipt.
The reader rejects duplicate, unused, out-of-range and reordered table entries.
Unknown fields and invalid domain values fail restoration.

A round accepted under confirmation enforcement records `"confirmed_validation": true`;
the field is absent otherwise, so restoring and rewriting an earlier round keeps its bytes.

Resolution receipts retain thread, resolution state and provenance. Their
subject, reviewer and round come from the header and containing round.
Encoding rejects receipt bindings that disagree before omitting those fields.
Full rendered bodies, evaluations, answers, answer targets, public identities
and profile identity remain exact. Absent communicated snapshots remain distinct
from recorded empty snapshots.

The codec bounds raw JSON to 8 MiB. Before cloning shared values, it limits an
aggregate expansion estimate to 32 MiB and expanded references to 65,536.
The estimate includes compact JSON bytes, every referenced finding and policy
name, and each restored assessment or resolution header. References count those
table values and restored headers. Checked arithmetic rejects overflow.
Encoding applies the same limits. These fixed codec bounds are separate from
the GitLab client's configured request, response and pagination limits.

The provider DTO is separate from `RoundData`, which continues to encode local
context, journal intent and command results. Unsupported provider-state versions
fail without conversion or provider writes.
The [application projection](../../application/README.md#rendering-and-summary-projection)
defines how restored records affect rendering and recovery.
The [GitLab format](../gitlab-adapter/README.md#discovery-markers-and-retained-state)
owns escaped framing and independently versioned discovery markers.

## Review-run documents

The `checkpoints` module maps one authored review and the tool's run metadata.
Documents reject unknown fields. Required nullable fields must be present.
The adapter performs no file or network I/O.

| Document | Kind | File | Schema |
| --- | --- | --- | --- |
| `ReviewWorkspaceDocument` | `review_workspace` v1 | `review-workspace.json` | [workspace](schemas/review-workspace-v1.schema.json) |
| `ReviewRunDocument` | `review_run` v1 | `review-run.json` | [run](schemas/review-run-v1.schema.json) |
| `AssessmentDocument` | `assessment` v4 | `review-assessment.draft.json` and its accepted snapshot | [assessment](schemas/review-assessment-v4.schema.json) |
| `AcceptanceResultDocument` | none, version 1 | `accept` output | [acceptance](schemas/checkpoint-acceptance-result-v1.schema.json) |

### Tool-owned documents

`ReviewWorkspaceDocument` holds the repository binding and selected run ID.
The binding compares provider, host and repository, excluding the request number.
`ReviewRunDocument` holds run identity, captured-file hashes, one optional `review_assessment` reference and lifecycle.
`RunLifecycle` distinguishes open, completed and stopped states.
Completed states record their outcome; abandonment and stopping require a reason.
Decoding and encoding reject invalid lifecycle combinations.
`AcceptanceResultDocument::path` derives `accepted/<hex>/review-assessment.json` from the accepted reference.
A disagreeing serialized path is rejected.

### One authored review

`AssessmentDocument::initial` writes the run ID, exact captured revision, carried finding IDs and each carried entry's generated `reusable_confirmation`.
Other authored values start empty or unset.
Each policy contains its identity, display name, required flag, relevance reason and typed outcome once.
`PolicyOutcomeData` distinguishes excluded, unresolved, satisfied, violated and blocked outcomes.
Null outcomes are draft placeholders, rejected at completion.
Subject, reviewer and previous findings come from immutable captured context, without copies in this file.
Shared proof and private notes remain in readable retained evidence; `local_context` holds at most `LOCAL_CONTEXT_LIMIT` characters of pointers and decisive facts.

`check_assessment` decodes and completes against `AssessmentInputs`: run, context and successor rule.
`AssessmentDocument::complete` validates the [combined declaration](../../application/src/use_cases/checkpoints/README.md), derives judgments and applies existing assessment rules.
It attaches public declaration rows in authored order.
Each row preserves its public name, derived applicability and complete relevance reason.
The adapter does not discover policies or prove that an independent check happened.

Carried entries contain their ID, the generated `reusable_confirmation` and a `ReassessmentData`: pending (`null`), `"unchanged"` or a complete finding.
Completion rejects a changed `reusable_confirmation` (`changed_reusable_confirmation`).
Completion restores their immutable previous state from context and checks exact carried coverage.
An unchanged decision uses the captured version; one that requires confirmation restarts unvalidated.
`ConfirmationData` entries become the application's [confirmations](../../application/src/use_cases/checkpoints/README.md#finding-confirmations),
which set validation on open and uncertain blocking findings before domain completion.
New findings, answers and transitions use the shared domain mappings.
`local_context` never enters completed assessments, public renderer inputs or provider bodies.

Diagnostics include malformed or unsupported documents, wrong run or revision, stale successor, missing declarations, duplicate policies, required-policy exclusion, oversized private context and confirmation problems.
The [CLI reference](../../../bin/cli/README.md#assessment-editing) owns the authored-field contract and command behavior.

### Revision

`revise_assessment` prepares one successor from current draft bytes or retained accepted state.
`AssessmentRevisionInputs` supplies the run, context and optional accepted assessment.
`RevisionRequest` supplies the reason and regeneration choice.
The returned `Revised` document binds the captured identity and current accepted reference programmatically.
Revision clears the decision and independent-validation assertions, and drops recorded confirmations.
It clears applicable judgments while retaining excluded and unresolved outcomes for affected reassessment.
It keeps candidate findings, carried reassessments, answers, transitions, scope, depth, limitations and private references.
Incompatible candidates and unknown references remain visible through diagnostics.
Without regeneration, malformed bytes produce `RevisionError::MalformedDraft` without a successor.
Regeneration uses accepted state, or initial captured state before first acceptance.
The host archives exact prior draft bytes before replacing them.

## Output documents

`CheckDocument` includes accumulated diagnostics or the checked plan.
`PlanDocument` retains exact known bodies and unresolved finding IDs.
Its terminal summary preview can lack provider links.

`RunReportDocument` retains timing, the final assessment, plan or publication outcome, and final evidence failure.
Publication results include `retried_creations` when a creation was resent after uncertainty.
Those writes can have late duplicates even when the result is completed.
`PublicationResultDocument` preserves decision separately from communication status.
It separates attempted writes, verified effects, skipped actions, original stop, and finalization stop.
`AbandonDocument` carries the same publication result or planning diagnostics.

`EvidenceDocument` stores typed application events, including `discarded_unsent` for a prepared write that never reached the provider and could not resume.
`EvidenceDocument::records_publication_attempt` reports whether the events show that publication or abandonment began; planning events do not.
The host can record its renderer identity on run reports and rendered evidence events.
Each event keeps its own identity across appended runs; absent metadata remains unknown.
These are local documents and may contain provider identifiers, source evidence, and diagnostic causes.
Only the provider adapter selects public serialization and rendering boundaries.

## Operation logs

Within a scoped [observer](../../observation/README.md), decoding, encoding,
context reconstruction, draft completion, checkpoint checks, templates and
revisions, and progress and round restoration emit structured operation records. Logs disclose byte or item
counts, fixed failure categories, and unresolved-progress flags. They exclude
document content, diagnostic messages, provider identifiers, and error text.
Business values and serialized documents retain their existing contents.

## Verification

Run the workspace checks in the [domain README](../../domain/README.md#verification).
Focused fixtures cover unassessed drafts, agent-declared coverage, private context, strict decoding, and exact progress restoration.
[Conversation tests](tests/conversation.rs) validate version 1 and version 2 documents and written documents against their schemas.
They also cover verbatim position round trips, canonical positions, freshness differences and strict decoding.
[Review document tests](tests/checkpoints.rs) cover current fixtures, generated drafts, completion and revision.
