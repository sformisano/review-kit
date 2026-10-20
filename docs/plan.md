# Implementation status

The [decisions](decisions.md) define accepted scope. The [workflow](workflow.md)
defines agent responsibilities and compatibility limits.

## Current status

The GitLab workflow is implemented through `rkc` review runs: capture, checkpoint
checking, acceptance and revision, planning, publication and recovery, local
completion and explicit abandonment. Trusted configuration, runtime files and commands live in
the [CLI guide](../crates/bin/cli/README.md).

| Component | Implemented responsibility | Contract and verification |
| --- | --- | --- |
| Observation | Synchronous redacted operation logs with parent correlation and explicit effect states | [Logging contract](../crates/lib/observation/README.md), actual JSON, library and CLI tests |
| Generated observation | Application, adapter and CLI operation declarations with managed application ports and source architecture guards | [Boundary contract](../crates/lib/observation/README.md#generated-operation-boundaries), runtime, compile-fail and architecture checks |
| Domain | Values, findings, assessments, transitions and policy judgments | [Domain README](../crates/lib/domain/README.md) and unit tests |
| Application | Discovery, checkpoint validation, conversation comparison, aggregate checking, publication, run binding and settlement, follow-ups and recovery | [Application README](../crates/lib/application/README.md) and fake-port workflow tests |
| GitLab adapter | HTTP transport, provider observations, positions, committed source, retained source exports and conversation capture | [GitLab README](../crates/lib/infrastructure/gitlab-adapter/README.md), controlled HTTP tests and opt-in [CE suite](../tests/gitlab/README.md) |
| JSON adapter | Versioned context, assessment, run, checkpoint, conversation, journal, evidence and result documents | [JSON README](../crates/lib/infrastructure/json-adapter/README.md) and document tests |
| Filesystem adapter | Review-run storage and locking, no-overwrite drafts, atomic replacement, intent storage and the run-bound journal | [Filesystem README](../crates/lib/infrastructure/filesystem-adapter/README.md) and storage tests on temporary Git repositories |
| json2md adapter | Embedded and external profiles, offline export and versioned structured rendering inputs | [Renderer README](../crates/lib/infrastructure/json2md-adapter/README.md) and rendering fixtures |
| CLI | Arguments, configuration, review runs, wiring, structured output and exit status | [CLI README](../crates/bin/cli/README.md), controlled-HTTP process tests and [CLI acceptance fixtures](../tests/cli/README.md) |

This table records implemented surfaces and their proof commands. It does not
claim that an opt-in suite ran in every checkout. Use the
[workspace checks](../README.md#workspace-checks) and suite instructions for
current execution evidence.

## Verification boundary

The application suite exercises failure and recovery through fake ports, including
checkpoint rules, run binding, settlement and the release of unsent owners.
Controlled HTTP tests exercise actual request mapping and transport outcomes.
The local GitLab CE suite exercises real stored results, reviewer tokens,
conversation capture with note author accounts and the write targets that
freshness compares, on its pinned self-managed image.

The ordinary workspace tests run each review-run command as a process against
temporary Git worktrees and a controlled HTTP GitLab server. They cover the run
lifecycle, assessment acceptance and revision, successor, hash and identity checks,
freshness preflight, journal ownership and complete public declaration retention.
CLI process tests exercise `--retry-uncertain never` across restarts, preserving
unresolved intent and retry budgets without resending an absent creation.
The application and GitLab HTTP suites also test that conservative policy.
The [crash-point tests](../README.md#workspace-checks) abort `rkc` at each named
boundary of acceptance, run start, revision and publication, then continue from stored state.
These fixtures do not simulate power loss, synchronization failures or several hosts.

The opt-in [CLI E2E suite](../tests/cli/README.md#repeatable-cli-suite) drives review runs
(`run start`, one complete assessment acceptance, plan and publication) against disposable local GitLab merge requests
with scripted judgments. The repository records no live or fresh-agent results; run the suites
for current evidence. Fresh agent reviews are the separate evidence
that reviewers can assess source and author valid checkpoints.

Accepted checkpoints prove declared coverage consistency. No test establishes that
an agent discovered every applicable policy or reasoned correctly.
Other GitLab versions, hosted deployments and token configurations require their
own authorized verification. Provider observations establish matching state without proving request causation.
No suite turns multi-effect publication into a transaction.

Process tests kill and restart the application with real HTTP and filesystem adapters.
They cover preparation, dispatch checkpoints, readback, retry limits and operating-system lock release.
CLI subprocess tests cover progress during blocked HTTP and structured stdout.
These fixtures do not prove hosted GitLab behavior.

The [state compatibility contract](render-profiles.md#retained-state-compatibility)
lists the document and journal versions each reader accepts.
Review Kit makes no drop-in compatibility promise for other review tools.

## Distribution and support limits

The [binary release guide](distribution.md) defines the `0.1.1` Linux x86-64
package, license notices, installation, upgrade and rollback procedures.
Archive assembly binds artifacts to a clean source commit and records checksums.
The [distribution gates](distribution.md#distribution-gates) require checks against
those exact bytes.
The accepted [release scope](decisions.md#version-010-public-release) distributes
the binary through GitHub Releases. Provider verification uses local GitLab merge requests.

Additional deployments and permission configurations require their own verification.
Crates.io publication and GitHub review support are outside the current scope.
