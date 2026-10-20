# Review Kit

Review Kit is a CLI and a set of Rust libraries for publishing and updating GitLab code reviews.
It captures a merge request into a review run and freezes one complete assessment.
Each policy appears once with its selection, relevance and result.
The tool checks that assessment against the reviewed revision and turns it into a summary and inline comments.
Each finding keeps its ID and GitLab discussion across revisions, including fixes, withdrawals and reopenings.

The executable is `rkc`. It handles capture, checkpoint validation, source positions,
comment formatting, publication and saved progress. Review judgments are inputs;
Review Kit does not analyze code for bugs.

## Contents

- [Install and run](#install-and-run)
- [CLI usage](#cli-usage)
- [Start a review run](#start-a-review-run)
- [Declare policies](#declare-policies)
- [Check an assessment](#check-an-assessment)
- [Change comment formatting](#change-comment-formatting)
- [Publish comments](#publish-comments)
- [Interrupted publication](#interrupted-publication)
- [Rust libraries](#workspace-layout)
- [Documentation](#documentation)
- [Workspace checks](#workspace-checks)

## Install and run

Download the **Linux x86-64 binary** from [Releases](https://github.com/sformisano/review-kit/releases/tag/v0.1.2).
The [installation guide](docs/distribution.md#download-and-install) covers build identity,
download verification, installation, upgrades and rollback. The binary includes its rendering templates
and schemas; it needs no Rust installation or separate json2md executable.
The [0.1.2 release notes](docs/releases/0.1.2.md) describe that binary.

From a checkout of this repository, with Rust 1.98 or later:

```sh
cargo install --path crates/bin/cli --locked
rkc --help
```

Cargo's binary directory must be on your `PATH`.

See the [platform limits](docs/releases/0.1.2.md#verification-and-limits) for verified
Linux and GitLab versions.

## CLI usage

Save configuration in `~/.review-kit/config.toml`. It selects the GitLab server,
token environment variable, allowed operations and publication
state directory. The [configuration reference](crates/bin/cli/README.md#trusted-configuration)
includes a complete example, explicit overrides and existing JSON configuration support.

Commands return JSON. `--help` and `--version` return text.
Structured redacted operation logs append to the configured file by default.
The [logging preferences](crates/bin/cli/README.md#logging-preferences) define the default path and stderr fallback.
The [logging contract](crates/lib/observation/README.md) defines fields and collection limits.
See [log collection](crates/lib/observation/README.md#collect-cli-logs) to connect an external logging agent.

Run commands work on the Git worktree that contains the current directory.
They take no directory or file arguments.

| Command | What it does |
| --- | --- |
| `run start --mr URL` | Captures the merge request into a new review run and selects it |
| `check` | Diagnoses the complete assessment draft without accepting it |
| `accept` | Validates the complete assessment and freezes its exact bytes |
| `revise --reason TEXT` | Archives the assessment draft and prepares its successor |
| `status`, `run list`, `run select ID` | Reports the selected run, lists stored runs or selects another run |
| `publish --plan` | Reads current GitLab state and returns the planned comments and discussion changes |
| `publish` | Publishes or recovers the selected run and verifies each effect |
| `publish --abandon --reason TEXT` | Closes the round of the selected run's unfinished publication |
| `run finish`, `run stop --reason TEXT` | Completes or stops a run without provider writes |
| `render-profile export` | Exports editable JSON Schemas and Jinja templates without configuration or credentials |

The [review-run reference](crates/bin/cli/README.md#review-runs) defines every
option, the run lifecycle and the runtime files.
Publication uses the single accepted assessment of a review run.

A **review run** is one capture of a merge request and the agent's checkpoints for it.
A **round** records one assessment and its publication progress.

## Start a review run

```sh
rkc run start --mr https://gitlab.example.com/team/project/-/merge_requests/7
```

`run start` captures the current comparison, committed source, diff, verified
managed review history and supported conversation. It stores them in a new run
directory, selects the run and writes the first draft. The result reports the
run ID, carried findings and the next free finding ID.

Runtime files live in `.review-kit/` at the worktree root, which rkc keeps out of Git.
The agent edits one file in the selected run:

```text
.review-kit/review-runs/<run-id>/review-assessment.draft.json
```

`run start` writes that draft with the captured run and revision binding.
The [runtime file reference](crates/bin/cli/README.md#runtime-files) lists the
captured files, accepted snapshot and results stored beside it.

While an open run exists for the same merge request and reviewer, `run start`
fails with `open_run_exists` and lists it. Continue that run, or pass `--new` to start another.

## Declare policies

The agent reads its policy-list skill and selected definitions, then records each policy once in the assessment.
Each row includes its identity, required flag, relevance and evaluation result.
Excluded policies retain their reasons. Unavailable coverage remains explicitly blocked.
A follow-up includes the policy of every earlier finding that is still open or uncertain.

rkc validates declared coverage and assessment consistency without fetching policy definitions.
It cannot detect a policy the agent omitted or establish that its judgments are correct.
The [assessment contract](crates/bin/cli/README.md#assessment-editing) defines the fields and rules.

## Check an assessment

```sh
rkc check
```

`check` validates the selected run's assessment draft against its
retained context and committed source without contacting GitLab.
It rejects incomplete or inconsistent input, including:

- Duplicate policy rows or inconsistent relevance and evaluation results.
- Missing judgments for declared policies or earlier findings.
- A `ready` decision with an open blocking finding.
- A new finding whose location is outside the reviewed diff.
- A suggestion whose original lines differ from the committed source.
- A finding transition without its required explanation.

For example, an assessment that marks the review ready while leaving F1 open
and blocking exits with code `2`. The JSON result reports `"status": "invalid"`
and includes this diagnostic:

```json
{
  "code": "ready_with_blocking_finding",
  "finding": 1,
  "input": null,
  "message": "ready decision conflicts with open blocking finding 1",
  "policy": null
}
```

A successful check exits with `0` and reports `"status": "valid"`. Its JSON
summarizes the planned operations. It also writes summary previews in the run directory:

```text
rendered/review-summary.md
rendered/terminal-review-summary.md
```

The renderer formats finding IDs, severity labels, policy names and summary tables
from the assessment data. `local_context` stays out of the rendered comments.
An invalid check removes the previews, so earlier previews never stand for a current result.
`rkc accept` applies the same checks and freezes the assessment.

## Change comment formatting

Export the built-in profile, edit its templates, then select it when checking:

```sh
rkc render-profile export --dir ./review-style
rkc --render-profile ./review-style/profile.json check
```

Profiles contain schemas and templates for findings, summaries, transitions,
answers and suggestion blocks. The same profile renders checks and publication.
Changing the files needs no rebuild or separate renderer executable.

The [profile guide](docs/render-profiles.md) covers configuration, editing,
publication and recovery. It includes a complete example profile.

## Publish comments

Accept the assessment, then plan against current GitLab state without writing comments:

```sh
rkc accept
rkc publish --effects review --plan
```

Publish the same accepted assessment:

```sh
rkc publish --effects review
```

Publication requires an accepted assessment, a draft equal to its frozen snapshot and intact captured files. It also checks that
the requested operations are allowed by the configuration. GitLab also checks the
account's permissions and token scope.

Before the first write, the current head and comparison must still equal the
run's capture. The summary note and the finding threads that the publication
writes to must also be unchanged. A change fails with `source_changed` or
`conversation_changed` (exit `3`); stop that run with
`rkc run stop --reason TEXT` and assess the change in a new run with
`rkc run start`. Activity elsewhere does not block, including replies to
the summary and changes in the reviewer's threads that the publication leaves
alone. The [publication reference](crates/bin/cli/README.md#publication-and-recovery)
defines the rule.
Publication communicates the assessed snapshot, not later events.

For a first review with findings, Review Kit:

1. Creates a summary that says its comments are still being posted.
2. Posts each finding at its checked diff position.
3. Updates the summary with the verified finding links and completes the round.

Each write is read back from GitLab before the next step.
The summary shows the supplied review decision. Completed publication does not
change it, so a completed round can still say **Changes required**.
The run then completes as `published`. Publishing it again reports `already_settled` and writes nothing.

Later runs use the existing findings and discussions:

| Assessment input | GitLab operation |
| --- | --- |
| A new finding | Create an inline discussion |
| A finding changes from open to fixed | Post the transition explanation and resolve its discussion |
| A finding changes from fixed to open | Post the transition explanation and reopen its discussion |
| An answer to a retained reply | Post the answer in that discussion |
| A completed assessment and its comments are unchanged | Verify the existing comments; write nothing, create no new round and complete the run as `no_changes` |

The summary lists verified finding states and actions. Each earlier round has
its own collapsed section, preserving the states communicated in that round.
Review Kit manages comments and discussion state. It does not approve or merge
requests, delete comments, or apply suggestions.

Without `--effects`, `publish` defaults to `none` and only plans.
The requested effects cannot expand the configuration's allowed operations.
A run that is not published ends with `rkc run finish` or `rkc run stop --reason TEXT`.
See the [output reference](crates/bin/cli/README.md#output-and-exit-status) for
publication status, partial results and exit codes.

## Interrupted publication

GitLab can accept a comment even when its response never reaches the client.
Review Kit keeps one publication journal per merge request and reviewer in the
configured publication state directory, shared by every run and worktree.
Before the first write, the journal is bound to the run's accepted assessment and captured files.
Each intended write is saved before it is sent, then verified.
The journal records the run's outcome before the run's own files are updated.

To recover, select the run that owns the unfinished publication and publish it again:

```sh
rkc status
rkc run select <owner run ID>
rkc publish --effects review
```

Recovery uses the frozen accepted bytes and reuses verified effects. It can make
one controlled retry of an absent creation. That retry can produce a duplicate
if the original request arrives late. While the owner's publication is unfinished
and one of its writes could have reached GitLab, other runs can draft and finish
locally, but their publication fails with `publication_pending_owner`. When none of the owner's writes could
have been sent, the next run's publication releases the owner's binding.
The [recovery reference](crates/bin/cli/README.md#publication-and-recovery)
defines the retry policy, the conservative option and retained permission limits.

`rkc publish --abandon --reason TEXT` closes an unfinished round with a public
explanation, but it cannot discard a write whose outcome is unknown.
Local changes to a worktree's runs hold its workspace lock, and publication also
holds a local lock for the merge request and reviewer.
Moving unfinished publication to another container or machine requires a consistent copy
of the run and the journal, and exactly one writer afterwards. Review Kit has no distributed lock.
After the reviewer's latest round finished, a
[new environment](crates/bin/cli/README.md#follow-up-reviews-in-a-new-environment)
can publish a follow-up review without either.
Multiple GitLab writes are not an atomic transaction.

Long commands report [progress on stderr](crates/bin/cli/README.md#progress-output); stdout holds the command's JSON result.

## Workspace layout

The Rust libraries can be used independently of `rkc`:

| Crate | Responsibility |
| --- | --- |
| [`review-kit-observation`](crates/lib/observation/README.md) | Scoped, redacted operation logs |
| [`review-kit-domain`](crates/lib/domain/README.md) | Findings and assessment consistency |
| [`review-kit-application`](crates/lib/application/README.md) | Checking checkpoints and reviews, coordinating publication and its run binding |
| [`review-kit-gitlab-adapter`](crates/lib/infrastructure/gitlab-adapter/README.md) | GitLab API calls, diff positions and conversation capture |
| [`review-kit-json-adapter`](crates/lib/infrastructure/json-adapter/README.md) | Review, run and journal documents and command output |
| [`review-kit-filesystem-adapter`](crates/lib/infrastructure/filesystem-adapter/README.md) | Review-run storage, local files and the publication journal |
| [`review-kit-json2md-adapter`](crates/lib/infrastructure/json2md-adapter/README.md) | Comment rendering with embedded or external schemas and templates |
| [`review-kit-cli`](crates/bin/cli/README.md) | The `rkc` executable |

The [architecture](docs/architecture.md) describes how these crates fit together.

## Documentation

- [CLI reference](crates/bin/cli/README.md): configuration, commands, JSON fields and exit codes.
- [Feature contract](docs/features.md): supported behavior and limits.
- [Review guide](docs/guide.md): how the tool handles findings, replies and review history.
- [Documentation index](docs/README.md): further references and implementation status.

## Workspace checks

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

The workspace test run excludes the CLI crash-point tests. Their test build
enables the `fault-injection` feature, which aborts `rkc` at a named crash point;
each test then runs `rkc` again from the stored state:

```sh
cargo test -p review-kit-cli --features fault-injection --test faults --locked
```

The [GitLab adapter tests](tests/gitlab/README.md) and
[CLI tests](tests/cli/README.md#repeatable-cli-suite) use disposable merge requests
on a local GitLab server. They are ignored by the ordinary workspace test run;
their guides give the commands to run them explicitly.

## License

Review Kit is available under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
