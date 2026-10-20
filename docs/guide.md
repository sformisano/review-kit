# Follow one review

An agent reviews a GitLab merge request and finds a bug. Review Kit records the
agent's declarations and judgments in one assessment, checks their consistency,
and coordinates publication of the review comments.

The [CLI overview](../README.md#cli-usage) describes the tool's commands and outputs.
The [CLI reference](../crates/bin/cli/README.md) defines configuration
and document details. This walkthrough explains the rules
behind those commands. The [implementation status](plan.md#current-status) records
implementation status and verification limits.

## 1. The agent records a finding

Suppose an error path drops a request. The agent identifies the failing code and
records why it needs a change.

A **finding** is one reported problem, with an ID, source location, judgment,
and evidence. In this example, the agent calls it F1:

| Part | Example |
| --- | --- |
| Finding ID | F1 |
| Problem | The failure path drops the request |
| Evidence | The error branch exits without retaining or returning the request |
| State | Open |
| Disposition | Blocking: the agent requires a fix before marking the review ready |
| Review decision | Changes required |

The agent includes F1 in an **assessment**, the record of its review at a
particular code revision. That record also includes review scope, depth,
policy evaluations, and any limitation.
A **policy** is a named review rule whose criteria the agent reads from a skill.
The agent finds the policies, decides which apply, and evaluates them.

Review Kit checks the supplied judgment for consistency.
The agent supplies the evidence that the code is defective.
The [responsibility contract](workflow.md#agent-and-tool-responsibilities)
defines this division of work.

## 2. A review run freezes what the agent assesses

The agent assesses a capture of the merge request. `rkc run start --mr URL` stores
the reviewed revision, committed source, diff, the reviewer's earlier review comments
and the supported conversation as a **review run** in the reviewer's Git worktree.
Later checks read this capture, not the live merge request.

The agent writes one `review-assessment.draft.json`.
Each policy row contains its name, why it applies and its evaluation outcome.
The same file contains findings, current reassessments and the overall decision.
F1 is open and blocking, so an isolated checker examines its exact version, and the assessment records that verdict in `confirmations`; without it, only a blocked decision is valid.
The [confirmation contract](../crates/bin/cli/README.md#finding-confirmations) defines these entries.
`rkc accept` validates the complete document and freezes its exact bytes under their SHA-256 hash.

A material correction uses `rkc revise --reason TEXT`, then another acceptance.
The successor names the previous accepted assessment.
Publication requires the current draft to equal its accepted snapshot.

Acceptance proves declared consistency with the retained run.
It cannot establish complete discovery, correct relevance decisions or correct judgments.
The [assessment acceptance contract](../crates/bin/cli/README.md#assessment-acceptance) defines these checks.

## 3. Review Kit checks the record and source

The library checks the agent-declared policy results and the review decision.
Each applicable policy needs a judgment. A ready decision cannot accompany an
open blocking finding. The agent owns selection; the library reads no policy definitions.

The application checks new finding locations against committed source and the
reviewed diff. A suggestion also needs original lines that match the committed
code exactly.
These checks establish record consistency and source positions; the agent
remains responsible for the proposed correction's validity.
`rkc check` runs them without accepting the draft;
`rkc accept` runs them and freezes the assessment.

The [domain contract](../crates/lib/domain/README.md#findings-and-assessments)
defines assessment rules. The
[application contract](../crates/lib/application/README.md#discovery-and-source)
defines source checks.

## 4. Preview the comments, then publish

The library compares the accepted assessment with current GitLab review state.
It identifies comments already published by the authenticated reviewer and
plans what remains to be done.
An **effect** is one write to GitLab, such as posting F1 or updating the summary.
A **round** records an assessment and its publication progress in review history.

A read-only plan shows the required effects and comment bodies where their
inputs are known. A link to a new finding remains unresolved until GitLab
returns its reference. Planning performs no provider writes.

Before the first write, the CLI compares the current head and comparison with
the run's capture. It also compares, with their captured state, the summary note
and each finding thread where this publication will reply, resolve or reopen.
If the author pushed a commit, someone edited the summary note, or someone
replied in, edited or resolved one of those threads, publication stops before
writing anything. The agent assesses the change in a new run. Replies to the
summary, replies in the reviewer's other threads, comments elsewhere by people
or bots, and title or description edits do not stop publication; the next
review assesses them. Publication communicates the assessed snapshot, not later
events.

For publication, assume the capture is current, no owned summary
exists, and the caller grants the required write authority.
Assume each GitLab operation succeeds and its result is verified:

| Order | Verified operation | Review state |
| --- | --- | --- |
| 1 | Create a pending summary | The round records unfinished publication |
| 2 | Post F1 at its checked source position | One owned finding exists |
| 3 | Update the summary with the verified finding link | The round is completed |

The application reads back each effect before continuing.
It checks the current revision, target, and authority before writes.
The publication journal then records that this run settled as `published`, and
the run is completed.

The final review decision remains changes required. Publication is completed
because its required comments are verified. Review decision and publication
status answer different questions.
The [publication sequence](architecture.md#normal-publication) gives the full
operation order and its limits.

## 5. Run the same review again

Publishing the completed run again reports `already_settled` and writes nothing.

Suppose a new run captures the same revision, and its completed assessment
communicates the same review. The library verifies the existing review and
returns unchanged. It writes no comments and allocates no new round.
The run completes as `no_changes`. F1 still identifies the same finding.

This depends on current provider observations, including ownership and intact
published effects. The
[comparison contract](../crates/lib/application/README.md#publication-and-recovery)
defines which changes require another round.

## 6. Follow the author's fix

The author changes the code. `rkc run start` starts a new run for the selected
run's merge request. Its capture carries F1 as an earlier finding with its
communicated state.

F1 is still open, so declare its policy `error-handling` in the new assessment;
acceptance otherwise fails with `missing_active_finding_policy`. The agent reassesses F1 as fixed and supplies
an explanation for that transition. Assume the policies are now satisfied and no
other blocking problem remains. The agent supplies a ready decision.

The library keeps F1's identity and existing discussion.
It posts the agent's explanation before resolving that discussion, then
completes the summary for the new round. The reply adds no new finding.
The [transition contract](../crates/lib/application/README.md#assessment-boundary)
also covers withdrawal, recurrence, and reclassification.

## 7. Handle a lost response

Suppose GitLab stores F1, but the connection fails before returning its response.
The publication journal for this merge request and reviewer already names this run
and holds the unresolved write. Running `rkc publish --effects review` again on the
same run looks for that exact owned effect. If discovery verifies it, publication
can continue without posting F1 again.

If F1 is absent after complete reconciliation, the default policy permits one
controlled resend. That retry is retained across restarts; a late original can
still create a duplicate. The conservative option leaves the absent effect unresolved.

Suppose a summary update succeeds but its response is lost. Recovery can accept
the exact observed summary without sending the update again.
It records the observed result without claiming which request caused it.

While this run owns the unfinished publication, another run for the same review
can draft and finish locally, but its publication fails with `publication_pending_owner`.
If the process stopped before any write could leave, nothing reached GitLab.
The next run's publication can then release this run's binding.

Recovery uses the run's frozen accepted assessment and its capture, together with
the journal. To continue in another container or machine, copy the run and the
journal consistently and keep a single writer; Review Kit has no distributed lock.
The [recovery contract](../crates/lib/application/README.md#publication-and-recovery)
defines these obligations. The
[CLI recovery reference](../crates/bin/cli/README.md#publication-and-recovery)
defines the commands and the journal files.

## Where to go next

Read the [feature contract](features.md) for product requirements and limits.
Use the [documentation index](README.md) to choose a reference for your task.
For the types behind this example, start with the
[domain model](../crates/lib/domain/README.md#how-the-model-fits-together) and its
source reading order.

For examples in code, the
[application workflow tests](../crates/lib/application/tests/review_workflow.rs)
exercise initial publication, unchanged reruns, fixes, and lost responses.
The [CLI review-run tests](../crates/bin/cli/tests/runs.rs) run these commands
against temporary Git worktrees and a controlled HTTP GitLab server, including a fixed follow-up.
The [GitLab HTTP tests](../crates/lib/infrastructure/gitlab-adapter/tests/http_workflow.rs) exercise real
request mapping against a local fixture server.
