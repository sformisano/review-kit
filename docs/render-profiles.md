# Rendering profiles

A profile contains JSON Schemas and Jinja templates for five artifacts:
findings, summaries, transition replies, answer replies and suggestion blocks.
Installed `rkc` binaries load these files directly. Editing a profile needs no
Rust build, source checkout or separate json2md executable.

The [binary package](distribution.md) includes external profiles.

## Export and edit

Export the defaults into a new directory:

```sh
rkc render-profile export --dir "$HOME/.review-kit/review-style"
```

This command needs no configuration, credentials or network access.
It creates `profile.json` and ten asset files. Existing target files prevent export.

The [default presentation](../crates/lib/infrastructure/json2md-adapter/README.md#default-presentation-and-safety)
leaves authored evidence out of finding comments. For example, change
`finding.j2` to show it:

```jinja
**F{{ id }}: {{ title_markdown }}**

{{ body }}

**Evidence:**

{{ evidence }}
{% if suggestion is not none %}
{{ suggestion.markdown }}{% endif %}
*{{ severity }} · {{ policy.name_markdown }}*
```

The [compact example](../examples/render-profiles/compact/) contains a complete
profile with all five roles. Copy the whole directory to use it independently.
Its schemas accompany its templates.

The [manifest and input reference](../crates/lib/infrastructure/json2md-adapter/README.md)
defines every field, structured action, historical row and suggestion value.
Schemas validate rendering inputs; they cannot weaken review or provider checks.

### Add policy results to a v1 profile

A profile that declares `review-kit-render-input/v1` renders without policy results.
Follow the [v2 steps](../crates/lib/infrastructure/json2md-adapter/README.md#input-contract-versions),
then read `policy.result` in `summary.j2`:

```jinja
{% for policy in policies %}- {{ policy.id_markdown }}: {% if policy.result is none %}not evaluated{% else %}{{ policy.result.kind }}{% endif %}
{% endfor %}
```

## Select the profile

Add these optional fields to your [configuration](../crates/bin/cli/README.md#trusted-configuration):

```toml
review_workflow = "team-review"
render_profile = "review-style/profile.json"
```

Place these top-level fields before the first table header in the home configuration.
The workflow name is public metadata. Supply your team's name explicitly.
The agent supplies the policy-set display name, policy IDs and names in the
[assessment](../crates/bin/cli/README.md#assessment-editing). Checking and publication read them from the accepted assessment.

Profile selection follows this order:

1. `--render-profile PATH`, relative to the invocation directory.
2. Configuration `render_profile`, relative to the configuration file.
3. Embedded defaults.

Schema and template paths are relative to the manifest.
An invalid selected profile fails with an asset-specific diagnostic.
The loader reads local files only and runs no hooks.

## Prepare and check a review

From a Git worktree, start a review run. Complete its single assessment draft before acceptance:

```sh
rkc run start --mr "$MR_URL"
```

The [review-run reference](../crates/bin/cli/README.md#review-runs) defines the draft.
Fill the authored judgments, findings, evidence, scope and depth in
`review-assessment.draft.json`, following the [assessment reference](../crates/bin/cli/README.md#assessment-editing).

Check and render with the configured profile:

```sh
rkc check
```

Without a configured profile, the same command uses embedded defaults.
To override either choice:

```sh
rkc --render-profile "$HOME/.review-kit/review-style/profile.json" \
  check
```

Inspect the command result and the run's `rendered/review-summary.md` and
`rendered/terminal-review-summary.md`. The result's `render_profile` contains the
profile ID, input contract and computed fingerprint.
Local paths and `local_context` are absent from public rendering inputs.

## Preview and publish

Accept the assessment, then preview against current GitLab state:

```sh
rkc accept
rkc publish --effects review --plan
```

Publish within configured authority:

```sh
rkc publish --effects review
```

If you selected a CLI override during checking, supply the same override here.
All commands load the same profile implementation.
Publication submits the rendered Markdown with tool-owned markers and state attached.
It records exact bodies and profile identity in execution evidence.

Successful assessment acceptance and planning retain the profile identity in the
run's `review-render-profile.json`. Publication with another profile identity,
or of a run that records none, fails with `render_profile_changed` before any
provider request.
After editing assets, run `rkc publish --effects review --plan` again before publication.
Planned actions remain separate from verified actions in summary tables.
Finding links appear only after their roots are verified.

## Review a later commit

Start a new run for the selected run's merge request:

```sh
rkc run start
```

Declare all selected policies in the assessment, including the policy of every
finding that is still open. Fill the policy outcomes, reassessments and required
transition explanations, then check, accept, preview and publish the assessment
with the same commands.
Each earlier round retains its verified findings table.
Fixing F1 changes the latest row to Fixed while its Round 1 row remains Open.
A recurrence keeps F1 and its original discussion.

Coverage includes every declared policy, including excluded and unresolved policies without findings.
Original declarations and trusted names survive later exclusion or removal.
Replies and retries do not add finding roots.

## Recover an interrupted attempt

Keep the run directory, profile assets and configured publication state.
Select the run that owns the unfinished publication and invoke `publish` again with the original arguments.
The tool verifies observed effects before deciding whether publication can continue.

A changed profile cannot replace unresolved action bodies.
Restore the exact original manifest and assets if recovery reports a profile mismatch.
Rechecking changed assets does not override that restriction.
Uncertain writes retain the existing [recovery limits](../crates/bin/cli/README.md#uncertain-writes).

An unchanged completed assessment with identical public declarations produces no writes and no new round.
A declaration change starts a successor under the [declaration contract](../crates/lib/application/README.md#public-policy-declarations).
A presentation-only change also leaves completed comments unchanged.
Changing a known workflow or policy-set name while repeating a completed assessment
returns `review_identity_changed`. Restore the original names for that retry;
a changed assessment can establish new names in its next round.
Commands sharing publication state use a local review lock under the
[CLI recovery contract](../crates/bin/cli/README.md#publication-and-recovery).

## Retained-state compatibility

Summaries read and write version 3 provider state with lossless shared tables,
policy names, public metadata, profile identity and verified historical rows.
The [retained-provider-state contract](../crates/lib/infrastructure/json-adapter/README.md#retained-provider-state)
defines the format and restoration bounds. Provider markers remain version 1.
Absent historical rows display as unavailable. The tool does not infer them
from current finding states.
Unfinished state without a profile identity cannot safely resume with an identified renderer.
No import of other tools' review markers is provided.
The [package identity](distribution.md#build-identity) identifies the executable used for a review.
Keep using the same executable and profile files throughout an unfinished attempt.

| State | Writer | Readers |
| --- | --- | --- |
| Provider summary state | Version 3 with `confirmed_validation` rounds | Version 3, with or without that field |
| Review runs: workspace, run manifest and captured context | Version 1 | Version 1 |
| Captured conversation, `review-conversation.json` | Version 2 | Versions 1 and 2; version 1 notes have no author account |
| Run assessment, `review-assessment.draft.json` | Version 4 | Version 4 |
| Run publication journal, `publication-progress.json` | Version 3 | Version 3 |

Readers reject unknown fields.
Run commands keep the profile identity in `review-render-profile.json`.
The journal preserves the run binding, dispatch phase, original authority, consumed retries and settlements.
Current v2 summary input includes each policy's full public declaration history; coordinated custom schemas and templates must follow the [renderer contract](../crates/lib/infrastructure/json2md-adapter/README.md#summary).
The [recovery reference](../crates/bin/cli/README.md#publication-and-recovery) defines recovery limits.
