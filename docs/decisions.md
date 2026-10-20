# Decisions

## Operation logging

- Use a shared synchronous observer for meaningful application and adapter operations, including internal stages.
- Use slog, slog-json and Redactable with nested JSON values and explicit disclosure projections.
- Keep CLI log delivery separate from command-result stdout under the [logging preferences](../crates/bin/cli/README.md#logging-preferences).
- Make library logging opt-in through a scoped observer.
- Keep logging independent of publication authority, recovery and mandatory execution evidence.
- Exclude remote delivery, OpenTelemetry, traces and metrics from the logging scope.

The [logging contract](../crates/lib/observation/README.md) defines implemented fields, coverage and failure limits.

Operation declarations govern execution boundaries:

- Generate meaningful application, adapter and CLI boundaries from complete operation declarations.
- Generate managed port facades from provider, source, renderer, changes, draft and evidence contracts.
- Bind ports at public application ingress and reuse managed handles internally.
- Use synchronous public APIs and preserve domain invariants, cache behavior and publication authority at observation boundaries.
- Check production source declarations and raw-port ownership with [architecture guards](../crates/lib/application/tests/operation_architecture.rs); review helper exemptions and disclosure policies.

## Accepted direction

- Name the project and repository `review-kit` (Review Kit), with `rkc` as the executable name.
- Use `review-kit-domain` and `review-kit-application` for the implemented inner-layer crates.
- Provide Rust libraries and a CLI with a clean architecture core and adapters.
- Support publication through the GitLab adapter only.
- Keep pure review rules and publication orchestration in the core.
- Let adapters implement core-owned interfaces; let the CLI wire the implementations together.
- Use the [compact review workflow](workflow.md) as the baseline.
- Let the agent select and assess applicable policies; keep assessment consistency validation in the core.

The implementation scope includes a pure domain crate, an application crate with mocked external calls, and a separate GitLab adapter with real HTTP transport.
The [domain](../crates/lib/domain/README.md) contains values, findings, assessments, and policy rules.
The [application](../crates/lib/application/README.md) contains use cases, ports, reconciliation, and publication.
Neither crate depends on an application framework or database.
The [GitLab adapter](../crates/lib/infrastructure/gitlab-adapter/README.md) implements provider and source ports.
The [implementation status](plan.md#current-status) records the components.
Product scope does not authorize live provider writes, package publication, or release hosting.

## Agent-owned policy assessment

The agent reads a named policy-list skill and its linked skills, decides applicability,
and evaluates their criteria. Ordinary links and prose carry policy definitions,
requirements and prerequisites. No policy JSON companion is required.

Review Kit consumes the agent's declared policy names, IDs and results. It validates
assessment consistency without loading definitions, expanding dependencies or imposing
fixed baseline names. This removes the catalog coupling and makes omitted-policy
detection the agent's responsibility. Missing required assessments remain explicit gaps.
The agent declares policies in the [single review assessment](#review-runs-and-checkpoints).
The [CLI assessment contract](../crates/bin/cli/README.md#assessment-editing) defines the boundary.

### Blocked review communication

A blocked review can communicate individually validated findings under completed policy evaluations while unrelated evaluations remain unavailable.
Review readiness stays separate from verified communication.
The [application contract](../crates/lib/application/README.md#publication-and-recovery) owns eligibility, finalization and retained history.

## CLI and adapter delivery

- Keep libraries under `crates/lib` and the CLI under `crates/bin/cli`.
- Use json2md directly with embedded or external schemas and templates.
- Use versioned local JSON for retained discovery, editable assessments, publication progress.
- Provide source installation through Cargo.
- Preserve core-owned review rules and application-owned ports.

The [architecture](architecture.md) defines these boundaries.
The [CLI guide](../crates/bin/cli/README.md) defines the implemented command and
workspace contract. The [implementation status](plan.md) defines verification and support boundaries.
GitHub review support is outside the current scope.

## External rendering profiles and retained history

- Load caller-owned JSON Schemas and Jinja templates through the json2md library.
- Support findings, summaries, transitions, answers and suggestion blocks in one versioned profile.
- Export editable defaults without GitLab access or configuration.
- Accept explicit public workflow identity in trusted configuration and policy-set identity through the policy inventory.
- Retain complete finding versions, trusted policy names, profile identity and verified historical rows in lossless shared tables.
- Read and write version 3 provider state only; preserve version 1 provider markers.
- Refuse changed profiles during unresolved publication instead of replacing retained write intent.

The [profile guide](render-profiles.md) documents use.
The [renderer contract](../crates/lib/infrastructure/json2md-adapter/README.md) owns asset and input formats.

## CLI files and publication state

- Use one application home for default trusted configuration, operation logs and shared publication progress.
- Configure logging before argument parsing, independently of explicit review configuration selection.
- Preserve explicit JSON configuration and reject ambiguous default selection without automatically moving files or progress.

The [configuration reference](../crates/bin/cli/README.md#trusted-configuration) defines home paths, formats and selection.

`review-kit-cli` is one binary entrypoint that selects and wires infrastructure implementations.
The [component boundaries](architecture.md#components) define application ownership of draft models, use cases and storage ports.
The CLI selects a filesystem adapter to store draft assessments, rendered comments and publication results as the review progresses.
The GitLab adapter handles authentication, provider observations, positions, HTTP operations and transport outcomes.

Keep local workflow state visible in the review workspace.
The GitLab adapter has no lock or request-journal subsystem.
Application-owned `PublicationProgress` carries unresolved write intent across calls under caller responsibility.
The [application contract](../crates/lib/application/README.md#publication-and-recovery) defines retention and recovery limits.
The [CLI file contract](../crates/bin/cli/README.md#runtime-files) defines the
implemented filenames, hash references and retained source export.
The [filesystem adapter](../crates/lib/infrastructure/filesystem-adapter/README.md)
stores unresolved intent before dispatch and restores it across calls.
The CLI uses one configured subject/reviewer-scoped progress store across
review runs and worktrees. Drafts, execution evidence and results remain in their
run directories. The [CLI configuration](../crates/bin/cli/README.md#trusted-configuration)
defines the shared state location.
The CLI holds a filesystem lock while using shared progress for a subject and reviewer.
Separate state roots and machines remain the caller's serialization responsibility.

## Publication recovery and progress output

- Retain exact intent with separate prepared and possible-dispatch phases.
- Persist the original round's effect ceiling and each consumed ambiguous retry.
- Accept exact observed desired state while preserving observation provenance.
- Permit one controlled resend of an absent creation after complete reconciliation, with a conservative option.
- Discard, with a recorded event, a prepared write that can no longer resume: it never reached the provider. Never discard a write that may have been dispatched, and never declare one lost.
- Report retry risk explicitly; a late original can still create a duplicate.
- Keep progress and local locking in existing ports and adapters without a database or service.
- Use plain stderr progress lines by default, with an opt-in terminal-aware mode and an off mode.
- Keep progress independent of structured logs and machine-result stdout.

The [application recovery contract](../crates/lib/application/README.md#publication-and-recovery)
defines continuation limits. The [CLI reference](../crates/bin/cli/README.md#progress-output) defines progress behavior.

## Review runs and checkpoints

- Let `rkc` own review checkpoints. `run start` captures one merge request into a review run.
  One assessment combines policy selection, relevance, judgments, findings and communication inputs.
  Store each policy once and freeze the complete assessment with one acceptance.
- Identify captured files and accepted checkpoints by the SHA-256 hash of their exact bytes.
  Bind the assessment to its captured run and revision. Bind each successor to the assessment it supersedes.
- Store runs in `.review-kit/` at the root of the Git worktree that contains the invocation directory.
  Take no directory or file arguments. Keep the files out of Git and reject tracked, redirected or unrecognized runtime paths.
- Archive exact draft bytes before `revise` replaces a draft. Use `revise --regenerate` as the only way past a draft that does not decode.
- Require every policy of an active carried finding in the inventory.
  Allow its exclusion only when the assessment closes that finding.
- Keep policy definition paths and introducing sources out of the assessment; no reader or published comment uses them.
- Let a carried finding be decided as `"unchanged"`, so the agent restates only what changed.
- Validate an open or uncertain blocking finding only through a recorded confirmation of its exact final version,
  or reuse of an unchanged captured version whose confirmation an earlier round recorded at the same revision.
  Compare versions in the core; leave verdicts to the agent.
- Record in each accepted round that its blocking validation came only from confirmations, so later runs can
  tell reusable confirmations from earlier unenforced validation without reading local role records.
- Limit private `local_context` to 2000 characters of pointers and decisive facts.
- Capture the supported merge-request conversation as context only.
  Before the first possible dispatch, require the current revision to equal the capture and the reviewer's own threads to be unchanged:
  the discussion of its owned summary and its owned finding threads, as discovery verifies them from the API author and corroborated markers.
  Any change inside those threads blocks, because publication updates, answers, resolves or reopens them.
  Other people's and bots' notes, threads and resolutions, and title or description edits, do not block; publication never writes to them.
  Comparing the whole conversation is rejected: a later comment by another person or a bot then stops publication
  and forces a fresh assessment, which can reverse confirmed findings.
  Publication communicates the assessed snapshot, not later events.
- Retain each captured note author's username and the provider's bot status beside the numeric author ID,
  so review agents can tell bots such as GitLab Duo from people, also inside the reviewer's own threads.
  Write the conversation as version 2 and keep reading version 1, whose authors have no account details.
  Ask GitLab for the bot status once per distinct author at run start, within a fixed bound, and record an unknown
  username or status rather than failing or guessing when GitLab does not provide it.
  Compare authors by numeric ID only: usernames can change.
- Bind publication to one run in journal version 3, `publication-progress.json`.
  The binding names the run's accepted assessment and captured-file hashes and is installed with the first prepared write.
  A dispatch flag is set before the first request could leave.
- Record each run's settlement (`published`, `no_changes` or `abandoned`) in the journal before the run's manifest completes.
  Journal evidence overrides the local manifest.
- Create an empty journal only for a run that never attempted publication, by its manifest or its own execution events,
  and only when the provider shows no reviewer-owned state or the reviewer's only summary shows its latest round terminal.
  A follow-up review in a new environment therefore needs no earlier state after a finished round. Never recreate the journal of an attempted run.
- When another run's binding has dispatched, has no unresolved write and the provider shows its bound round terminal, record a provider-verified settlement for that run and admit the new run.
- When none of another run's writes could have reached the provider, let only a replacement run's admission release that binding.
  Release restores the effect ceiling that existed before the binding and keeps retries, receipts and settlements.
  The owner's own publication, abandonment and lifecycle commands keep the binding.
- When the first write of a newly installed binding is definitely rejected without a verified effect, restore the previous binding and flag.
- Current run publication reads only `publication-progress.json`; an existing key directory missing that journal refuses publication.
  Unrelated files remain untouched. Generic progress APIs refuse an existing current run journal.
- Review commands use the accepted review-run chain. Raw `--assessment`, `discover`, `init` and the separate `abandon` command are absent.
- Keep locks local. Continuing unfinished publication in another container or machine requires a consistent transfer of the run and its journal and one authoritative writer.
  Review Kit has no distributed lock.

The accepted assessment proves declared coverage consistency, not discovery completeness or correct reasoning.
Bounded recovery can still duplicate a creation whose original request arrives late.
The [review-run reference](../crates/bin/cli/README.md#review-runs) defines commands, files and lifecycle.
The [application binding contract](../crates/lib/application/README.md#review-run-binding) defines admission, release and settlement,
and its [empty journal admission](../crates/lib/application/README.md#empty-journal-admission) defines which provider history permits an empty journal.
The [filesystem adapter](../crates/lib/infrastructure/filesystem-adapter/README.md#review-runs) defines run storage and the journal files.

## Version 0.1.0 public release

- Tag `v0.1.0` and distribute the Linux x86-64 binary and checksums through GitHub Releases.
- License the workspace under `MIT OR Apache-2.0` and include both license texts.
- Use a Debian Bullseye build container and verify the archive on Rocky Linux 9,
  as defined in the [distribution guide](distribution.md).
- Identify the build by the source commit and executable checksum in `BUILD.json`.
- Verify installation from the public download before completing the release.
- Limit support claims to environments verified with the packaged executable.
- Keep the Rust crates unpublished on crates.io and provide no Homebrew package.

The [distribution guide](distribution.md) defines package contents, build identity,
installation and verification.
The version string alone does not identify compatible retained state; the
[state compatibility contract](render-profiles.md#retained-state-compatibility)
defines the supported retained-state formats and recovery requirements.

## Inner-layer choices

Use the [workspace layout](../README.md#workspace-layout) for library and binary crates.
Keep infrastructure as a directory of adapter crates, each with its own dependencies.

Use synchronous, borrowed application ports. Hosts with an asynchronous event loop schedule these calls on a worker.
Use one review publication path for initial reviews, follow-ups, and recovery.
Keep assessment and model invariants in domain; keep observations and effect orchestration in application.
The domain has no dependency on another project layer. The application depends on the domain.
Both crates use `displaydoc::Display` and `thiserror::Error` to derive standard error traits.
Publication planning uses `thiserror` display attributes for computed failure counts while retaining the detailed vectors.
These derive macros add no provider, storage, or runtime dependency to the core.

Use the standard `Debug` derive for ordinary types. Authentication data must
never enter logs or debug output. Credential types use `derive_more::Debug`
with field skips or an opaque representation, with only its `debug` feature
enabled. Server URLs, retained source and local review notes remain available
in debug output. Local notes remain excluded from published review content.

The agent supplies the decision, depth, scope, limitations, and transition explanations.
The tool validates these inputs and computes rounds, effects, counts, and publication status.
Agent-declared policy coverage remains bound to assessment completion, checking, and publication.
Completed communication uses structural comparison without an agent-assigned assessment key.
Pending recovery requires exact retained intent and corroborated provider effects.
The [application README](../crates/lib/application/README.md) defines these contracts and their tests.
Their versioned mappings are defined by the adapters.

## GitLab adapter choices

Use `review-kit-gitlab-adapter` for GitLab JSON, credentials, HTTP and public summary-state serialization.
Use reqwest's blocking client with Rustls, bounded reads, no redirects and no automatic retries.
Use API-confirmed authorship and small v1 bottom markers. Retain complete application state in one escaped HTML comment before the owned summary marker.
The exact format and supported API response shapes live in the [adapter README](../crates/lib/infrastructure/gitlab-adapter/README.md).
These records define Review Kit's own versioned contract.

The caller serializes publication for one review and reviewer.
Acquisition binds identity and authority without promising writer exclusion.
The application carries unresolved intent and the consumed retry budget in caller-supplied progress.
It reuses exact observed effects and controls eligible resends under the recovery contract.
Observed update settlement does not establish which request caused the state.
The [publication contract](../crates/lib/infrastructure/gitlab-adapter/README.md#publication-and-uncertain-writes) defines the adapter's HTTP limits.
Controlled HTTP verifies transport outcomes. The opt-in
[local GitLab CE suite](../tests/gitlab/README.md) verifies real server operations
with Developer `api` and `read_api` reviewer tokens on its pinned image.
Other server versions and hosted configurations remain unverified.

The application maintains no local database.
Evidence and clock ports retain run outcomes without performing file or clock I/O inside the core.
