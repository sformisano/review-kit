# json2md renderer adapter

`Json2MdRenderer` implements the application's `ReviewRenderer` port through json2md.
Both embedded defaults and caller-owned profiles compile JSON Schemas and standalone MiniJinja templates through the same library path.
The distributed `rkc` binary contains that library and its embedded assets.
Profiles require no source checkout, Rust compilation, separate executable, network access or executable hooks.

This document owns the profile manifest and rendering-input contracts.
The [CLI guide](../../../bin/cli/README.md) owns profile selection, relative configuration paths, checking, publication and recovery commands.
The [application contract](../../application/README.md#rendering-and-summary-projection) owns verified counts, actions and retained historical snapshots.

## Load or export a profile

`Json2MdRenderer::new()` loads the embedded profile.
`Json2MdRenderer::from_profile(&Path)` loads an explicitly selected manifest.
Invalid selected profiles return errors; loading does not fall back to embedded assets.

`Json2MdRenderer::export_default(&Path)` exports the manifest, schemas and templates.
It returns the exported `profile.json` path.
The corresponding offline CLI command is:

```sh
rkc render-profile export --dir ./render-profile
```

Export requires no GitLab credentials or configuration.
Existing target files, including symbolic links, prevent export before any profile file is created.
An existing directory can contain unrelated files.
Exclusive file creation also protects targets that appear during export.

The exported profile works immediately in a worktree with a selected review run:

```sh
rkc --render-profile ./render-profile/profile.json check
```

Edit the exported templates or schemas, then check and preview again.
The [CLI workflow](../../../bin/cli/README.md#review-runs) covers review runs, publication, follow-up and interrupted recovery.

## Manifest contract

The manifest uses `review-kit-render-profile/v1`:

```json
{
  "version": "review-kit-render-profile/v1",
  "id": "team-review",
  "input_contract": "review-kit-render-input/v2",
  "artifacts": {
    "finding": {
      "schema": "finding.schema.json",
      "template": "finding.j2"
    },
    "summary": {
      "schema": "summary.schema.json",
      "template": "summary.j2"
    },
    "transition": {
      "schema": "transition.schema.json",
      "template": "transition.j2"
    },
    "answer": {
      "schema": "answer.schema.json",
      "template": "answer.j2"
    },
    "suggestion": {
      "schema": "suggestion.schema.json",
      "template": "suggestion.j2"
    }
  }
}
```

All five artifact roles are required.
Each role owns its schema and template.
`input_contract` selects one of the [supported input contracts](#input-contract-versions).
Unknown manifest properties, unsupported versions and unsupported input contracts fail loading.
The public `id` contains 1-128 ASCII letters, digits, dots, underscores or hyphens.
`.` and `..` are invalid IDs.
Paths, credentials and private context do not belong in the public ID.
Asset paths must be nonempty and relative to the manifest directory.

Schemas can use internal references and embedded definitions.
External schema retrieval is disabled.
Templates are standalone; includes, imports and inheritance cannot load external templates.
MiniJinja provides its built-in filters and tests, strict undefined values and no automatic escaping.
The bundled json2md library supports expressions, conditionals and loops; template macros are unavailable.
Schema defaults do not fill absent input properties.

Construction loads every asset before compiling any rendering view.
It rereads the manifest and assets to reject changes during loading.
Compiled views retain their owned bytes for the invocation.
Later file edits affect the next invocation.

`identity()` returns application-owned `RenderProfileIdentity`:

```json
{
  "id": "team-review",
  "input_contract": "review-kit-render-input/v2",
  "fingerprint": "sha256:<hex digest>"
}
```

The fingerprint hashes the canonical manifest and length-delimited asset bytes in the fixed role order shown above.
Manifest whitespace and JSON property ordering do not change it.
Changing an asset, manifest ID, input contract or declared asset path changes it.
An unedited exported profile has the embedded profile's identity.
Public identity contains no local asset paths.
The [recovery contract](../../application/README.md#publication-and-recovery) defines how retained identity protects exact pending write intent.

## Rendering-input contract

Each input is a JSON object whose `version` is the profile's declared input contract.
Nested finding and suggestion objects carry the same `version`.
The schemas in [assets](assets/) give complete field types and required properties.
Caller-owned schemas validate the supplied rendering data; they do not replace assessment validation or typed core rules.

Plain labels have separate raw and Markdown-safe properties.
For example, `title` preserves the authored title, while `title_markdown` escapes Markdown table and link syntax.
`name_markdown`, `scope_markdown`, `depth_markdown` and revision `*_markdown` fields follow the same rule.
Authored finding explanations, evidence, answers and transition explanations remain Markdown.
They are not rewritten or duplicated by the projection.
`local_context`, credentials, transcripts and private execution evidence are absent from every input.

### Input contract versions

| Contract | Inputs |
| --- | --- |
| `review-kit-render-input/v2` | The fields described below, including summary policy `result` and `declarations` |
| `review-kit-render-input/v1` | The same fields without summary policy `result` or `declarations` |

The embedded profile and `rkc render-profile export` use v2.
Profiles that declare v1 load and receive v1 inputs.
Each schema's `version` constant must match the declared contract.
A closed v1 summary schema also rejects the v2 `result` property.

To move a custom v1 profile to v2:

1. Set `input_contract` in `profile.json` to `review-kit-render-input/v2`.
2. Change every `version` constant in the profile's schemas to `review-kit-render-input/v2`.
3. Add required `result` and `declarations` properties to the summary schema's policy items, as in the [default summary schema](assets/summary.schema.json).
4. Run `rkc check` and review the rendered previews.

A v2 summary schema must accept the full declaration array under each summary policy;
a closed schema without `declarations` rejects the input before publication.
Start a custom summary schema and template from the [default summary assets](assets/summary.schema.json), then run `rkc check` and inspect the preview.
A permissive custom template can choose a shorter presentation; it does not automatically show the default policy tables.

Switching changes the profile identity.
An unfinished round still requires its original profile under the [recovery contract](../../application/README.md#publication-and-recovery).

### Finding

The finding role receives:

| Field | Meaning |
| --- | --- |
| `id` | Stable positive finding number; templates can prefix it with `F` |
| `title`, `title_markdown` | Authored title and safe label |
| `body`, `evidence` | Separate authored explanation and evidence |
| `severity` | `P1`, `P2`, `P3`, `P4` or `P5` |
| `disposition` | `Blocking` or `Advisory` |
| `status` | `Open`, `Fixed`, `Withdrawn` or `Uncertain` |
| `policy` | Stable `id`, trusted display `name` and `name_markdown` |
| `location` | `path`, `path_markdown` and positive anchor `line` |
| `context` | Public round metadata described below |
| `suggestion` | `null`, or structured suggestion data plus rendered `markdown` |

`finding_context(FindingRenderInput)` supplies the trusted policy name and public context.
The `finding(&Finding)` method returns a missing-context diagnostic.
Display names come from the agent-authored assessment, independently of stable IDs.
The renderer does not load policy definitions or resolve skill URLs.

### Public context

Finding, transition and answer inputs expose `context` with:

- `round`: positive attempted round number.
- `revision`: exact `head` and ordered `comparison` references, plus their Markdown-safe variants.
- `publication_status`: `Pending`, `Partial`, `Completed`, `Blocked`, `Superseded` or `Abandoned`.
- `review_workflow`: caller-supplied public workflow identity, or `null` when omitted or unavailable in retained metadata.
- `policy_set`: explicitly selected named set.
- `scope` and `depth`: separate authored fields.
- Markdown-safe variants of workflow, policy set, scope and depth.

This context establishes presentation data, not publication authority.
It contains no private invocation paths.

### Summary

The summary role handles both pending and terminal presentation.
It receives `round`, `history`, `findings`, `counts`, `policies` and `unpublished`.

`round` contains `number`, `revision`, `decision`, `status`, separate `scope` and `depth`, workflow identity and policy-set identity.
It also contains review/publication limitations and an optional abandonment reason.
Raw labels have Markdown-safe counterparts.
`revision_kind` is `Reviewed` when publication finished, with status `Completed` or `Blocked`; other statuses use `Attempted`.
The review decision remains independent of publication status.

`findings` contains the latest verified communicated rows.
Each row exposes finding data, verified `url`, `root_note`, `thread` and structured `actions`.
The root note supplies the finding link; the thread supplies the reply target.
Actions use `{kind, label, from, to, in_reply_to}`.
Their kinds are `created`, `transition`, `answered`, `resolved` and `reopened`.
They describe verified effects for the displayed round.
A `transition` whose `from` equals `to` is a same-state reclassification labeled `Updated`.
Resolution kinds identify the verified state. Their `label` distinguishes
acknowledged writes from recovered observations: `Observed resolved` or `Observed reopened`.
Use `label` when displaying action claims; an observation does not establish which request caused the state.
Planned or uncertain effects do not become completed action labels.

`history` contains previous rounds in descending round order.
Each retained round has its own metadata and `history_available` flag.
When available, its `findings` array holds that round's immutable verified rows.
Historical finding names use that round's recorded name, or the latest earlier name when its assessment excludes the policy.
Later definitions do not rename earlier finding rows.
When unavailable, `findings` is `null`.
The embedded template names this historical gap instead of reconstructing rows from current finding states.

`counts` contains verified `total`, `open`, `fixed`, `withdrawn` and `uncertain` counts.
`policies` includes every current or historical public declaration and policies of verified owned finding roots. Newly excluded policies retain public rows. `rounds` counts only Applicable declarations; unresolved applicability does not count as applied.
Each policy exposes stable `id`, trusted `name`, Markdown-safe labels, numeric `rounds` and distinct verified owned-root `comments`.
Under v2, each policy also exposes `result`, the agent's judgment in the latest round.
`result` is `null` when the latest assessment has no evaluation for the policy.
Otherwise it is `{kind, limitation, limitation_markdown}`, with `kind` set to `Satisfied`, `Violated` or `Blocked`.
Only `Blocked` carries its limitation text and Markdown-safe variant; the other kinds have `null` limitations.
Each v2 policy also exposes `declarations`, in retained chronological order.
A declaration has `round`, its original `name` and `name_markdown`, `status` (`Applicable`, `Excluded` or `Unresolved`), exact `reason` and `reason_markdown`, and `result` derived from that round's judgment.
An excluded declaration has a null result. Aggregate names prefer the current declaration, otherwise the latest retained declaration.
Markdown escaping preserves public text; it does not redact private prose.
Violation evidence stays in the assessment.
An unused policy has `rounds: []` and `comments: 0`.
Later exclusion preserves earlier applicability and root counts.
Unavailable retained names are `null`.
Replies, retries, resolutions and foreign roots do not increase these counts.
`unpublished` contains IDs of findings without verified roots that the round may post; a blocked review omits the findings it withholds. No links are fabricated.

### Transition

`transition_context(TransitionRenderInput)` receives separate `finding`, `previous_status`, `new_status`, `explanation` and `context` properties.
`finding` has the finding-role shape.
Templates arrange these values directly.
The application checks status changes against verified communication before rendering.
The `transition` method returns a missing-context diagnostic.

### Answer

`answer_context(AnswerRenderInput)` receives `id`, `title`, `title_markdown`, authored `body`, `in_reply_to`, `root_note`, `thread`, `question`, `finding` and `context`.
`question` contains the verified target `id`, `thread`, API-confirmed `author` and observed `body`.
`finding` describes the latest verified communicated finding and its trusted policy name.
Its `suggestion` is `null`; answer bodies cannot publish suggestion blocks.
The `answer` method returns a missing-context diagnostic.

### GitLab suggestion

The suggestion role renders before the enclosing finding.
Its result appears in `suggestion.markdown` alongside its structured data.
Summary rows expose suggestion data without rendering another suggestion block.

| Field | Meaning |
| --- | --- |
| `id` | Stable finding number |
| `path`, `path_markdown` | Source path and safe label |
| `anchor_line`, `start_line`, `end_line` | Inclusive committed source range |
| `original` | Exact original line array |
| `replacement` | Exact replacement line array; empty means deletion |
| `replacement_text` | Replacement lines joined with LF |
| `replacement_content` | Joined replacement plus one final LF for a nonempty line array; empty for deletion |
| `fence` | Backtick fence longer than every backtick run in the replacement, with a minimum length of three |
| `info` | `suggestion` or `suggestion:-<above>+<below>` |
| `lines_above`, `lines_below` | Nonnegative offsets from the anchor |
| `revision` | Exact committed revision references |
| `evidence` | Authored correction evidence |

Use the supplied formatting values to preserve multiline replacement and deletion:

```jinja
{{ fence }}{{ info }}
{{ replacement_content }}{{ fence }}
```

The provider adapter parses the final finding body independently.
It rejects a missing, extra or changed suggestion block.
It also verifies the exact replacement, range, committed source and diff position.
The GitLab adapter [rejects nonempty whitespace-only replacements](../gitlab-adapter/README.md#source-and-suggestion-support);
use an empty replacement array for deletion.
A weaker custom schema cannot bypass those checks.

## Default presentation and safety

Embedded finding bodies show a bold `F<id>: <title>` heading, the authored explanation and an optional verified suggestion.
Their footer names the severity and the policy display name, such as `*P2 · Rust Error Contracts*`.
Authored evidence, disposition and policy IDs stay out of finding bodies; the summary's findings tables show disposition.
The default adds no AI attribution.

Embedded transition replies open with the outcome in bold, followed by the authored explanation.
A multi-line explanation starts in its own paragraph so its lists and code blocks keep their formatting.
The outcome is `Fixed.`, `Withdrawn.` or `Reopened.`; an uncertain finding that becomes open is `Confirmed.`.
A same-state reclassification keeps the status and opens with `Updated.` instead.
Other changes use the new status.

Embedded summaries give every round a section with the same parts: its number, its decision and, when it has findings, its findings table.
The current round's section is visible.
It shows the review decision as a heading, then a recap line with the round number and each nonzero finding state.
When no finding has a verified root and none is waiting to post, the recap ends in `no findings`.
The overall limitation stays visible.
Pending, partial, superseded and abandoned publication add a status line.
The latest findings table shows each finding's severity and disposition, such as `P2 · Blocking`, its status and its verified actions in this round; a same-state reclassification shows `Updated`.
A resolution the tool observed instead of performing reads `resolved (observed)` or `reopened (observed)`.
The `unpublished` IDs follow under `Not posted yet:`.

Each earlier round follows in its own collapsed section, oldest first.
Its label names its number and decision, then its publication status when that is pending, partial, superseded or abandoned, such as `Round 1 · Changes required · abandoned`.
The section holds only that round's immutable findings table, whose last column shows its actions in that round.
An earlier round that posted no findings says `No findings were posted.`
One without a retained table says `Findings for this round were not recorded.`
Historical input carries no finding counts, so an earlier round has no recap line.

One collapsed `Review details` section closes the summary.
It holds the supporting details of every round, each under a `Round <number>` heading.
The current round comes first, then the earlier rounds in the order of their sections.
Two rounds give `Round 2`, `Round 1`; three give `Round 3`, `Round 1`, `Round 2`.
No section nests another, and `Review details` repeats no findings table.
Blank lines separate each heading, list and table from its neighbors and from the section tags, so GitLab renders the Markdown inside a collapsed section.

Under its heading, each round lists its commit, scope, depth, workflow, publication limitation and abandonment reason.
An earlier round also lists its limitation there; the current round's limitation stays visible beside its decision.
Each round then lists each policy once, in one table per applicability group with its count:

| Group | Columns |
| --- | --- |
| Applicable policies | Policy, latest result, open findings, reason |
| Policies with unresolved applicability | Policy, latest result, open findings, reason |
| Excluded policies | Policy and reason; open findings only while an excluded policy still has one |
| Policies not declared in this round | Policy and its earlier applicable rounds; a missing display name shows the policy ID |

These are the current round's columns.
An earlier round has the first three groups, with that round's names, results and reasons and without open findings.
Empty groups are omitted. A round without policies says `No policies were declared.`
Results read `Pass`, `Fail` or `Blocked`, or `—` when that round did not assess the policy.
A blocked result carries its limitation in the same cell, as `Blocked: <limitation>`.
It reads `Blocked` alone when the limitation equals the policy's reason, and `Blocked: see the limitation above` when it equals that round's overall limitation, so each text appears once.
That overall limitation appears above the reference: beside the decision for the current round, and under its own heading for an earlier round.
The template compares exact text; it never shortens or rewrites authored text.
Table labels and reasons escape pipes, newlines and HTML; mention syntax is interrupted with a zero-width space.
The [CLI guide](../../../bin/cli/README.md#public-summary-text) shows how to write concise scope, depth, limitation and reason text.

Custom profiles can change headings, ordering, labels, tables and surrounding prose for every supported role.
The application and GitLab adapter retain ownership, authority, revision, identity, positioning and readback rules.
Rendered markers and unsafe quick actions are rejected independently of schemas.
The tool attaches discovery markers and retained machine state outside rendered Markdown.
The caller serializes publication for the same subject and reviewer; profile loading introduces no writer lock.

## Diagnostics and verification

Loading errors name the invalid manifest, asset role or local asset path.
Rendering errors include the role, schema/template paths, failed input/schema JSON Pointers and underlying template details.
These diagnostics are local; public publication limitations contain safe tool-owned categories.

Inside a shared [observation scope](../../observation/README.md), the renderer records profile loading, compilation, export and each rendering role.
Schema and template execution has a child record, including suggestion rendering within a finding or transition.
Logs disclose embedded or file selection, artifact roles and byte counts.
Paths, profile contents, rendering inputs and rendered bodies remain masked or absent.
Error records disclose typed renderer categories and mark dependency causes as opaque, without formatting local diagnostic strings.

Run `cargo test -p review-kit-json2md-adapter` for renderer and profile regression tests.
They cover default bodies, where each round's findings and supporting details appear, a complete two-round summary, input contract versions, historical rows, export equivalence, custom schemas/templates, missing data, invalid assets and suggestion formatting.
Observation tests inspect actual JSON, parent execution IDs and sensitive-value exclusion on success and diagnostic paths.
The [CLI tests](../../../bin/cli/tests/) cover actual command selection and HTTP publication wiring.
Live GitLab behavior requires the separately authorized [GitLab suite](../../../../tests/gitlab/README.md).
