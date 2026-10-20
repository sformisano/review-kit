# rkc

`review-kit-cli` installs `rkc`. It captures a GitLab merge request into a review
run in the current Git worktree, accepts one complete policy assessment, and plans, publishes or
recovers the review. The application owns use cases; this crate owns arguments,
configuration, adapter wiring and process output. This guide is the command contract.

## Installation

For a prebuilt Linux binary, use the [installation guide](../../../docs/distribution.md).
It includes checksum checks, installation, upgrade and rollback instructions.

Follow the [source installation instructions](../../../README.md#install-and-run)
for prerequisites and commands. Make Cargo's configured binary directory
available on your `PATH` before running `rkc`.

Cargo installs `rkc` in its configured binary directory. Rendering assets are
embedded in the [json2md adapter](../../lib/infrastructure/json2md-adapter/README.md).
The separate json2md executable is unnecessary.

## Trusted configuration

Review Kit uses `$HOME/.review-kit/` as its application home. Save trusted review
settings in `~/.review-kit/config.toml`:

```toml
version = 1
publication_state = "publication-state"
review_workflow = "team-review"
render_profile = "review-style/profile.json"

[gitlab]
server = "https://gitlab.example.com"
token_env = "GITLAB_TOKEN"
authority = ["create_summary", "update_summary", "finding", "reply", "resolve", "reopen"]

[logging]
destination = "file"
directory = "logs"
```

`version = 1` is required for review commands.
The other settings in this example are optional.
`--help`, `--version` and `render-profile export` require no review settings.
`run start`, `publish`, `check` and `accept` require them.
Other run commands use the configuration, when it loads, only to read the
[publication journal](#publication-and-recovery).

`--config PATH` overrides `RKC_CONFIG`. Both select review settings only;
[logging preferences](#logging-preferences) always come from the application home.
Paths ending in `.toml` use TOML. Other paths use JSON, including extensionless
files. Explicit JSON configuration remains supported:

```json
{
  "version": 1,
  "publication_state": "publication-state",
  "review_workflow": "team-review",
  "render_profile": "review-style/profile.json",
  "gitlab": {
    "server": "https://gitlab.example.com",
    "token_env": "GITLAB_TOKEN",
    "authority": ["create_summary", "update_summary", "finding", "reply", "resolve", "reopen"]
  }
}
```

Without an explicit path, the CLI checks the home TOML and the existing JSON default.
The JSON default is `$XDG_CONFIG_HOME/review-kit/config.json`, or
`$HOME/.config/review-kit/config.json` when `XDG_CONFIG_HOME` is unset.

| Available configurations | Selection |
| --- | --- |
| Home TOML only | Home TOML |
| JSON default only | JSON default |
| Both | Error; select the intended file with `--config` or `RKC_CONFIG` |
| Neither | Error; create configuration or select an existing file explicitly |

Unreadable or invalid selected configuration fails the command. The CLI never
moves, merges or overwrites configuration automatically.
Configuration must be a regular UTF-8 file no larger than 1 MiB.

`render_profile` is optional and resolves relative to that file.
The global `--render-profile PATH` overrides it and resolves relative to the
invocation directory. Without either, commands use embedded defaults.
Asset paths resolve relative to the selected manifest.
An invalid explicit profile fails; it never selects embedded defaults instead.

`review_workflow` is an optional public name shown in review metadata.
It accepts up to 160 bytes of letters, digits, spaces, dots, underscores and hyphens.
Omitting it displays unavailable workflow identity.
The policy-set name comes from the agent's assessment.
Neither identity is inferred from templates or GitLab text.

`publication_state` also resolves relative to that file and defaults to
`publication-state`. The home TOML therefore uses `~/.review-kit/publication-state`.
Explicit configuration retains its own relative paths.
It stores one publication journal per subject and reviewer, shared by every
review run and worktree. Configurations used for the same review
must select the same state directory. Protect and retain it alongside the
review runs; changing it cannot settle an unknown write.
When changing configuration locations, select the existing state directory
explicitly before resuming publication. The CLI cannot find journals in arbitrary directories.
`gitlab` is optional for offline commands. `token_env` defaults to `GITLAB_TOKEN`.
The token comes from that environment variable; configuration contains no token.
The configured server and credentials bind provider access independently of the
assessment. See the [GitLab contract](../../lib/infrastructure/gitlab-adapter/README.md)
for supported URLs and token capabilities.

`authority` defaults to an empty list. Allowed values are `create_summary`,
`update_summary`, `finding`, `reply`, `resolve` and `reopen`.
Command effect selection cannot expand this authority or provider permissions.
Unknown configuration fields and unsupported versions fail validation.
Keep configuration under host control; provider content cannot select it.

## Logging preferences

The home `config.toml` controls process logging before argument parsing.
No logging flags or shell redirections are required.
The optional `[logging]` table accepts these settings:

| Setting | Values and default |
| --- | --- |
| `destination` | `"file"` (default) or `"stderr"` |
| `directory` | Nonempty directory path; defaults to `"logs"` |

File delivery appends newline-delimited JSON to `review-kit.jsonl` in that directory.
Relative directories resolve under the application home. Absolute paths remain
absolute; `~/` resolves under `$HOME`. The default file is
`~/.review-kit/logs/review-kit.jsonl`.

For another directory:

```toml
[logging]
destination = "file"
directory = "~/review-logs"
```

For stderr:

```toml
[logging]
destination = "stderr"
```

Missing home configuration uses default logging settings. Unreadable or malformed
home configuration also uses those settings. Invalid logging fields use defaults
without rejecting otherwise valid review settings.
A malformed or unreadable document still fails a review command that selects it.
Help, version and profile export remain independent of review configuration.

An unset, empty or relative `$HOME` provides no application home; logging falls
back to stderr. Failure to open the log file also falls back to stderr.
These fallbacks preserve command output and exit codes.
The `cli.run` start record reports only fixed configuration and destination
categories and a fallback flag under `execution.input.logging`.
It includes no paths, configuration contents or parser errors.

On Unix, newly created log directories use mode `0700`; new log files use `0600`.
Existing permissions remain unchanged. Each record is buffered and appended under
a cross-process file lock, then synchronously flushed. Logging does not call
`fsync` or guarantee durable delivery.
This lock covers log delivery. Publication uses a separate [review lock](#publication-and-recovery).
External agents own remote delivery, rotation and retention. A running process
keeps its open file after rotation renames it.
See [log collection](../../lib/observation/README.md#collect-cli-logs) for agent setup
and the [delivery limits](../../lib/observation/README.md#delivery-and-limits) for sink failures.

## Progress output

The global `--progress auto|plain|none` option controls status messages on stderr.
It is independent of operation logging and command-result stdout.

| Mode | Behavior |
| --- | --- |
| `auto` | Compact status in a terminal; complete lines when redirected |
| `plain` (default) | Complete, flushed lines, including in a terminal |
| `none` | Suppress progress; preserve diagnostics, logs and results |

Long operations report their current stage. Plain output emits a waiting line
every ten seconds without a stage change. Interactive auto output animates the
current stage. Messages contain fixed stage names without review
bodies, credentials, paths or provider identifiers.

Agents and CI collectors need no progress option: the `plain` default writes
complete lines even under a pseudo-terminal.

Drain stderr while the process runs. Progress helps hosts detect a running
command; it does not override a host's fixed execution timeout.
Use `--progress none` when a collector requires only configured JSON logs on stderr.
See [observer delivery limits](../../lib/observation/README.md#delivery-and-limits)
for blocked output sinks.

## Review runs

A review run is one capture of a merge request and its complete assessment.
Every run command works on the Git worktree that contains the invocation
directory, from any subdirectory. Commands take no directory or file arguments
for runs. Outside a Git worktree they fail with `not_in_git_worktree`.
Linked worktrees of one repository keep separate runs.

| Command | Behavior |
| --- | --- |
| `rkc run start --mr URL [--new]` | Capture the merge request, create a run, select it and write `review-assessment.draft.json` |
| `rkc run start [--new]` | The same for the selected run's merge request and the currently authenticated reviewer |
| `rkc run list` | List stored runs: identity, head, selection, lifecycle, `publication_attempted` and assessment state |
| `rkc run select ID` | Validate a stored run and select it, including a stale or terminal run; never recaptures or reopens it |
| `rkc status` | Report the selected run, its assessment, retained files, publication journal state and next action |
| `rkc check` | Diagnose the complete assessment draft with the acceptance validations |
| `rkc accept` | Validate the complete assessment and freeze its exact bytes |
| `rkc revise --reason TEXT [--regenerate]` | Archive the draft and prepare a successor; see [checkpoint revision](#checkpoint-revision) |
| `rkc publish --effects SET [--plan]` | Plan, publish or recover the selected run; see [publication](#publication-and-recovery) |
| `rkc publish --abandon --reason TEXT [--effects update_summary]` | Close the round of the selected run's unfinished publication |
| `rkc run finish` | Complete a clean accepted run locally without provider calls |
| `rkc run stop --reason TEXT` | Stop unpublished work; needs no accepted assessment and no provider connection |

A normal review runs:

```sh
rkc run start --mr https://gitlab.example.com/team/project/-/merge_requests/7
rkc accept                 # after completing review-assessment.draft.json
rkc publish --effects review --plan
rkc publish --effects review
```

`run start` captures the full current comparison, committed source, verified
managed review history and supported conversation. A capture failure leaves no
selectable run and the previous selection unchanged. Contradictory managed
history fails with [`managed_history_conflict`](#output-and-exit-status).

When GitLab moves a finding onto changed anchor or suggestion text, capture
succeeds and lists its ID in `outdated_positions`. The generated draft's
`local_context` flags these findings for reassessment, listing up to 50 IDs.
The output includes the complete list. Captured findings keep their retained
versions and creation locations; the conversation keeps GitLab's observed positions.
This classification does not mark a finding fixed or confirm its current validity.
The [adapter contract](../../lib/infrastructure/gitlab-adapter/README.md#source-and-suggestion-support)
defines position validation and the identity conflicts that still fail capture.

Reassess each affected finding against the captured source.
For a changed location, give the new `path:line` in the transition reply and identify the old suggestion as outdated.
Replies, resolutions and summary updates use the existing thread.
No position-dependent write uses an outdated position.

When an open run already exists for the same merge request and reviewer, `run start`
fails with `open_run_exists` and lists its run IDs; continue that run or pass
`--new` to start another one. Runs of several merge requests of one repository
share the worktree; select one with `run select`. The worktree is bound to the
repository of its first run; a merge request of another repository fails with
`repository_mismatch`. `run start` checks the binding under the workspace lock
and never rewrites it. Without `review-workspace.json`, such as after a crash
before the first run was selected, the binding comes from the stored runs'
manifests, and manifests of different repositories fail with
`repository_mismatch`.

### Runtime files

Runtime files live in `<worktree root>/.review-kit/`. rkc writes `.gitignore`
there with `*`, so Git ignores them. Before any runtime file is read, rkc
rejects a `.review-kit` path or managed component that is a symbolic link
(`runtime_redirected`), any Git-tracked path below it (`runtime_tracked`), and
unrecognized entries in a directory that has no `review-workspace.json`
(`runtime_unrecognized`). A run file that is a symbolic link, such as
`review-publication-events.json`, fails with `runtime_redirected` when a command
uses it; nothing is read or written through the link.

| Path | Owner | Purpose |
| --- | --- | --- |
| `review-workspace.json` | rkc | Repository binding and selected run |
| `review-workspace.lock` | rkc | Operating-system lock for local mutations; the file stays |
| `staging/<run-id>/` | rkc | Incomplete capture; never selectable; kept for diagnosis |
| `review-runs/<run-id>/review-run.json` | rkc | Manifest: identity, retained-file hashes, accepted references, lifecycle, `publication_attempted` |
| `review-runs/<run-id>/review-context.json` | rkc capture | Comparison, verified managed history and observed permissions |
| `review-runs/<run-id>/reviewed-source.json` | rkc capture | Provider diff data and committed file contents |
| `review-runs/<run-id>/merge-request.diff` | rkc capture | Readable unified diff |
| `review-runs/<run-id>/review-conversation.json` | rkc capture | Title, description and supported discussions, as review context; see [captured conversation](#captured-conversation) |
| `review-runs/<run-id>/review-assessment.draft.json` | rkc, then the agent | [Assessment](#assessment-editing) draft |
| `review-runs/<run-id>/accepted/<hex>/review-assessment.json` | rkc | Frozen accepted bytes |
| `review-runs/<run-id>/draft-history/<hex>/<draft file>` | rkc | Exact draft bytes archived by `revise` |
| `review-runs/<run-id>/review-check-result.json` | rkc | Latest check or acceptance result; not an authority |
| `review-runs/<run-id>/review-render-profile.json` | rkc | Profile identity of the latest successful assessment acceptance or plan |
| `review-runs/<run-id>/rendered/review-summary.md`, `rendered/terminal-review-summary.md` | rkc | Previews of the latest valid assessment check or acceptance |
| `review-runs/<run-id>/review-publication-events.json` | rkc | Execution evidence, exact bodies and observed results |
| `review-runs/<run-id>/review-publication-result.json` | rkc | Latest plan, publication or abandonment report |

`<hex>` is the lowercase SHA-256 of the exact bytes; references are `sha256:<hex>`.
Every retained reference resolves against the run directory, never against `accepted/<hex>/`.
The agent edits only `review-assessment.draft.json`. rkc refuses retained files whose bytes differ from
the manifest's hashes (`hash_mismatch`), even at the same revision, and files
that describe another subject, reviewer or revision (`identity_mismatch`).
`status` reports changed files that still decode for the run as
`retained_files.state: "mismatch"`. It reports files that are missing, do not
decode or describe another run as `"damaged"`. Hashes detect changed bytes;
they do not authenticate a process that can rewrite the manifest and every file.

Each file is written to a temporary file, synchronized, renamed and its directory
synchronized. A new run is written completely under `staging/` and renamed into
`review-runs/` before `review-workspace.json` selects it. Every local mutation
holds the workspace lock; a competing command fails with `workspace_busy`.
`status` and `run list` take no lock. A command reads the
selected run once and works on that run until it ends.

### Captured conversation

`review-conversation.json` holds the merge-request conversation as `run start`
captured it. It is review context for the agent: it grants no finding identity,
reply authority or effect authority. rkc never rewrites it. rkc writes version 2
and also reads version 1, which has no `author_username` or `author_bot` field.
The [state compatibility contract](../../../docs/render-profiles.md#retained-state-compatibility)
lists which executables write and read each version.

| Field | Meaning |
| --- | --- |
| `version` | `2`, or `1` in a file that rkc 0.1.0 wrote |
| `kind` | `"review_conversation"` |
| `subject` | Provider, host, repository and merge-request number |
| `observed_at` | Capture time in UTC, for example `2026-10-20T08:15:00Z` |
| `title`, `description` | Merge-request text at capture; a `null` description from GitLab is `""` |
| `discussions[].id` | GitLab discussion ID |
| `discussions[].resolvable` | `true` when any note of the discussion is resolvable |
| `discussions[].resolved` | `true` when every resolvable note is resolved; `null` when the discussion is not resolvable |
| `discussions[].notes[].id` | GitLab note ID |
| `discussions[].notes[].author` | The author's numeric GitLab user ID, as text. This value identifies the account |
| `discussions[].notes[].author_username` | The author's GitLab username at capture time, or `null` when GitLab sent none or a blank one. A username can change; `author` does not |
| `discussions[].notes[].author_bot` | `true` when GitLab marks the account as a bot, `false` when GitLab reports a non-bot account, `null` when unknown |
| `discussions[].notes[].body` | The note's Markdown as GitLab stores it |
| `discussions[].notes[].position` | GitLab's diff position object, or `null` for a note without a diff anchor |

GitLab marks project and group access token bots, service accounts and internal
users such as GitLab Duo (`GitLabDuo`) as bots. rkc asks GitLab for the bot
status only during `run start`: once per distinct note author, for at most 50
authors. Planning and publication ask nothing. `author_bot` is `null` when the
lookup failed or was not made, for example after a rate limit or for an author
beyond the first 50. The
[GitLab adapter](../../lib/infrastructure/gitlab-adapter/README.md#note-author-accounts)
lists which failures also stop the lookups for later authors. A version 1 file
has no `author_bot` field, which also means unknown. Treat `null` or a missing
field as unknown: it shows neither a bot nor a person. `false` reports GitLab's
account type only; it does not prove that a person wrote the note.

Account details never change publication rules. Inside a
[write target](#publication-and-recovery), a bot's note blocks like anyone
else's. The freshness preflight compares authors by `author` only, so a renamed
account, a missing username or a changed bot status never blocks publication.
System notes are not captured. The
[JSON adapter](../../lib/infrastructure/json-adapter/README.md#supported-conversation)
owns the schema and validation.

An intact conversation with a version above the one this rkc reads fails with
`newer_run_document` (exit `2`). Intact means that its bytes match the
manifest's hash. The diagnostic adds `file`, `version` and `supported`, and
`status` shows `retained_files.state: "newer"`. Continue such a run with the
rkc that wrote it, or a later one.

rkc 0.1.0 reads version 1 only. On a run that a newer rkc started, rkc 0.1.0
`run list` and `run stop` still work, and `status` shows
`retained_files.state: "damaged"`. Every other rkc 0.1.0 command that opens the
run fails with exit `2`: `run select`, `check`, `accept`, `revise`, `run finish`
and `publish` in every mode. The code is `invalid_run_document` when the
captured conversation has a note, and `unsupported_version` when it has none. When such a run owns
unfinished publication and one of its writes may have reached GitLab, rkc 0.1.0
therefore cannot recover it. Planning and publication of every other run of the
same merge request and reviewer then fail with `publication_pending_owner`
until the newer rkc finishes it. When none of the owner's writes could have
been sent, another run's publication releases the owner's binding as usual.
[Rollback](../../../docs/distribution.md#upgrade-and-rollback) lists what to
finish first.

A note by a project access token bot, with illustrative values:

```json
{
  "id": "1201",
  "author": "345",
  "author_username": "project_42_bot_0123456789abcdef0123456789abcdef",
  "author_bot": true,
  "body": "Automated review: no blocking issues detected.",
  "position": null
}
```

### Assessment acceptance

`accept` validates the complete draft, installs its exact bytes under `accepted/<hex>/`, then advances the manifest's single assessment reference.
The snapshot is durable before the manifest names it.
A crash leaves either the previous reference or the new reference; an unreferenced snapshot grants no authority.
An exact retry reports `existing`. Acceptance never overwrites the draft.
`check` runs the same validations without changing the accepted reference.
On a terminal run, checking reports diagnostics without writing previews or the check-result file.

Local checks, acceptance and revision use the immutable retained capture without refreshing GitLab.
Online planning and publication apply their distinct [freshness boundaries](#publication-and-recovery).
Acceptance proves declared consistency, including run, revision, successor, policy and finding rules.
It cannot establish complete policy discovery, correct relevance decisions or true evidence.
A policy omitted from the entire declaration cannot be detected by the tool.

`status`, `run list` and `run select` report one assessment state:
`missing`, `draft`, `accepted`, `dirty`, `missing_snapshot` or `damaged_snapshot`.
`dirty` means the draft is absent or differs from its accepted snapshot.
Missing or damaged accepted bytes require restoring that exact snapshot.
Selection never recaptures or reopens a run.

### Lifecycle

A run is `open`, `completed` or `stopped`. Completion is `published`,
`no_changes`, `local_only` or `abandoned`. Terminal runs stay inspectable and
selectable; `accept`, `revise`, `run finish`, `run stop` and new publication
reject them with `run_terminal`. A fresh review is a new run.

`run finish` requires an accepted assessment, intact retained files and a draft equal to its snapshot
(`dirty_draft` otherwise). `run stop --reason` records a nonblank reason. It
reads only the manifest and the publication journal, so it works when retained
files are missing or damaged; `status` then reports `retained_files.state` as
`damaged` with the affected `files`.
Before `accept`, `revise`, `run finish` and `run stop`, rkc reads the
publication journal (see [local state and the journal](#local-run-state-and-the-journal)).

## Assessment editing

`run start` writes one version 4 assessment with the captured run ID, revision and carried finding IDs.
Fill this single document with the complete review.
Subject, reviewer and immutable prior findings come from `review-context.json`; they are not duplicated in authored input.
The [assessment schema](../../lib/infrastructure/json-adapter/schemas/review-assessment-v4.schema.json) owns its exact shape.

| Field | Meaning |
| --- | --- |
| `version`, `kind`, `run_id`, `revision`, `supersedes` | Generated binding; preserve it |
| `reason` | Initial assessment or successor explanation |
| `policy_set` | Nonblank public name of the selected policy set |
| `policies` | One ordered combined row per selected policy |
| `findings` | One `{id,reusable_confirmation,reassessment}` entry per captured finding; no copied `previous`. rkc generates `reusable_confirmation`; preserve it |
| `new_findings` | New findings under selected policies |
| `decision` | String `ready`, `changes_required` or `blocked`; `null` only in an unfinished draft |
| `depth` | Nonblank free-text string describing scrutiny and its basis; `null` only in an unfinished draft |
| `scope` | Nonblank free-text string describing reviewed coverage; `null` only in an unfinished draft |
| `limitation` | Nonblank string when supplied; required for `blocked`, otherwise `null` when absent |
| `answers`, `transitions` | Needed authored communication |
| `confirmations` | Independent verdicts on findings; see [finding confirmations](#finding-confirmations) |
| `local_context` | Private file pointers and decisive facts, at most 2000 characters; excluded from public content |

Each policy row records `policy_id`, public `name`, `required`, public `reason` and `outcome` once.
A selected policy whose definition cannot be read has an `unresolved` outcome.
An unreadable selected list leaves unknown coverage; the agent must not claim a complete declaration.
The tool never reads policy prose or chooses policies.

| Outcome | Meaning |
| --- | --- |
| `{"kind":"excluded"}` | Policy does not apply; no judgment is created |
| `{"kind":"unresolved","limitation":"..."}` | Applicability is unknown; the evaluation remains blocked |
| `{"kind":"satisfied"}` | Applicable and satisfied; no separate prose justification is required |
| `{"kind":"violated","evidence":"..."}` | Applicable with established violation evidence |
| `{"kind":"blocked","limitation":"..."}` | Applicable, but evaluation is unavailable |
| `null` | Unfinished draft only; acceptance rejects it |

A required policy cannot be excluded.
Every active carried finding's policy must remain selected.
Excluding that policy cannot erase an active finding; the assessment must close it with an explanation.
The tool derives public applicability and policy judgments from each outcome.
Every policy's name, derived applicability and reason remains public, including excluded and unresolved rows.
Finding evidence, violation evidence and blocked limitations also enter public retained state even when templates omit them.
Keep private paths, prompts and proof details in `local_context` or retained host evidence.
Retain shared evidence once by readable reference, without copying definitions or producing another complete report.

Each finding uses this shape:

```json
{
  "id": 1,
  "policy_id": "rust-error-contracts",
  "title": "Retain the request after a failed send",
  "body": "The failure branch drops the request before another attempt can run.",
  "severity": "p2",
  "disposition": "blocking",
  "status": "open",
  "evidence": "The error branch returns without retaining or returning the request.",
  "validated": false,
  "location": {"path": "src/worker.rs", "line": 42},
  "suggestion": null
}
```

Finding IDs are positive integers; `run start` reports the next available ID as
`next_finding_id`.
Severity is `p1` through `p5`; disposition is `blocking` or `advisory`. Status is
`open`, `fixed`, `withdrawn` or `uncertain`. An open or uncertain blocking
finding is validated only by a [confirmation](#finding-confirmations); write
`false` and rkc sets it. Other findings keep the agent's `validated` record,
which does not prove an independent review happened. The
[domain](../../lib/domain/README.md#findings-and-assessments) and
[application](../../lib/application/README.md#assessment-boundary) define finding,
suggestion, reply and transition requirements.

Decide every carried finding in its `reassessment`:

| Value | Meaning |
| --- | --- |
| `"unchanged"` | The captured version stands exactly; rkc uses it as the current version. A suggestion keeps its captured revision, so an open finding with a suggestion needs a finding object after the head changes |
| A finding object | The complete current version with its state and evidence |
| `null` | Undecided; checking reports it |

The immutable previous record comes from the capture. An `"unchanged"` open or
uncertain blocking finding restarts unvalidated; only a `checker` or applicable `retained`
confirmation validates it. A status change needs a transition:

```json
{"finding": 1, "from": "open", "to": "fixed", "body": "The failure path retains the request; the retry regression passes."}
```

So does a same-state reclassification: a version that keeps its status but changes its policy,
severity, disposition, validation, location or suggestion, such as a finding whose line moved.
Its `from` and `to` are equal; the default profile's reply opens with
[`Updated.`](../../lib/infrastructure/json2md-adapter/README.md#default-presentation-and-safety).
A carried finding left unvalidated differs from its posted version in validation alone, so it needs such a
transition and a `blocked` decision; a blocked review withholds that reply.
Checking rejects a same-state transition when the final finding equals its posted version.

An answer names the finding and the exact foreign reply note ID from the
retained context:

```json
{"finding": 1, "in_reply_to": "123", "body": "The updated failure path now retains the request for another attempt."}
```

A suggestion copies the assessment revision; its original lines must match
retained committed source exactly. Its `location` equals the finding's `location`
and lies within the replaced lines, from `start_line` through the last `original` line:

```json
{
  "location": {"path": "src/worker.rs", "line": 42},
  "revision": {"head": "head-sha", "comparison": ["base-sha", "start-sha"]},
  "start_line": 42,
  "original": ["    Err(_) => queue.ack(job.id),"],
  "replacement": ["    Err(_) => queue.retry(job.id),"],
  "evidence": "The failure regression confirms the request remains available for retry."
}
```

`check` and `accept` complete the assessment against the
run's `review-context.json` and `reviewed-source.json`, render it with the
selected profile and validate GitLab bodies. They build the publication request
as planning and publication do, with the trusted configuration's
`review_workflow` and every review effect, so previews, acceptance and
publication carry the same public review identity. A valid result writes the
previews under `rendered/`; an invalid one removes them, so earlier previews
never stand for a current result. `accept` also records the profile
identity in `review-render-profile.json`. Review runs require distinct locations for new findings.

Preview counts and finding states reflect retained observations. Publication
recomputes the summary after verified effects; its final snapshot shows what was
communicated.

### Public summary text

The default summary shows the decision, the overall `limitation` and the findings first.
Its last collapsed section, `Review details`, shows each round's `scope` and `depth`, and each policy's `reason` and any blocked or unresolved `limitation`,
grouped by applicability with each result. The
[renderer contract](../../lib/infrastructure/json2md-adapter/README.md#default-presentation-and-safety) defines the layout,
including each earlier round's collapsed findings section and the order of rounds in `Review details`.
These fields are plain text: backticks, links and other Markdown show literally.
The renderer prints each one in full, so keep each one short:

| Field | Example | Avoid |
| --- | --- | --- |
| `scope` | `Retry queue, its error types, the worker loop, the retry test and the README retry section.` | Restating the merge request description or listing every file |
| `depth` | `Standard: read all changed lines and the callers of RetryQueue::push and Worker::run; not executed.` | A step-by-step narrative; proof details belong in `local_context` |
| `limitation` | `Build and test proof is unavailable; see Baseline Proof Policy. The three findings are confirmed from source.` | Repeating a policy's blocked limitation in full |
| Applicable `reason` | `New fallible send and retry paths.` | `Applies because ...` |
| Excluded `reason` | `Nothing is persisted.` | `Excluded: ...` |
| Blocked or unresolved `limitation` | `No CI pipeline ran for this head and the host cannot build the workspace.` | Restating why the policy applies |

A reason states the fact about this change that decides applicability; the table already shows the outcome.
Write each blocked limitation once, in its policy row, and let the overall `limitation` name the affected policies and what still holds.
A policy limitation equal to its reason or to the overall limitation is shown once.

### Finding confirmations

`confirmations` records independent verdicts. An isolated checker returns one
entry per finding it examined, with the exact version it checked:

```json
{"source": "checker", "finding": 1, "result": "confirmed", "checked": {"id": 1, "...": "complete finding"}, "basis": "Reproduced the dropped request against the reviewed source."}
```

`result` is `confirmed`, `rejected` or `evidence_gap`.

Each round that rkc accepts records in retained GitLab state that its open or
uncertain blocking findings were validated only from such confirmations. A carried
finding's `reusable_confirmation` is `true` when one of those rounds reviewed the
current revision and validated exactly the captured version. Its confirmation then
stands for this run too, and the agent records the reuse:

```json
{"source": "retained", "finding": 1, "basis": "The recorded confirmation covers this unchanged version at this revision."}
```

Validation recorded by a build without confirmation enforcement, or at another
revision, is not reusable; that finding needs a `checker` entry. A publication that
changes nothing writes no round, so it records no new confirmation.

rkc validates an open or uncertain blocking finding exactly when a `confirmed`
checker entry's `checked` version equals its final version in every field except
`validated`, or a `retained` entry names a finding with `reusable_confirmation: true`
whose final version equals its captured version. Such a finding without that evidence stays unvalidated, which
requires a `blocked` decision and generates no finding effects in a blocked review.
`confirmations` and each `basis` stay in local files; they enter no published comment or retained GitLab state.

Checking rejects a second entry for one finding, a blank `basis`, a `checker`
entry whose `finding` differs from its `checked` ID, a `confirmed` entry for a
finding the assessment lacks or whose `checked` version differs from a final open or uncertain
blocking finding, an inapplicable `retained` entry, a changed `reusable_confirmation`, and `validated: true` on an open or
uncertain blocking finding without a matching `confirmed` checker entry or applicable `retained` entry. `rejected` and
`evidence_gap` entries are records that admit nothing; they need not match a final finding. The verdict and
whether to rely on a retained check stay the agent's judgments; rkc compares
versions only.

## Checkpoint revision

`rkc revise --reason TEXT` archives the draft's exact bytes under `draft-history/<hex>/`, then prepares its successor.
`supersedes` names the current accepted assessment; `reason` records TEXT.
Revision never accepts. Complete the successor and run `rkc accept`.
The decision and validation assertions are reset, and recorded confirmations are dropped.
Applicable policy judgments require renewal; excluded and unresolved declarations remain available for affected reassessment.
Findings, needed answers, transitions, scope, depth and private evidence remain available for correction.
The captured prior findings remain authoritative.

`--regenerate` rebuilds from accepted state, or the initial capture when no accepted assessment exists.
It archives and discards unaccepted edits.
Without regeneration, malformed drafts fail with `malformed_draft` and remain unchanged.
A run owning unfinished publication cannot revise or accept a successor.
Changed source or a changed [write target](#publication-and-recovery) requires a new capture, not a revision.

## Publication and recovery

`rkc publish` acts on the selected run's accepted assessment. `--effects` takes
`review`, `none` or a comma-separated list of the authority names above, and
defaults to `none`; `--effects none` plans. Effects never exceed configured
authority, provider permissions or the original round's retained ceiling.

| Command | Behavior |
| --- | --- |
| `rkc publish --effects SET --plan` | Read-only plan: no provider writes, no attempt or dispatch flags, no journal creation |
| `rkc publish --effects SET [--retry-uncertain once\|never]` | Publish, or recover this run's unfinished publication |
| `rkc publish --abandon --reason TEXT [--effects update_summary]` | Close the round of this run's unfinished publication; settles the run as `abandoned` |

The planned writes do not depend on `--effects`. The plan's `effects` object
shows which of them the effective effect set permits. Publication output
carries the same object for its round, read from the journal after the run.

| Field | Meaning |
| --- | --- |
| `requested` | The `--effects` set |
| `configured_authority` | The trusted configuration's `authority` |
| `provider_permissions` | The effects the provider supports for the merge request now |
| `round_ceiling` | `{"round", "effects", "retained": true}` when the journal retains a ceiling for the planned round. `{"round", "effects": [], "retained": false}` when the provider shows that round pending or partial and the journal retains no ceiling for it. Otherwise `null`: the round's first authorized write records the ceiling. This includes a blocked round that finishes in place from an [empty journal](#follow-up-reviews-in-a-new-environment) |
| `effective` | `requested` ∩ `configured_authority` ∩ `provider_permissions` ∩ the round ceiling |
| `summary_writes` | `{"write", "effect", "within"}` for the `start` write (`create_summary` when the reviewer has no summary, otherwise `update_summary`) and any required `final` update; blocked finding actions and pending/partial recovery require that final update; empty for an unchanged plan |
| `actions` | `{"index", "finding", "effect", "within"}` for each planned action; `index` is its position in `run.outcome.plan.actions`, or `run.outcome.result.planned` for publication |
| `outside` | Each effect of a planned write outside `effective`, once |

Publication checks every write again and stops at the first one outside the
effective set, with exit 4; effects verified before it remain. The first
authorized write of a round fixes its ceiling, so a broader later request cannot
complete a round started with a narrower one. Close that round with
`rkc publish --abandon --reason TEXT` and review again in a new run.

New planning and publication require an accepted assessment, an unchanged draft (`dirty_draft`) and intact retained files. Publication also requires
`review-render-profile.json`, written by the latest successful assessment
acceptance or plan, to name the selected profile (`render_profile_changed`). A
run without that file is refused the same way. Run
`rkc publish --effects review --plan` with the selected profile first. Recovery
after a possible dispatch refuses a different recorded profile but needs no
recorded one: its retained intent and pending round already fix the profile.
Offline `check` can render a preview with another profile without changing accepted snapshots, the recorded execution profile or the journal.
Planning, publication and abandonment refuse a profile other than the one that rendered an unfinished round
(`render_profile_changed`). Each such refusal carries `next` in its diagnostic or
in the document. It says that the round continues only with that exact profile,
selected with `--render-profile PATH` or `render_profile` until the round
finishes ([rendering profiles](../../../docs/render-profiles.md#recover-an-interrupted-attempt)).

Before the first write could reach the provider, publication checks that the
assessed snapshot is still current where it writes:

- The current head and comparison must equal the run's revision
  (`source_changed`).
- The write targets must be unchanged since capture (`conversation_changed`).
  The write targets are the existing objects that this publication writes to.
  They are the reviewer's summary note and each finding thread that receives a
  transition reply, an answer, a resolve or a reopen. A new finding thread is
  not a write target, because it does not exist yet.

| Change after capture | Blocks publication |
| --- | --- |
| A note added to, removed from, edited in, or moved into or out of a target thread, by anyone | Yes |
| A resolution change of a target thread | Yes |
| An edit, a move or the removal of the summary note, or a summary note created after capture | Yes |
| A reply in the summary note's discussion | No |
| Any change in a finding thread of the reviewer that receives no planned effect | No |
| Any other note, thread or resolution change outside the write targets, by anyone | No |
| A title or description edit | No |
| Another order of notes or discussions, or another capture time | No |

The diagnostic's `changes` lists only changes to write targets. Publication
does not write where a change does not block; the next review assesses it.
After either refusal, start a new run. Stop a run without a publication binding
first with `rkc run stop --reason TEXT`, then run `rkc run start`. A run whose
binding sent nothing cannot be stopped yet: start the new run with
`rkc run start --new`, and stop the refused run after the new run's publication
releases the binding ([unsent binding](#local-run-state-and-the-journal)).
Each refusal's message names the route for the refused run. A deleted summary
note or finding thread fails discovery first with
[`managed_history_conflict`](#output-and-exit-status).

When the reviewer has no summary, at capture or now, publication writes to no
existing object, so only a source change blocks it. A plan that writes nothing, such as
an exact repeat of the current round, has no write targets either and settles
as `no_changes` without writes.
Publication communicates the assessed snapshot, not later events: its summary
records only the round's own effects and the notes that its answers name.

The freshness preflight runs in this order:

1. Check the accepted assessment offline against the captured history, the
   retained source and the run's own journal progress. The result is the
   captured plan.
2. Discover the merge request. For a new run, refuse another run's unfinished
   publication (`publication_pending_owner`).
3. Compare the current revision with the captured one (`source_changed`).
4. Plan the accepted assessment against the current discovery with the same
   progress. This is the plan that publication executes. When it cannot be
   built, the captured plan names the write targets instead. A change that
   broke the plan, such as a deleted answered note, is then reported as
   `conversation_changed`, and the operation log records why the plan failed.
   The engine's check in step 7 still keeps every write inside the compared
   targets.
5. Collect the write targets of that plan from both the captured and the
   current history, and add the target of a retained write that recovery sends
   first.
6. Capture the current conversation and compare it with the retained one at
   those targets (`conversation_changed`).
7. Load the journal and record `publication_attempted`, then hand the targets to
   the publication engine. The engine discovers and plans again. Before it
   records any intent, it refuses a round that would write to an existing
   summary note or finding thread outside the targets (`uncompared_write`,
   exit `3`). This covers a change of the reviewer's history between the
   preflight and the engine's discovery. Nothing is written; start a new run.

Discovery identifies the reviewer's summary note and finding threads by their
API author and corroborated Review Kit markers. A marker copied into another
author's note grants no ownership and adds no write target.

| Command and run state | Freshness | Targets |
| --- | --- | --- |
| `publish --effects ...` of a new run | Steps 1 to 7 | The plan against the current discovery |
| `publish --effects ... --plan` of a new run or of a binding that sent nothing | Steps 1 to 6; planning writes nothing, so the engine receives no targets | As for the matching `publish` |
| `publish` of a binding none of whose writes could have reached GitLab | Steps 1 to 7; refusals carry the [unsent-binding](#local-run-state-and-the-journal) `next` | The plan of the bound round with the journal's progress, and the retained prepared write that is resumed first |
| `publish` or `--plan` after a possible dispatch | Skipped; the engine's per-effect checks apply | None; once a write of the binding may have reached GitLab, recovery completes the retained round under those checks |
| `publish --abandon` | None; abandonment only closes the bound round in the summary | None |
| A settled run | None; the command reports `already_settled` | None |

Publication holds the workspace lock, then the subject/reviewer journal lock
(`publication_busy` when held). Its durable order is: read the journal; discover
the merge request; refuse another run's unfinished publication; check freshness;
load the journal, creating an empty one only for a run that never attempted
publication, a store without this review's directory and provider history that
[permits an empty journal](#follow-up-reviews-in-a-new-environment)
(`journal_missing` otherwise); record `publication_attempted: true` in the manifest; bind the journal
to this run's accepted assessment and retained-file hashes together with the
first prepared write; set the journal's dispatch flag before the first request
could leave; verify each effect by readback; record the run's settlement in the
journal; and only then complete the manifest.

A run attempted publication when its manifest records `publication_attempted`
or its own `review-publication-events.json` records a journal checkpoint, a write
intent or a publication run. Without its journal, such a run never starts over:
`publish`, `--plan` and `--abandon` fail with `journal_missing` before any
provider read, and `next` says to restore the authoritative publication state.
rkc never recreates that journal, because a write of the earlier attempt may have
reached GitLab.

### Local run state and the journal

The journal `<publication_state>/<review-key>/publication-progress.json` is the
authority for publication. It records the binding to the owning run, the
uncertain write and its dispatch phase, the retained effect ceiling, consumed
retries and one settlement per run (`published`, `no_changes` or `abandoned`).
The run manifest's lifecycle and `publication_attempted` flag are local state;
the flag only records that an attempt began and is never cleared.

| Journal says about the run | `accept`, `revise`, `run finish`, `run stop` | `publish` |
| --- | --- | --- |
| Settled | Repair an open manifest to the settled completion, then reject as `run_terminal` | Repair and report `already_settled` with exit 0; `--plan` reports without repairing |
| Settled, but with another accepted assessment or other retained-file hashes than the run, such as a restored older copy | Reject with `binding_mismatch`; nothing changes | Reject with `binding_mismatch`, also with `--plan` and `--abandon` |
| Owns unfinished publication with a possible provider write | Reject with `publication_pending_owner` | Recover with the frozen accepted bytes; drafts are ignored |
| Owns its binding, but no write could have been sent | Reject with `publication_pending_owner` and `provider_write_possible: false` | Resume after the freshness preflight; `--abandon` has no round to close |
| Unbound | Allowed | New publication |
| Journal unreadable and the run attempted publication | Reject with `journal_unavailable` | Reject |
| Current journal absent and the run attempted publication | Reject with `journal_unavailable` | Reject with `journal_missing`; restore the authoritative state, never recreate it |
| Current journal absent and the run never attempted publication | Allowed | New publication when the provider history [permits an empty journal](#follow-up-reviews-in-a-new-environment); otherwise `journal_missing` |
| Journal unreadable and the run never attempted | Allowed | Reject |

`status`, `run list` and `check` never repair or change lifecycle. A settled
outcome is never relabelled `local_only` or `stopped`.

For a run that owns unfinished publication with a possible provider write,
`status` checks the local files that recovery reads before it suggests
`rkc publish`. Recovery and abandonment both read the retained files, so `next`
first names any retained file to restore. Recovery also publishes from the exact
accepted snapshots and fails with `accepted_snapshot_missing` or `hash_mismatch`
without it, so `next` names the missing or damaged assessment with its digest
and `accepted/<hex>/` directory to restore from a copy of the run. Abandonment
does not read accepted snapshots. `next` offers it only as the alternative when
they cannot be restored, because it ends the round publicly with its reason.

While another run owns unfinished publication, other runs can draft, accept and
finish locally, but their planning and publication fail with
`publication_pending_owner` naming the owner in `owner_run_id`. This check reads
the journal after discovery and before the freshness preflight, so it also
applies when the owner's partial publication changed this run's write targets;
its `next` says how to recover the owner. Recover the owner first:

```sh
rkc status
rkc run select <owner run ID>
rkc publish --effects review
```

A run captured before the owner's publication then fails `conversation_changed`
when it would write to the summary note or a thread that the owner's publication
changed, or `source_changed` after a new head; start a fresh run.

When the owner has no unresolved write and the provider shows its bound round
terminal, the next run's publication records that settlement and proceeds.
When none of the owner's writes could have reached the provider (its dispatch
flag is clear and any intent is only prepared), the next run's publication
releases that binding and restores the earlier effect ceiling, so the new run is
not limited by the owner's grant. That run's `--plan` reads the journal as the
release would leave it, without changing the journal, and names the owner in
`publication_owner_released`. This is the way forward when the owner's source
or one of its write targets changed after a crash:

```sh
rkc run start --new
# complete review-assessment.draft.json, then run rkc accept
rkc publish --effects review --plan
rkc publish --effects review
rkc run select <owner run ID>
rkc run stop --reason TEXT
```

The owner's own `rkc status` reports `journal.publication_status: "bound_unsent"`
and `journal.provider_write_possible: false`. While the binding is unsettled,
`rkc status` of any run of the same merge request and reviewer reports the
owner's run ID as `journal.publication_owner` and
`journal.owner_provider_write_possible: false`.
After a possible dispatch, recovery skips the freshness preflight and applies the
engine's per-effect source, answer-target, ownership and permission checks; an
unrelated new comment does not stop it. A new head or base stops remaining stale
effects after earlier ones are reconciled: publication exits 3 with the top-level
diagnostic `changed_revision`, whose `next` names the round to close with
`rkc publish --abandon --reason TEXT` once no write is uncertain, before a fresh
run. Abandonment needs current summary-update authority and a public reason.

### Uncertain writes

A write that is only `prepared` never reached GitLab: the dispatch flag and the
dispatch checkpoint are persisted before any request can leave. When recovery or
abandonment finds that such a write can no longer resume, for example because its
question was edited or its revision is stale, rkc discards it, records a
`discarded_unsent` event in `review-publication-events.json` and continues from
the current plan. The binding, the dispatch flag, the effect ceiling and consumed
retries stay unchanged. When the current plan then refuses the round, such as
with `invalid_answer` for an edited question, `next` says to close it with
`rkc publish --abandon --reason TEXT`; a fresh run answers the current question.
A write that may have been dispatched is never discarded.

`--retry-uncertain once` (default) permits one resend of an absent uncertain
summary creation, finding or reply after complete reconciliation reads. The
consumed retry survives restarts. A late original request can still arrive, so a
resend can produce a duplicate; Review Kit never deletes duplicates and does not
promise exactly-once writes. `--retry-uncertain never` leaves such creations
unresolved. Updates are reconciled by observed state and never resent blindly.

A creation that may have been dispatched but never appears can be resent only on
the reviewed revision and only once. After the head or base changes, or after
that one resend is consumed, it stays uncertain. It then blocks this reviewer's
publication for the merge request: the owner's recovery exits 6, other runs fail
with `publication_pending_owner`, and abandonment refuses while the write is
uncertain and cannot close a round that GitLab never received
(`not_abandonable`). Review Kit never declares such a write lost. The options are
to restore the original revision so that recovery can complete, to wait until a
late arrival becomes observable, or to review with a different reviewer identity.
In this state, the `next` of the owner's `publish` and refused `--abandon`, and
of other runs' `publication_pending_owner` refusals, says that the write stays
uncertain and lists these options instead of asking to publish again. It names
the reviewed head to restore only while the one resend is unused. While a resend
is still possible, `next` keeps asking to run `rkc publish` again.

### Follow-up reviews in a new environment

A new container or machine can start with an empty `publication_state` and no
review runs. A run there that never attempted publication creates an empty
journal on its first `rkc publish` (`journal_origin: "bootstrapped"`) when the
reviewer has no Review Kit records on the merge request, or when the reviewer's
only summary shows its latest round finished. The
[empty journal admission](../../lib/application/README.md#empty-journal-admission)
lists the finished states and explains why neither the earlier journal nor the
earlier run's files are needed. The run then publishes like any follow-up:
`run start` captures the carried findings with their IDs, and the reviewer's one
summary records the new round with the earlier rounds in its history. An
unchanged assessment writes nothing and settles as `no_changes`.

When the reviewer's summary shows its latest round `Pending` or `Partial`, or
the reviewer's records do not show one current round, `--plan` and `publish`
fail with `journal_missing` before any write. The diagnostic's `round` names the
unfinished round (`null` when there is none) and its `next` gives the reason:
only the publication state that started the round holds its possible unresolved
write and effect ceiling. When another environment is still publishing, let it
finish and start a fresh run with `rkc run start --new`. Otherwise
[move that publication state](#moving-publication-state) here together with the
run that started the round, and recover that run. rkc neither finishes nor
closes the round without that state. While such a round is unfinished, `check`
and `accept` reject any assessment other than the round's exact one
(`changed_pending_assessment`). Without a journal here, that rejection's `next`
says GitLab shows the round unfinished and gives the same reason and options
instead of asking to fix the draft, because accepting the round's assessment
only leads `publish` to `journal_missing`.

An empty state root cannot show that no other environment has a write in
flight. Let one environment at a time publish for a merge request and reviewer.

### Moving publication state

Continuing unfinished publication in another container or machine requires
stopping every writer in the old environment, restoring the run directory at the same
`review-runs/<run-id>/` path together with the journal from a consistent copy,
and using exactly one authoritative state root afterwards. The old copy must not
resume. Moving the old state root aside disables a transferred run that attempted
publication: without its journal it fails with `journal_missing` instead of
starting an empty one. A run there that never attempted publication would still
create a new journal when the provider history
[permits one](#follow-up-reviews-in-a-new-environment), so nothing in the old
environment may publish. `run select`
checks that the journal's binding or settlement names the run's accepted inputs
(`binding_mismatch`). Locks are local operating-system locks; separate hosts need
external serialization, and Review Kit has no distributed lock. An internally
consistent stale copy of a transferred run and its journal is indistinguishable
from the authoritative copy without external coordination; if it runs after the
authoritative copy completed, it settles against the provider state like the
authoritative copy. A run ID identifies state; it does not locate or download it.

## Output and exit status

Every command writes one JSON document to standard output, including invalid
arguments and failures. `--help` and `--version` print text with exit 0.
Documents carry `version` and, for run commands, `command`, `status`, `run_id`,
`run_directory` (relative to the worktree root) and `next` where they apply.
Artifact paths such as `draft`, `path` and `archived_draft` are relative to the
run directory. Failures use
`{"version": 1, "status": "error", "diagnostics": [{"code", "message", "causes", ...}]}`.
A retained file that decodes but fails validation gives
`{"version": 1, "status": "invalid", "diagnostics": [...]}` with exit `2`, for
example `unsupported_version` or `invalid_value`. Some diagnostics add fields
such as `run_ids`, `owner_run_id`, `round`, `files`,
`drafts`, `stage` or `next`. The configured destination carries
[structured redacted operation logs](../../lib/observation/README.md); see
[logging preferences](#logging-preferences). Log delivery failures do not change
results or exit codes.

| Command | Main output fields |
| --- | --- |
| `run start` | `status: "created"`, `run_id`, `subject`, `reviewer`, `revision`, `run_directory`, `carried_findings`, `next_finding_id`, `other_open_runs`, `draft` |
| `accept` | `version`, `run_id`, `status` (`stored` or `existing`), `reference`, `path`, `manifest`, `diagnostics`, previews and plan counts; invalid drafts give `status: "rejected"` with diagnostics and exit 2 |
| `check` | `status` (`valid` or `invalid`), `checked` digest, `diagnostics`, `accepted`, `recorded` (false for a terminal run); a valid assessment adds `previews`, `render_profile` and, when valid, `plan` counts and the offline `check.plan` |
| `revise` | `status` (`prepared`, `malformed_draft`, `blank_reason`, `revision_failed`), `draft`, `supersedes`, `archived_draft`, `diagnostics` |
| `run select` | `status: "selected"`, `selected_run_id`, `run_status`, `completion`, `reason`, `assessment`, `journal`, `publication_owner` |
| `publish` | `mode`, `status` (`planned` or `failed` for a plan; `settled`, `unfinished`, `already_settled`, `settlement_rejected`), `settlement`, `publication`, `effects` (see [publication](#publication-and-recovery); `null` without a plan or publication result), `run` (the full report), `progress` (journal path), `storage_failures`; a plan adds `publication_owner_released` and `publication_owner_settled_by_provider` |
| `status` | lifecycle, `assessment` (`state`, `draft`, `accepted`; see [acceptance](#assessment-acceptance)), `retained_files` (`state` is `intact`, `mismatch`, `newer` or `damaged`, with `files`; `newer` means that a newer rkc wrote the [captured conversation](#captured-conversation) and no other retained file is damaged), `journal` (`state`, `publication_owner`, `owner_provider_write_possible`, `settlement`, `publication_status` (`pending` or `bound_unsent`), `provider_write_possible`, `round`, `dispatch_may_have_started`, `unresolved_effect`) |

Publication output separates the agent's decision, publication status, attempted
and verified effects; [result inspection](#result-inspection) shows how to read them.

| Exit | Meaning | Codes |
| --- | --- | --- |
| `0` | Success; inspect publication status | `already_settled` |
| `2` | Invalid arguments, configuration, environment, input or rendering | `arguments`, `configuration`, `discovery`, `not_in_git_worktree`, `runtime_tracked`, `runtime_redirected`, `runtime_unrecognized`, `no_selected_run`, `invalid_run_id`, `run_missing`, `run_file_missing`, `run_identity_mismatch`, `identity_mismatch`, `repository_mismatch`, `hash_mismatch`, `accepted_snapshot_missing`, `invalid_accepted_snapshot`, `invalid_run_document`, `newer_run_document`, `unsupported_version`, `assessment_not_accepted`, `dirty_draft`, `blank_reason`, `render_profile`, `render_profile_changed`, `gitlab_body`, `review_workflow`, `subject` |
| `3` | Reviewed source or a write target changed before dispatch, the engine's round would write outside the compared write targets, or the revision changed during publication or recovery | `source_changed`, `conversation_changed`, `uncompared_write`, `changed_revision`, `discovery`, `gitlab` |
| `4` | Reviewer or authority mismatch | `wrong_reviewer`, `discovery` |
| `5` | Busy locks, run, journal or managed history state conflicts | `workspace_busy`, `publication_busy`, `open_run_exists`, `run_terminal`, `publication_pending_owner`, `binding_mismatch`, `no_unfinished_publication`, `journal_unavailable`, `journal_missing`, `run_journal_exists`, `settlement_conflict`, `managed_history_conflict`, `unsupported_retained_state_version`, `discovery`, `gitlab` |
| `6` | Unknown write, unresolved intent or readback mismatch | publication `stop` diagnostics |
| `7` | Filesystem, evidence, clock, encoding, transport, provider read or rejected-write failure | `filesystem`, `evidence`, `clock`, `encoding`, `progress_storage`, `result_storage`, `discovery`, `gitlab` |

`managed_history_conflict` means the reviewer's own Review Kit records on the
provider contradict each other, so `run start`, planning, publication and
abandonment stop without writing. The diagnostic adds `conflict`, `next` and
`finding` (the finding number, or `null` when the conflict names no single
finding). A `duplicate_finding` diagnostic also lists every root note that claims
the finding as `roots: [{"note", "created_at"}]` in GitLab's order, or `null` when
GitLab could not list them:

| `conflict` | Provider state |
| --- | --- |
| `missing_finding_root` | The finding's root note is gone while the summary or the reviewer's replies still record it, such as after the root was deleted |
| `duplicate_finding` | More than one managed root note claims the finding |
| `inconsistent_history` | Summary, finding or reply records disagree, or the retained summary data is invalid |

When the reviewer's summary names a retained-state version that this executable
does not read, `run start`, planning, publication and abandonment stop with
`unsupported_retained_state_version` (exit `5`) instead of
`managed_history_conflict`. The diagnostic adds
`state_version`, `supported_state_version` and `next`. Retain the provider
records and use an executable that supports their version.

Review Kit does not repair provider history: it never treats the finding as
fixed or recreates it, and a new run or policy declaration does not clear the
conflict. Report the blocked operation and preserve the records. Provider-history
repair or a different reviewer identity requires separate operator authority;
the diagnostic grants neither action.

A duplicate root is the documented risk of a resend: the original creation
arrived late, after the one bounded resend. Review Kit never deletes duplicates.
For a separately authorized repair, delete on GitLab the root that this reviewer's
run never verified, then start a new run with `rkc run start --new`. The verified root is
the one in the publishing run's finding readback records; when that record is
unavailable, delete the root with the later `created_at`:

```sh
jq --argjson f 1 '[.events[] | select(.kind == "readback" and .write.kind == "finding"
  and .write.finding.id == $f) | .snapshot.findings[] | select(.finding.id == $f)
  | .root_note] | unique' .review-kit/review-runs/<run ID>/review-publication-events.json
```

Other provider failures during discovery keep the `discovery` code with their own
exit status: 7 for transport, authentication and provider read failures, 5 for
conflicting provider state outside the reviewer's managed records or a wrong
subject, 4 for a wrong reviewer, 3 when the revision changes while discovery
reads it, and 2 for invalid GitLab configuration.

Assessment diagnostics include `malformed_draft`, `missing_draft`, `run_binding`, `revision_mismatch`,
`stale_successor`, `incomplete_policy_declaration`, `missing_active_finding_policy`,
`required_policy_excluded`, `local_context_too_long`, `changed_reusable_confirmation` and existing assessment consistency codes.
[Confirmation](#finding-confirmations) diagnostics are `invalid_confirmation`,
`unknown_confirmation_finding`, `duplicate_confirmation`, `blank_confirmation_basis`,
`stale_confirmation`, `invalid_retained_confirmation` and `unconfirmed_blocking_finding`.
The [JSON adapter](../../lib/infrastructure/json-adapter/README.md) owns schemas.
The [workflow](../../../docs/workflow.md#command-contract) defines compatibility boundaries.

### Result inspection

Read a result in this order: the exit status, the top-level document, then the
nested report. Save standard output, and keep the whole file when the exit status
is not 0:

```sh
rkc publish --effects review > result.json; echo "exit $?"
```

Each `jq` example below prints one complete section. Select fields by the exact
paths that the examples use. A document also carries earlier rounds, receipt
snapshots and earlier bodies, so a recursive search such as `.. | .actions?`
mixes current and historical records.

**Outer errors.** A document without `command` is an outer error. The command
failed before it produced a result, so no nested report exists.

```sh
jq 'if has("command") then {command, status} else {error: .status, diagnostics} end' result.json
```

Each diagnostic has `code` and `message`. Most diagnostics also have `causes` and
code-specific fields such as `changes` or `owner_run_id`. A nonzero exit status
alone does not mean an outer error: `check` and `accept` return their complete
document with exit 2 for an invalid draft, and `publish` returns its complete
document with exit 4 to 7 when publication stops or is refused.

**Assessment checks.** `check` reports `status` `valid` or `invalid`.
`accept` reports `stored`, `existing` or `rejected`. Read every
diagnostic, not only the first:

```sh
jq '{command, status, diagnostics}' check.json
```

Each assessment diagnostic has `code` and `message`; its `finding`, `policy` and
`input` fields name what it concerns, or are `null`.

**Online plan.** `publish --effects SET --plan` plans against current GitLab
state. When `status` is `planned`, the plan is `.run.outcome.plan`; when it is
`failed`, read the failure details below. List the actions first; indexes start
at 0:

```sh
rkc publish --effects review --plan > plan.json
jq '.run.outcome.plan | {round: .round.number, unchanged, unresolved_findings,
  actions: [.actions[] | {kind, finding: (.finding.id? // .finding)}]}' plan.json
```

Then read one action per command: its complete target, then its complete body.

```sh
jq --argjson i 2 '.run.outcome.plan.actions[$i] | del(.body)' plan.json
jq -r --argjson i 2 '.run.outcome.plan.actions[$i].body // empty' plan.json
```

| Action `kind` | Target |
| --- | --- |
| `finding` | The `finding` with its `location` and any `suggestion` (`start_line`, `original`, `replacement`, `revision`) |
| `reply` | The thread of finding `finding`; `reply.kind` is `transition` with the new `status`, or `answer` with the answered `note` |
| `set_resolved` | The thread of finding `finding` and its new `resolved` state; the action has no body |

Read each summary body with its own command:

```sh
jq -r '.run.outcome.plan.summary_body' plan.json
jq -r '.run.outcome.plan.terminal_summary_preview' plan.json
```

The plan output, not a preview file, is the current source of what publication
sends. `rendered/review-summary.md` and `rendered/terminal-review-summary.md`
show the latest valid `check` or `accept`, which use the
observations retained by `run start`. An online plan never rewrites these files,
so they can differ from the plan, for example during recovery. `check`
returns its offline plan as `.check.plan`.

**Publication outcome.** Only `status` `settled` or `already_settled` means that
the run's publication is finished. Every other `status` leaves it unfinished, and
`publication.status` reports what happened:

```sh
jq '{status, settlement, publication: .publication.status,
  stop: [.run.outcome.result.stop[]?.code],
  finalization_stop: [.run.outcome.result.finalization_stop[]?.code]}' result.json
```

| `status` | `publication.status` | Meaning |
| --- | --- | --- |
| `settled` | `completed` | Every planned effect is verified; `settlement.outcome` is `published` |
| `unfinished` | `partial` | Some effects are verified, and `stop` or `finalization_stop` ended publication; `.run.outcome.result.skipped` lists actions not performed |
| `unfinished` | `uncertain` | A dispatched write has no verified, recorded readback; recover this run first |
| `unfinished` | `denied` | Missing authority stopped the first write; nothing was sent |
| `unfinished` | `not_started` | GitLab rejected the first write, or a read or evidence record failed before any effect |
| `unfinished` | `rejected` | Publication was refused before execution; `.run.outcome.kind` is `failed` |
| `already_settled` | (absent) | The journal settled this run earlier; there are no provider writes and no `run` report |

A `settled` run can also report `unchanged`, `blocked` or `superseded`;
`settlement.outcome` is then `published` or `no_changes`. `.run.outcome.kind` is
`published` whenever publication ran, also for an unfinished result.
`already_settled` reports `repaired: true` when it recorded the journal's
settlement in an open manifest. `settlement_rejected` means that the journal
refused the settlement; `settlement.error` names the cause. Recover an
unfinished run as its `next` field says, with the original effects.

**Failure details.** Each failure has one home. Quote the messages unchanged:

```sh
jq '{stop: .run.outcome.result.stop, finalization_stop: .run.outcome.result.finalization_stop,
  refused: .run.outcome.diagnostics, evidence_failure: .run.evidence_failure,
  storage_failures, diagnostics}' result.json
```

| Path | Reports |
| --- | --- |
| `.run.outcome.result.stop` | The diagnostic that ended execution, such as `write_unknown`, `readback_mismatch`, `provider_unavailable`, `authority_denied`, `write_rejected` or `evidence_unavailable` |
| `.run.outcome.result.finalization_stop` | A failed final summary update after the effects |
| `.run.outcome.diagnostics` | Why planning or publication was refused before execution |
| `.run.evidence_failure` | The final execution record was not saved; the reported result is still the actual outcome |
| `storage_failures` | A file not saved after the result: `progress_storage` for the journal, `result_storage` for `review-publication-result.json` |
| `diagnostics` | A top-level diagnostic of a complete document: `changed_revision` with `next` when a new head or base stopped publication, `uncompared_write` with `next` when the engine refused a round that would write outside the compared write targets, or `publication_pending_owner` with `owner_run_id` |

A readback failure is a `stop` with `provider_unavailable` after a dispatched
write. That write is in `attempted` but not in `verified`, and
`publication.status` is `uncertain`. `readback_mismatch` means that the readback
succeeded but showed another state. After `result_storage`, keep standard output:
`review-publication-result.json` does not hold this report.

**Attempted, verified and observed writes.** `attempted` lists the writes that
this invocation dispatched. `verified` lists the effects that this invocation
confirmed by readback, each as its `write` and the `readback` snapshot. Match
them by equal `write` values:

```sh
jq 'def id: {kind, target: (.finding.id // .action.finding // .thread)};
  .run.outcome.result | .attempted as $sent | [.verified[].write] as $seen
  | {sent_and_verified: [$seen[] | select(IN($sent[])) | id],
     observed_only: [$seen[] | select(IN($sent[]) | not) | id],
     sent_unverified: [$sent[] | select(IN($seen[]) | not) | id],
     resent: [.retried_creations[]? | id]}' result.json
```

- `sent_and_verified`: sent and confirmed by this invocation.
- `observed_only`: already visible, so this invocation sent nothing for it.
  Recovery reports a write of an earlier invocation this way.
- `sent_unverified`: sent without confirmation; read `stop`.
- `resent`: creations sent again after uncertainty. A late original request can
  still arrive, so they can have duplicates even when publication succeeds.

`target` is a finding ID, a thread ID, or `null` for the summary. Earlier
invocations' attempts and readbacks stay in `review-publication-events.json`, not
in this result.

**Final evaluations.** The final evaluation list of the current run is
`.run.final_assessment.evaluations` in its latest plan or publication output. It
holds one `judgment` per applicable or unresolved policy of the accepted
assessment named by `.assessment`. Use it only when `.assessment` equals
`.assessment.accepted` in `rkc status`. Compute policy totals only when
they are requested, and keep them apart from finding counts:

```sh
jq '.run.final_assessment | {decision,
  policies: (.evaluations | group_by(.judgment.kind) | map({(.[0].judgment.kind): length}) | add),
  findings: (.findings | group_by(.status) | map({(.[0].status): length}) | add)}' result.json
```

Each finding names one policy, several findings can share a policy, and a
violated policy can have no finding. Never derive policy totals from findings,
comments or summary tables.

**Saved files.** `review-publication-result.json` holds only the `.run` report
of the latest plan or publication, so its paths drop the `.run` prefix, such as
`.outcome.plan.actions` and `.final_assessment.evaluations`. It lacks `status`,
`settlement`, `publication`, `assessment`, `storage_failures` and top-level
`diagnostics`. After `publish --abandon`, it holds the `.abandon` document.
Outer errors, `already_settled` and a `result_storage` failure leave the earlier
file unchanged. `review-check-result.json` is the complete standard output of
the latest `check` or `accept` of an open run.
