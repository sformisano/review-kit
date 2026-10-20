# CLI acceptance fixtures

The Cargo integration tests run the production `rkc` process against private
GitLab merge requests. Fixtures use separate Maintainer author and Developer
reviewer identities. Review publication uses the Developer's `api` token.

## Repeatable CLI suite

Run the ordinary controlled-HTTP profile suite without Docker or credentials:

```sh
cargo test -p review-kit-cli --test render_profiles --locked
```

These tests execute the real CLI and json2md renderer through
[accepted review runs](../../crates/bin/cli/README.md#review-runs).
They verify export, all five custom roles, exact submitted bodies, history after
restart, profile changes during recovery, relative paths and safety checks under permissive schemas.

The review-run process tests also need no Docker or credentials:

```sh
cargo test -p review-kit-cli --test runs --locked
```

They run each review-run command against temporary Git worktrees and a controlled
HTTP GitLab server. The [crash-point tests](../../README.md#workspace-checks)
reuse that harness in a `fault-injection` build.

The opt-in suite below drives review runs against disposable local GitLab merge
requests; the [verification boundary](../../docs/plan.md#verification-boundary) states its limits.
Run it from the worktree:

```sh
cargo test -p review-kit-cli --test e2e managed_cli_workflow -- --ignored --exact --nocapture
```

The suite starts one disposable GitLab CE service through `testcontainers`.
Docker must be available. The [GitLab service guide](../gitlab/README.md) defines
the pinned image, readiness checks and external-server configuration.
The service owner remains alive until every scenario finishes.

Set `REVIEW_KIT_GITLAB_URL` and `REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE` to reuse
an existing loopback server. The suite leaves that server and its volumes running.
To verify an installed binary, also set `RKC_E2E_BINARY`:

```sh
REVIEW_KIT_GITLAB_URL=http://127.0.0.1:8929 \
REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE="$PWD/tests/gitlab/.local/admin-token" \
RKC_E2E_BINARY="$HOME/.cargo/bin/rkc" \
  cargo test -p review-kit-cli --test e2e managed_cli_workflow -- --ignored --exact --nocapture
```

Without `RKC_E2E_BINARY`, Cargo supplies its built `rkc` executable.
Ordinary `cargo test` reports these live tests as ignored.
Explicit invocation fails if a binary, credential, service or observation is unavailable.

For a focused case on the existing service, run one `external_` test by name:

```sh
cargo test -p review-kit-cli --test e2e external_initial_fixed_and_recurrent_review -- --ignored --exact --nocapture
```

The external cases default to `http://127.0.0.1:8929` and
`tests/gitlab/.local/admin-token`. The same environment variables override these defaults.
Run one suite or agent review at a time against an external service.

[Scenario actions](../../crates/bin/cli/tests/e2e/scenarios.rs) contain the
assertions. [Assessment inputs](../../crates/bin/cli/tests/e2e/assessments.rs)
state explicit judgments about the fixture's quantity and optional-audit defects.
The [fixture helper](../../crates/bin/cli/tests/e2e/fixture.rs) owns setup,
bounded process execution, cleanup and independent REST observations.
[Run steps](../../crates/bin/cli/tests/e2e/workflow.rs) give each review
workspace its own Git worktree and drive `run start`, one scripted assessment
draft, `check`, `accept` and `publish`.

The scenarios cover initial findings, a real suggestion, unchanged repetition
in a new run without provider writes, author answers, fixed-thread resolution
and recurrence on the same root from a fresh worktree. They verify retained
round history, a run accepted before a push that cannot publish the older head,
clean and blocked reviews, offline checks, malformed-draft regeneration, copied
retained files and assessments, private-body exclusion and actual Developer
`read_api` write rejection. [Authority controls](../../crates/bin/cli/tests/e2e/controls.rs)
cover planning, missing authority, reviewer mismatch, partial publication,
a retained unknown write that blocks another worktree's run, abandonment, a
subsequent round, and a run captured before the reviewer's summary changed
whose plan against the current history writes nothing, so it settles without
writes.
Controlled HTTP and adapter suites cover transport loss and source variants.
The managed suite also exports a custom profile and verifies every role against
GitLab, followed by an unchanged retry and a fix on a new commit.

The [conversation freshness scenario](../../crates/bin/cli/tests/e2e/scenarios/freshness.rs)
adds a Developer commenter and a project access token bot user. After each
capture they post notes, start, reply in and resolve a diff thread, copy Review
Kit markers into their notes, and the author edits the title and description.
Publication still completes with exactly its planned effects, and independent
REST reads show every foreign note unchanged. The retained
`review-conversation.json` names each author's username, with `author_bot`
`true` for the bot and `false` for the people. A round that answers only in
finding 2's thread still publishes after the author and the bot reply in
finding 1's thread, the author resolves it and a commenter replies to the
summary: REST reads show finding 1's thread unchanged, the reply to the summary
kept and the summary recording only the answered note. A reply or a resolution
change in a thread that the round answers in refuses planning and publication
with `conversation_changed` and writes nothing; a fresh run then publishes. A run
captured in another worktree before that publication, whose plan writes the
summary, is refused with the summary change.

Each scenario owns a unique private namespace and a `tempfile` directory under
ignored `tests/cli/.local/`. Success revokes the project's access tokens and
requests deletion of the exact verified project, group and users. Failure on an external server retains provider resources
for inspection. A managed service drops its container and provider resources when
the suite exits. Local manifests, credentials, child output and REST observations
remain available in both cases. The fixture prints its manifest path.
Credential and evidence files have mode `0600` beneath a directory with mode `0700`.
Repeated fixture commands append process evidence without overwriting earlier captures.
Git commands use a process group and terminate its descendants on timeout.

These scripted judgments verify the executable workflow. Fresh agent reviews
provide separate evidence that reviewers can assess source and author valid inputs.

## Agent review lifecycle

The Rust `cli_fixture` example uses the same fixture owner as the suite.
Build `rkc`, then create a fixture on the existing local service:

```sh
cargo build -p review-kit-cli --bin rkc
cargo run -p review-kit-cli --example cli_fixture -- setup
```

Setup prints the private manifest path and two merge request URLs.
The manifest contains identity and credential paths, revisions, trusted
configuration and ordinary Git source checkouts. Git authentication uses an
askpass helper without putting tokens in URLs or command arguments.
Set `RKC_E2E_BINARY` to select an installed executable.

The first merge request adds JSON input and optional audit output to a small
Rust library. Reviewers use its README contract, committed source and ordinary
offline tests. The second adds valid checked arithmetic for a clean-review control.
Give agents the merge request URL, reviewer credential/configuration paths,
source checkout and CLI instructions. Keep fixture internals outside the review brief.

To exercise policy selection, supply a trusted ordinary skill containing policy
URLs without companion manifests or a preselected assessment. Start reviewers
without fixture answers or earlier review results. Retain which definitions they
read, their inclusion and exclusion reasons, and current proof. Include a custom
policy outside the installed catalog and an unrelated policy. Separately exercise
an unreadable definition and missing required evidence, then restore each input
to check that valid work can pass.

Inspect GitLab through independent REST reads after publication. Check each
finding's primary policy name, every selected policy's summary coverage, and
the owned thread and summary identities through unchanged, fixed and recurrent
reviews. Policy names are references; the default renderer does not link skill
URLs. A successful CLI check proves assessment consistency, not complete or
correct agent-selected coverage.

Evolve or inspect the review fixture:

```sh
cargo run -p review-kit-cli --example cli_fixture -- fix --manifest MANIFEST
cargo run -p review-kit-cli --example cli_fixture -- inspect --manifest MANIFEST
cargo run -p review-kit-cli --example cli_fixture -- recur --manifest MANIFEST
cargo run -p review-kit-cli --example cli_fixture -- cli --manifest MANIFEST [--mr review|clean] -- run start --mr URL
```

`fix` commits corrections and regression tests, then replies as the author
to the reviewer's inline threads. `recur` commits the quantity defect again.
`fix` accepts the review, fixed and recurrence phases; `recur` requires the fixed phase.
Both commands wait for GitLab to collect the expected diff and clone its source
into a checkout identified by phase and commit. Earlier checkouts remain available.
They verify that the retained project belongs to the fixture's namespace.

`inspect` saves independent notes and discussions privately, then prints counts
and current heads. `cli` supplies the trusted configuration and reviewer token
to `rkc` and runs it in the current source checkout of the `--mr` merge request,
`review` by default. It forwards arguments after `--` and preserves the command's
exit code. `--rkc PATH` selects an executable for this command.

Use the fixture's `tmp` directory as `TMPDIR`. Keep each source checkout's Cargo
output in its own `target` directory. The synthetic library declares its own
Cargo workspace. Sharing a target between checkouts with the same package name
and version can reuse the wrong checkout's test binary. Verify the source revision
and expected test list before accepting proof. Setup runs no source build or review.

## Authority and abandonment controls

After a fresh agent completes the clean fixture review, run:

```sh
cargo run -p review-kit-cli --example cli_fixture -- controls --manifest MANIFEST --run RUN_DIRECTORY
```

`RUN_DIRECTORY` is the completed run's `<worktree>/.review-kit/review-runs/<id>`.
The controls reuse its accepted policy declarations and outcomes in new runs
on a separate merge request at the same committed head. They execute the
production CLI and observe GitLab directly. The accepted assessment must belong
to the clean fixture merge request and exact head. The unknown-write control
injects an unresolved write into the publication journal; controlled HTTP tests
and the campaign's response-withholding proxy cover transport loss. It restores
only its own synthetic intent after checking preservation.

Delete the fixture's verified provider resources when inspection is complete:

```sh
cargo run -p review-kit-cli --example cli_fixture -- cleanup --manifest MANIFEST
```

Cleanup saves a deletion ledger and follows GitLab's deletion policy.
It retains local evidence and refuses mismatched project, group or user identities.

## Campaign fixtures

A campaign is one private namespace with a project for each fixture family.
It owns a Maintainer author, a Developer reviewer with separate `api` and
`read_api` tokens, and a Developer commenter. Family content lives as data under
[`families/`](../../crates/bin/cli/tests/e2e/families/). Each family has a
`family.json` descriptor, default-branch files in `base/`, and named overlays in
`overlays/`. A merge request starts from its overlay; later overlays apply as new
commits. Each project carries its own contract documents and offline tests.
Every base also commits `.skillcatalog/skillcatalog.yml`, the SkillCatalog
manifest that a review host installs in the checkout before the reviewer starts;
the loader rejects an overlay that writes or deletes anything below
`.skillcatalog/`. Setup refuses content whose wording discloses an expected
review outcome.

| Family | Project | Change |
| --- | --- | --- |
| `quantity-invariant` | `quantity-ledger` | Rust parsing, arithmetic and ledger moves with five follow-up phases; three further merge requests add a quantity limit in two implementations and quantity totals |
| `clean-control` | `unit-counter` | Rust checked arithmetic with boundary tests; four further merge requests add the same multiplication with different tests |
| `authorization-boundary` | `document-service` | Python archiving and paged listings under access rules |
| `compatibility-migration` | `profile-store` | Two merge requests that change a documented stored format |
| `documentation-workflow` | `release-handbook` | An agent skill rewrite checked against a script and a procedure |
| `custom-url-criteria` | `payout-ledger` | Rust fees and refunds whose team rules come only from skill URLs |
| `readme-contract` | `retry-schedule` | Two README-only merge requests on a Rust crate: a spelling correction and a documented constructor contract |

List families and overlays, or write a phase's files for offline inspection:

```sh
cargo run -p review-kit-cli --example cli_fixture -- families
cargo run -p review-kit-cli --example cli_fixture -- materialize \
  --family quantity-invariant --mr review --overlay parse-through-constructor --dir DIRECTORY
```

Build `rkc` and the example, then create a campaign on the existing service with
the same `REVIEW_KIT_GITLAB_URL` and `REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE` variables
as the suite. `--families` selects a comma-separated subset; all families are the default.

```sh
cargo build -p review-kit-cli --bin rkc --example cli_fixture
target/debug/examples/cli_fixture setup-families
```

Setup prints the manifest path and every merge request URL. The campaign directory
is `tests/cli/.local/rk-camp-*`, or a directory below `RKC_FIXTURE_HOME` when set.
It holds the manifest, credential files with mode `0600`, the trusted `rkc`
configuration and one Git checkout per merge request head. The configuration's
`publication_state` is the campaign's `publication-state` directory, which host
and contained runs share. GitLab applies push effects after the API call returns, so
setup and history changes wait until the merge request's diff versions and notes
stay unchanged for three seconds.

### Provider operations

Every operation names the campaign manifest, a family and optionally a merge
request label (`--mr`, default: the family's first merge request). Before writing,
it verifies that the project belongs to the campaign namespace and that the
campaign author opened the merge request from the recorded branch. It saves a
private record under `evidence/mutations/`.

| Operation | Effect |
| --- | --- |
| `apply --overlay NAME [--to-target]` | Commits an overlay to the merge request branch, or to its target branch |
| `force-push [--onto REF] [--squash] [--message TEXT]` | Replays the merge request commits onto the target head or `REF` with new commit IDs, optionally as one commit, and force-pushes |
| `retarget --branch NAME --from REF [--overlay NAME]` | Creates the branch from `REF` (`initial` names the project's first commit) and points the merge request at it; the source head stays unchanged |
| `edit-description (--text TEXT \| --file PATH)` | Replaces the description as the author |
| `note --as ROLE --body TEXT` | Adds a general note as `author` or `commenter` |
| `inline-note --as ROLE --path PATH --line N [--old-line N] --body TEXT` | Starts an unmanaged diff thread at a real position |
| `reply --as ROLE (--note ID \| --discussion ID) --body TEXT` | Replies in a discussion |
| `edit-note --as ROLE --note ID --body TEXT` | Edits a note as its author, including the reviewer |
| `delete-note --as ROLE --note ID` | Deletes a note |
| `resolve --as ROLE --discussion ID [--unresolve]` | Resolves or reopens a discussion as the author or commenter |
| `bulk-notes --as ROLE [--count N]` | Adds general notes, 105 by default, so discussions span two pages |
| `delete-root --finding N [--as ROLE]` | Deletes the reviewer's managed root note for finding `N` and keeps its replies |

The manifest's `source` checkout changes after `apply` and `force-push`;
earlier checkouts remain.

### Independent observations

`observe --name NAME` reads the merge request, every note and discussion page and
the diff versions with the fixture's own REST client. It saves the raw responses
and a digest to `evidence/oracle/NAME.json` and prints the digest. The digest
contains diff references, versions, note counts by author, discussions with
positions and resolution, managed finding roots and managed summary bodies.
Names are unique per campaign. `compare --before NAME --after NAME` lists added,
removed and edited notes, moved positions, resolution changes and managed root
and summary changes. `inspect` observes every campaign merge request and prints counts.

### Running rkc

`rkc --manifest MANIFEST --family NAME [--token ROLE] [--rkc PATH] -- ARGUMENTS`
runs `rkc` from the merge request's current checkout with the campaign
configuration. The selected token (`reviewer`, `read-only`, `author` or
`commenter`) reaches `rkc` only through the configured environment variable.

### Recording proxy and response loss

`proxy --listen ADDRESS --upstream HOST:PORT --log FILE` forwards every request
unchanged apart from hop-by-hop headers and appends one JSON line per exchange:
method, path without query values, query parameter names, status and sizes. It
never records headers or bodies. `proxy-summary --log FILE` counts exchanges and
mutation requests, the independent zero-write check.

The withholding options select one mutation: `--withhold-method` (or `*`),
`--withhold-path` (`*` matches one path segment, `**` several), and
`--withhold-occurrence N` among matching mutations. `--withhold-response close`
forwards that request, reads GitLab's complete response, records its status and
closes the client connection. `hold:SECONDS` does the same but keeps the client
connection open past the client timeout first. `delay:SECONDS` waits before it
forwards the request, so a client that gives up and resends meets the original
request arriving later; the client connection closes without a response.

`contained-run` runs `rkc`, or with `--probe-note TEXT` one fixture-client note
write, in a disposable container attached to the GitLab Compose network:

```sh
target/debug/examples/cli_fixture contained-run --manifest MANIFEST --family clean-control \
  --network GITLAB_NETWORK --container-prefix PREFIX \
  --withhold-method POST --withhold-path '/api/v4/projects/*/merge_requests/*/notes' \
  --rkc target/debug/rkc -- ARGUMENTS
```

The proxy listens on the container's own `127.0.0.1` at the campaign server's port
and forwards to `gitlab:PORT` on that network (`--upstream` overrides it), so the
canonical server URL stays unchanged. The container image defaults to
`ubuntu:26.04`. It receives read-only mounts of the fixture executable, `rkc`, the
host `git` executable, the host CA bundle and a generated configuration, plus
writable mounts of the checkout, the campaign `publication-state` directory and a
log directory under `evidence/contained/`. The reviewer token travels in a mode
`0600` environment file that is deleted once the container exists. The container
runs as the campaign directory's owner without capabilities and is removed after
it exits or times out. The command prints the proxy summary and, for a probe,
the stored note IDs from an independent REST read, then exits with the container's status.

`--checkout DIR` mounts another checkout and `--state DIR` another
publication-state directory; both must lie below the campaign directory.
`--fault POINT` passes `RKC_FAULT=POINT` to a `fault-injection` build of `rkc`.
A program ended by a signal exits with 128 plus the signal number, so an
injected abort exits with 134.

### Trusted skill URLs

`serve-files --root DIRECTORY --listen 127.0.0.1:PORT --log FILE` serves regular
files below the root on loopback and records each request. Removing a file makes
its URL unreadable until it returns. Serve custom policy lists and skills this way
to retain which sources a reviewer read.

### Cleanup

`cleanup --manifest MANIFEST` verifies every recorded identity, revokes the
project's active access tokens, requests deletion of the project and group and
deletes the users. GitLab keeps a project bot user and its token working until it
removes the project, and the project and its top-level group stay scheduled for
deletion until the instance's retention period ends. Cleanup accepts the paths GitLab assigns to
scheduled deletions, saves `evidence/cleanup.json`, and can run again to finish an
interrupted cleanup.

[Offline harness tests](../../crates/bin/cli/tests/fixture_harness.rs) cover the
proxy against a scripted upstream, the file server, the family data and the
observation digest. They need no Docker or GitLab.
