# Check one review declaration

A review run has one complete assessment and one accepted content reference.
Each policy row contains its declaration and evaluation outcome once.
The host freezes the exact document bytes after full assessment checks.

## Inputs and results

`validate_declarations` checks a `ReviewDeclarationsDraft` against a `ReviewBinding`.
The binding supplies the run, reviewed revision, captured findings and a `Successor` rule.
It returns validated `ReviewDeclarations` or independent `ReviewIssue` values.
The host owns decoding, hashing and storage.

`Successor::Current` requires `supersedes` to name the current accepted review, or remain absent before first acceptance.
`Successor::Retained` validates an already accepted snapshot, including an exact acceptance retry.
There are no intermediate inventory or applicability checkpoints.

## Rules

- Run and revision match the retained capture; policy set and review reason are nonblank.
- Policy rows have unique nonblank IDs and names.
- Every open or uncertain captured finding keeps its policy selected.
- Each row has a nonblank public relevance reason and a completed outcome.
- Required policies cannot be excluded.

`PolicyOutcome` distinguishes excluded, unresolved, satisfied, violated and blocked results.
It derives public applicability and the assessment judgment without a second authored row.
Excluded policies have no judgment. Unresolved policies remain blocked.
Applicable policies may also be blocked by unavailable evidence.

[Assessment completion](../assessment/README.md) checks findings, decisions and communication inputs.
An active carried finding cannot disappear through policy exclusion; closing it requires its current reassessment and transition explanation.
The JSON adapter obtains immutable previous findings from captured context.

## Finding confirmations

`admit_findings` applies recorded `Confirmation` verdicts to the final findings.
An open or uncertain blocking finding, per `requires_confirmation`, is validated exactly when a confirmed verdict covers it:
`Confirmation::Checked` holds a version equal to the final finding apart from validation,
or `Confirmation::Retained` names a finding equal to a captured version in `reusable`.
`reusable_confirmations` derives those versions from the reviewer's rounds marked `confirmed_validation`
at the snapshot's revision: such a round validated exactly the captured content.
Without that evidence such a finding is unvalidated, and a validation claim is a `ConfirmationIssue`.
Other findings keep their authored validation.
Each finding has at most one verdict, with a nonblank basis. A confirmed verdict must name a final finding and, when that finding requires confirmation, cover its exact version;
rejections and evidence gaps are records that admit nothing.
The rule compares versions only. The verdict and the reuse of a retained check remain agent judgments.

The JSON adapter's `AssessmentDocument::complete` applies this rule before assessment completion,
and its `PublishRequest` sets `confirmed_validation`, which the planned round records.
Lower-level completion, such as the JSON adapter's `DraftDocument::complete`, takes findings with validation already decided;
its requests leave `confirmed_validation` false.

## Boundaries

These checks perform no I/O and read no policy prose.
They prove declared consistency, not complete policy discovery or correct reasoning.
The host retains immutable captures, exact accepted bytes and successor history.
Publication separately checks freshness, authority, ownership and recovery state.
