# Collect context for a review

A discovery bundle gives the reviewer the change and the retained conversation
needed for an initial review or follow-up. It combines provider observations,
exact diff text, and verified communicated finding state.

Use `discover_bundle` before initializing a draft or deciding which findings
need reassessment. Basic `discovery::discover` supplies the observed snapshot;
this use case adds change collection and history reconciliation.

## Inputs and result

The caller supplies a review reader, a `ReviewChanges` implementation, the
requested subject, and the expected authenticated reviewer. Changed-file records
distinguish added, modified, deleted, and renamed paths.

`DiscoveryBundle` retains the latest snapshot, exact revision-bound changes,
owned findings in numeric order, and other authors' replies on owned finding threads.
It also supplies `next_finding_id` and any `recovery_conflicts`.

Finding reconciliation requires the exact verified transition reply before
accepting a changed judgment. Completed communication must retain its required
roots and action replies. Summary assessments alone cannot prove those effects.

The next finding ID follows the highest retained owned identity, including
unpublished summary intent and surviving action markers. Exhaustion fails
instead of wrapping or reusing an identity.

## Example

An agent resumes a partial review. The bundle supplies the changed files,
the last verified judgment for each owned finding, and other authors' replies on owned finding threads.
If a retained question was edited, `recovery_conflicts` identifies its target.
The bundle remains readable so the agent can inspect the conflict; publication
still rejects replay against the changed target.

## Boundaries

The use case reads discovery before and after fetching changes at the initial
revision. A wrong subject or changed revision fails collection. These reads
do not lock the provider against later edits.

Reader, change-source, and retained-history failures return `BundleError`.
Changed answer targets in the current unfinished revision are reported separately.
No provider write occurs. Both read ports can use network access.
The bundle does not authorize publication or establish that an unseen write never occurred.
