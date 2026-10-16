# review-kit-domain

This crate owns review values, findings and assessments.
The [boundaries](#boundaries) define its dependencies and I/O limits.

The [local teaching page](../../../docs/learn/index.html) explains this model through
interactive examples. Open it in a browser, or start with the model below and
follow the source reading order.
[Follow one review](../../../docs/guide.md) shows the publication lifecycle.

## How the model fits together

Suppose an agent finds that an error path drops a request instead of retrying it.
It records the problem as F1, names the affected file and line, and explains the
failure. It marks F1 open and blocking because the code needs a fix.

### A finding and its values

That problem is a `Finding`. The agent supplies its fields in a `FindingDraft`.
`Finding::new` checks them before returning a finding. For example, it rejects
missing evidence. The finding and assessment rules are listed
[below](#findings-and-assessments).

Some fields have their own types. `FindingId` holds the number 1.
`SourceLocation` holds a `SourcePath` and a `LineNumber`.
`Severity` records how severe the problem is; `Disposition` records whether it
blocks the review. An optional `Suggestion` holds the original lines and their
proposed replacement.

These are value types: they describe an ID, a location, or a judgment.
F1 is an entity because it remains the same finding after the author fixes it.
Calling `transition` changes its status and evidence while keeping its ID.
The returned `FindingTransition` records the old status, new status, and explanation.
It does not record a published reply.

Two versions of F1 can have different statuses and still describe the same
problem. Comparing their IDs tests identity. Comparing the findings with `==`
tests all their fields, so the open and fixed versions differ.

### The rule behind F1

F1 also carries a `PolicyId`, naming the review rule it relates to.
The agent reads the linked policy skills and declares which IDs apply to the review.
The domain keeps those IDs and their results without loading policy definitions.

For each selected rule, the agent supplies a `PolicyEvaluation`: satisfied,
violated with evidence, or blocked with a limitation.
That evaluates the rule. F1 separately records the particular problem found in
the code. Both refer to the rule by the same `PolicyId`.

### The whole review

The agent puts F1 and the policy evaluations into an `AssessmentDraft`, together
with its overall decision, review depth, scope, and any limitation.
`Subject` names the change request, `ReviewerId` names the reviewer, and
`Revision` names the reviewed code.

`Assessment::new` checks that those fields agree.
It rejects a ready decision while F1 is open and blocking.
It also requires an evaluation for every selected policy and rejects duplicate
finding IDs. It returns the assessment or the consistency errors it found.
These checks cannot establish that the agent's bug report is correct.

The resulting `Assessment` is a value type: a record of the judgments for that
reviewed revision. It owns its findings and evaluations and exposes no mutation
methods. Changing a separate copy of F1 leaves this assessment unchanged.
The agent creates another assessment when reviewing the fix, keeping F1's ID.

The assessment records the policy IDs declared by the agent.
The application carries their display names in
[`AssessmentPolicies`](../application/src/use_cases/assessment/coverage.rs).

## Source reading order

1. [Finding](src/entities/finding.rs): read `FindingDraft`, `Finding::new`, then `transition`. Its tests show F1 keeping its ID after a status change.
2. [Assessment](src/values/assessment.rs): read `AssessmentDraft`, `Assessment::new`, then `validate_decision`. The [tests](src/values/assessment/tests.rs) show which combinations pass or fail.
3. [Application assessment use cases](../application/src/use_cases/assessment/mod.rs): `prepare` leaves judgments for the agent to fill; `complete` constructs the assessment.

The [source values](src/values/source.rs) and
[review identity values](src/values/review_identity.rs) explain the fields used
by findings and assessments.
For publication, continue with the [application README](../application/README.md).
Rounds, GitLab observations, and write operations belong to that layer.

## Values

| Type | Contract | Does not establish |
| --- | --- | --- |
| `FindingId` | Positive `u64` finding number, ordered numerically | Global identity, allocation, or membership in a review |
| `PolicyId` | Nonblank text, preserved exactly | Policy existence, selection, or a particular slug grammar |
| `LineNumber` | Positive `u64` source coordinate, one-based | File existence, bounds, or a valid diff position |
| `Severity` | P1 through P5, independent of disposition | Finding validity or readiness |
| `Disposition` | Blocking or advisory | A policy result or provider thread state |
| `Subject` | Provider, host, repository, and positive request number | Resource existence or authorization |
| `ReviewerId` | Nonblank, exact actor identity within the subject's host | Authenticated authorship |
| `Revision` | Nonblank head and exact ordered comparison references | Current provider state |
| `SourcePath` | Portable relative path without traversal or control characters | Filesystem containment or source existence |
| `SourceLocation` | Repository path and one-based anchor | A valid provider diff position |
| `Suggestion` | Nonempty original range, in-range anchor, exact lines, revision, correction evidence | That the correction is correct or publishable |

Constrained values have private storage and fallible construction.
Accessors expose immutable contents. Value conversion errors carry no input text.
`FindingId` and `LineNumber` are distinct types even though both contain positive numbers.
The containing review supplies the subject and reviewer scope for finding IDs.

Nonblank policy IDs retain whitespace, case, and Unicode exactly.
Membership checks compare exact IDs within the agent-declared assessment coverage.
Source paths preserve valid filename whitespace, including whitespace-only components.
Their validation does not establish compatibility with every host filesystem.

## Findings and assessments

`Finding` owns its stable ID, policy, source location, judgment, and evidence.
Its draft is unrestricted construction input; `Finding::new` requires title, body,
and evidence and checks that a suggestion shares the finding's anchor.
Titles exclude carriage returns, line feeds, `|`, `<`, and `>`.
Other Unicode separators and control characters have no additional domain restriction.
Output adapters enforce the requirements of their display or serialization format.
`transition` requires an explanation, rejects a repeated state, and preserves identity.
Open, fixed, withdrawn, and uncertain states describe the judgment.
Recurrence is a transition from fixed or withdrawn to open.
Severity and blocking disposition remain independent.
Policy evaluations and finding judgments are separate agent inputs.
An open blocking finding can coexist with a satisfied policy evaluation; the tool does not infer either judgment from the other.

`PolicyEvaluation` accepts satisfied without additional prose, violated with evidence,
or blocked with a limitation. `Assessment` requires one evaluation per selected policy,
unique finding IDs, selected policies for active findings, and open findings' suggestions on its reviewed revision.
Fixed or withdrawn findings can retain historical policies outside the current selection.
The application verifies that these are owned historical findings whose policy identity is unchanged.
Fixed, withdrawn, and uncertain findings
can retain historical suggestions; reopening makes the current revision check apply again.
Construction returns all independent consistency errors together.

The agent supplies the review decision, depth, scope, and any overall limitation.
Depth and scope are nonblank text with no fixed vocabulary. Their exact text is retained.
Construction validates the supplied decision without replacing it.
Ready cannot coexist with a violated policy or an open blocking finding.
Unavailable policy evaluations and uncertain or unvalidated findings require blocked.
The validation requirement also applies to fixed and withdrawn findings.
A blocked decision needs a nonblank overall limitation; it may describe missing context
beyond the selected policy evaluations. Supplied limitations must be nonblank for every decision.
Established violations and their evidence remain present when the decision is blocked.
Changes required remains an explicit agent judgment even when individual policy results are satisfied.
The validation flag records a claim and cannot prove independent review; for open and uncertain blocking findings the [confirmation rule](../application/src/use_cases/checkpoints/README.md#finding-confirmations) sets it.
`same_content` compares two versions of a finding in every field except that flag.

## Policy coverage

The agent owns applicability and review criteria. It declares policy IDs in
`AssessmentDraft::selected_policies` and supplies one evaluation for each ID.
The domain validates duplicate IDs, missing evaluations, finding membership,
and decision consistency. It cannot establish whether coverage is sufficient
or whether the supplied judgments are correct.

## Boundaries

The crate performs no I/O. Error derives follow the
[inner-layer dependency choices](../../../docs/decisions.md#inner-layer-choices).
It has no serialization, storage, provider client, clock, or runtime dependency.

The private [`values`](src/values.rs) module contains objects defined by their properties:
identifiers, judgments, source constraints and assessments.
The private [`entities`](src/entities.rs) module contains `Finding`.
A finding retains its scoped ID across reassessments.
Construction drafts and errors stay beside the types whose constraints they describe.
The crate root re-exports all public types without exposing the internal module layout.

Entity identity and exact state comparison serve different purposes.
Entity `PartialEq` and `Eq` compare complete state, as documented on their types.
Publication recovery uses these comparisons to detect changed retained intent.
Identifier values express identity comparisons within their enclosing scope.

Text parsing, JSON mapping, `F1` formatting, and provider limits belong at their consuming boundaries.
Pure value validation cannot establish facts that need source or provider observations.
Applications supply those facts through explicit calls, including exact committed lines
for `Suggestion::verify_original`. That comparison preserves whitespace and carriage returns.
The source adapter must read the requested range from the reviewed committed object.

The layout follows private state, validated inputs, typed rejections, and named domain operations.
There is no event store or persistence lifecycle because reviews do not need one here.

## Verification

Run the [workspace checks](../../../README.md#workspace-checks) from the workspace root.

Unit tests cover identity scope, numeric boundaries, exact source comparisons, invalid ranges,
finding transitions, assessment completeness and decisions.
No external service or database is required.
