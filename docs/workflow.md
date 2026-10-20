# Review workflow

The libraries and `rkc` separate agent judgment from checking, rendering and publication.
The [implementation status](plan.md#current-status) owns component status and verification boundaries.

## Agent and tool responsibilities

The agent supplies the decision, depth, scope, limitations, policy judgments, findings, and transition explanations.
It records selection, relevance and judgments together in one assessment.
Each policy appears once with its required flag, relevance reason and evaluation result.
Excluded policies retain a reason; unreadable criteria or missing proof remain explicit gaps.
The same document contains findings, carried reassessments and needed communication.
The [CLI assessment contract](../crates/bin/cli/README.md#assessment-editing) defines these inputs.

The tool freezes the complete accepted assessment and checks its run, revision and successor binding.
It derives subject, reviewer and immutable prior findings from the capture, without duplicating them in the authored file.
It reads no policy definitions. Acceptance proves declared consistency, not complete discovery or correct reasoning.
The tool owns round allocation, action identities, publication state, links, counts, and rendered layout.
The checker and publisher consume the same accepted assessment and retained capture.

Satisfied policies need no separate written justification.
Violated policies require evidence; unavailable evaluations require a limitation.
The assessment accepts `satisfied`, `violated`, and `blocked` policy entries.
`blocked` means evaluation is unavailable, not that the policy is violated or satisfied.
Keep established violations when another evaluation is unavailable.

The tool checks assessment consistency; it does not invent findings, choose severity, or perform independent review.
It preserves the agent's decision after validating that decision against the supplied facts.
Fixed, withdrawn, recurrence, and same-state reclassification replies require authored explanations.
Private `local_context` is excluded from public review content.
The [feature contract](features.md#responsibilities) defines when a finding counts as independently checked.

## Command contract

The [CLI guide](../crates/bin/cli/README.md#review-runs) owns implemented command
syntax, configuration, JSON examples, runtime files and exit status.
`rkc` provides review-run capture, checkpoint checking, acceptance and revision,
read-only planning, publication and recovery, local completion and explicit abandonment.

Review Kit defines its own versioned command and document contracts.
It makes no compatibility promise for other tools' commands, schemas, markers or exit behavior.
A run retains an immutable capture of the context, committed source, diff and supported conversation.
Its manifest records the SHA-256 hash of each captured file and the current accepted assessment reference.
Every reference inside a run resolves against the run directory.
Changed captured or accepted bytes fail with `hash_mismatch`, even at the same revision.
A changed merge request needs a new run; a run is never recaptured.

The effect set accepts `review`, `none`, or an explicit list of provider effects.
It cannot expand trusted host authority.
Read-only modes perform no provider writes.
The run use case sends an empty effect selection through planning without acquiring a publication session.
The [application coverage table](../crates/lib/application/README.md#use-cases-and-command-coverage) identifies the implemented Rust APIs.
The [CLI exit codes](../crates/bin/cli/README.md#output-and-exit-status) distinguish
invalid input, changed revision or write targets, authority limits, conflicts and provider uncertainty.
Exit zero can carry a non-completed publication status; consumers must read the structured result.
New advisory findings remain eligible for inline publication under the finding admission rules.

## Publication behavior

The core owns the sequence and recovery rules in [architecture.md](architecture.md).
The GitLab adapter supplies provider facts and executes individual effects.
Ownership comes from API authorship, never marker text.
The pending summary and each following effect require verified readbacks before progression.
An identical pending summary is verified without rewriting it.
Summary creation and summary update require their respective authority.
Blocked assessments can communicate individually validated findings whose own policy evaluation is complete.
Unvalidated or uncertain candidates generate no finding effects.
An unrelated unavailable evaluation keeps the final summary blocked.
The round stays pending or partial until its required communication is verified.
Repeating the same freshly assessed blocker on the same revision writes no duplicate warning.
An incomplete-status round with no finding actions can finish in place when its assessment becomes complete.
A terminal blocked round with finding actions retains its history; changed communication starts a successor round.

A completed, freshly assessed review whose communication is unchanged produces zero writes and no new round.
The [application recovery contract](../crates/lib/application/README.md#publication-and-recovery) defines structural comparison and retained intent.
Recover a matching pending or partial attempt without repeating verified effects.
Recovery requires the retained assessment, answers, transition explanations, targets, and action bodies.
Caller-carried progress retains unresolved write intent across publication calls.
The CLI keeps that progress in one journal per subject and reviewer and binds it to the
run whose accepted assessment it publishes. That run recovers its unfinished publication;
the [binding contract](../crates/lib/application/README.md#review-run-binding) defines when another run can replace it.
Exact observed effects can support continuation without another write.
The [CLI recovery options](../crates/bin/cli/README.md#publication-and-recovery)
control one bounded resend of an absent creation and retain the original round's authority.
Hosts that collect long-running command output receive [plain progress](../crates/bin/cli/README.md#progress-output) by default.
