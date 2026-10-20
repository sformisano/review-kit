# review-kit-gitlab-adapter

This crate implements the application's GitLab ports with a blocking REST v4
client. It authenticates the actor, binds one merge request, reads committed
source and diffs, and publishes individual effects. The application retains
review validation, ordering, reconciliation, history, and publication results.

## Configuration and ports

The host supplies a credential, deployment URL, merge-request URL, and explicit
effect authority. The crate
does not read environment variables, discover credentials, or establish human
approval. `Credential::private_token` sends `PRIVATE-TOKEN`;
`Credential::bearer_token` sends `Authorization: Bearer`. Authentication uses
`GET /user`; the returned numeric user ID establishes the reviewer identity.

```rust,no_run
use review_kit_application::publication::Effects;
use review_kit_application::use_cases::publishing::PublicationProgress;
use review_kit_gitlab_adapter::{Credential, GitLab, GitLabConfig};

fn connect(token: &str) -> Result<(), review_kit_gitlab_adapter::GitLabError> {
    let config = GitLabConfig::new(
        "https://gitlab.example/deployment/",
        Credential::private_token(token)?,
    )?;
    let gitlab = GitLab::connect(config,
        "https://gitlab.example/deployment/group/sub/repo/-/merge_requests/7")?;
    let host = gitlab.publication_host(Effects::review());
    let progress = PublicationProgress::default();
    let _ = (gitlab, host, progress);
    Ok(())
}
```

The deployment URL excludes `/api/v4` and permits HTTPS. Numeric loopback HTTP
is available for fixtures. URLs containing credentials, queries, or fragments
are rejected. Redirects and automatic HTTP retries are disabled; proxy
environment variables are ignored. TLS verification remains enabled.
Credential and configuration `Debug` output excludes authentication data.
Configuration debug output includes the server URL. Retained source uses the
standard `Debug` derive. Errors report typed outcomes without raw response
bodies. The host protects any additional logs it produces.

| Application port | Implementation | Provider boundary |
| --- | --- | --- |
| `ReviewReader` | `GitLab` and publication session | MR, notes, discussions, direct readbacks, API authors, retained summary |
| `ReviewChanges` | `GitLab` | Exact diff version, changed files and unified diff |
| `CommittedSource` | `GitLab` | Source-project tree and raw blobs at immutable head SHA; exact positions |
| `CommittedSource` | `RetainedSource` | Locally retained committed text and complete diff evidence; no network or worktree reads |
| `PublicationHost` | `GitLabHost` | Actor binding and explicit authority |
| `PublicationSession` | Acquired session | Summary POST/PUT, positioned discussion POST, reply POST, resolution/reopening PUT and readbacks |

Notes, discussions, tree entries and diff versions are fully paginated before a
result is returned. Missing pages, conflicting records, invalid references, and
revision drift fail the operation. Nested project paths are encoded as one API
segment. Publication targets the target project; source reads use the fork's
source project. MR identity and revision references are corroborated on refresh.

The snapshot effect set is a supported-operation upper bound, not proof of token
permissions. An unlocked or nullable lock flag allows attempted supported
effects; a locked discussion narrows the set to none. Host authority can only
limit it further. Actual server rejection remains authoritative, including
resolution or reopening denial after earlier effects succeeded.

Rendered bodies cannot invoke GitLab quick actions such as `/close` or
`/approve`. Slash-led lines must be fenced or indented code; the adapter checks after
removing CR bytes, as GitLab does. Fenced suggestions, other fenced or indented
code retain literal slash lines. Slash-led lines in multiline inline spans are
unsupported because GitLab extracts commands with different inline delimiters.
This restriction applies to summary, finding and reply bodies, without a command
allowlist.
[GitLab quick-action extraction](https://gitlab.com/gitlab-org/gitlab/-/blob/master/lib/gitlab/quick_actions/extractor.rb).

`validate_plan_bodies(&Plan)` applies these provider body constraints without
network or filesystem access. It checks both summary bodies, finding suggestions,
reply bodies and retained round actions, including history and abandonment
receipts. Offline checking and publication previews call it after core planning
so unsupported bodies fail before provider serialization. It does not establish
current revisions, ownership or publication authority.

Rendered bodies must use LF line endings and contain no carriage returns.
GitLab removes carriage returns before storing notes, which would prevent exact
readback of the retained body. This restriction includes finding and reply bodies
embedded in a pending summary. Committed source can retain CRLF line endings.

Default limits are 30 seconds per request, 10 seconds to connect, 8 MiB per
response, 32 MiB per paginated collection, 100 pages, and 1 MiB per serialized
write. `GitLabConfig::with_limits` accepts nonzero bounded alternatives. No
truncation or body normalization makes an oversized or changed record pass.

## Discovery markers and retained state

Owned comments end with two LF bytes and one exact marker, with no trailing
bytes. The separator is removed when reconstructing the rendered body; its
original bytes, including trailing whitespace, are preserved. JSON field order
is canonical when writing; parsing rejects unknown fields and unsupported
versions. Finding and transition identifiers are positive integers; answer
targets are canonical positive decimal GitLab note IDs represented as strings.

```text
<!-- review-kit:v1 {"kind":"summary"} -->
<!-- review-kit:v1 {"kind":"finding","finding":3,"round":2} -->
<!-- review-kit:v1 {"kind":"transition","finding":3,"round":4} -->
<!-- review-kit:v1 {"kind":"answer","finding":3,"round":4,"note":"91"} -->
```

Markers belong outside suggestion fences. Their meaning is scoped to the
bound MR and authenticated API author. Foreign markers are ordinary foreign
content. Malformed, duplicate, unsupported, or conflicting owned records stop
discovery. API scope, author, exact body, position, suggestion content, and
retained intent corroborate owned effects; marker text alone proves nothing.
Reusable confirmations trust the hidden state in summary notes the reviewer account wrote;
GitLab lets Maintainers edit other users' comments, so this trust extends to them.
A contradiction confined to one finding fails with `GitLabError::FindingHistory`,
which names the finding and a `FindingConflict`: `MissingRoot` (an owned reply
outside a thread rooted by that finding, such as after its root note was
deleted), `DuplicateRoot` (a second root for the finding) or `Inconsistent`
(an owned finding or reply record that disagrees with the retained summary
history). Invalid retained summary data stays `GitLabError::State`.
Discovery stops at the second root of a finding, so `GitLab::finding_roots`
lists every owned root note that claims it, with GitLab's `created_at` text, to
diagnose the duplicate. It reads notes and discussions only and deletes nothing.

The owned summary has one hidden state comment immediately before its summary marker:

```text
<!-- review-kit:state:v3 {"version":3,"subject":<subject>,"reviewer":<reviewer>,"findings":[<finding>,...],"policy_names":[<name>,...],"current":<round>,"history":[<round>,...]} -->

<!-- review-kit:v1 {"kind":"summary"} -->
```

Angle-bracket values above describe the [provider DTO](../json-adapter/src/rounds/state.rs),
not literal JSON. [Wire tests](src/wire/tests.rs) specify complete records and
invalid cases. Each round retains its complete assessment, status, abandonment
receipt with verified actions, publication limitation, answers, transitions,
exact answer targets, full public policy declarations, intended actions with rendered bodies,
and whether acceptance validated its blocking findings only from recorded confirmations. Declaration names use the shared name table; each round retains its original applicability and reason under the [provider DTO contract](../json-adapter/README.md#retained-provider-state). Answer targets
retain note and discussion references; finding-root references come from scoped
marker rediscovery. Domain construction validates decoded values.
JSON escapes HTML delimiters, backticks and tildes so retained text cannot close
the single-line state comment. The parser rejects missing, duplicate, reversed,
unpaired and unsupported state records. Authored bodies cannot emit reserved
markers or the data-panel title. Escaped state payloads are bounded to 8 MiB
before dispatch and decoding.
The state remains public in raw note bodies and API responses. Callers must exclude
private context from public assessments and rendered bodies. The adapter adds
no credentials, local paths, raw prompts, or execution evidence.

The retained-state codec reads and writes version 3 only.
Its [format contract](../json-adapter/README.md#retained-provider-state) defines
lossless shared tables, deterministic reference ordering and bounded restoration.
Unsupported versions return `GitLabError::UnsupportedStateVersion` before the
record shape is interpreted. CLI discovery reports
`unsupported_retained_state_version`, the observed version and supported version
without writing or replacing provider records.
Discovery markers use the v1 format. Rendering and CLI serialization have
separate contracts.

## Source and suggestion support

The implementation selects the exact diff version matching head, base and
start SHAs. It requires a collected version with complete file counts or an
empty version with a zero file count and no diffs. Every diff file requires
explicit `collapsed` and `too_large` flags. Responses without those flags are
unsupported. The response-shape requirement does not establish compatibility
with every server version.
[GitLab diff-version API](https://docs.gitlab.com/api/merge_requests/#retrieve-a-merge-request-diff-version).

Supported anchors are new-side added and context lines in regular UTF-8 text
files, including renamed files. Context positions retain both coordinates.
Suggestion ranges lie within one hunk and use GitLab line codes and offsets up
to 100 lines above or below the anchor. Single-line, multiple-line, and deletion
suggestions are supported. Original source, range, rendered fenced syntax and
provider readback must agree exactly.
[GitLab discussions API](https://docs.gitlab.com/api/discussions/).

Suggestion readback uses raw committed ranges with their terminal line endings.
Replacement endings follow GitLab's first CR/LF sequence in the final raw line,
including CRLF, lone CR, unterminated lines, and empty deletions.
[GitLab suggestion implementation](https://gitlab.com/gitlab-org/gitlab/-/blob/master/lib/gitlab/diff/suggestion.rb).
Replacement code can contain backticks when the renderer uses a suitable fence.
Missing, extra or mismatched suggestion blocks fail validation; nested blocks
are checked too. Planned finding and reply bodies are validated before the
pending summary is written. Raw HTML code blocks and whitespace-only nonempty
replacements are unsupported.
Summary and reply bodies permit no active suggestion blocks. Owned marked
reply readback also rejects unexpected provider suggestions; foreign replies
remain ordinary observed content.

Deletion-only anchors, binary/control-byte source, invalid UTF-8, symlinks,
submodules, unavailable or truncated diffs, cross-hunk ranges, and positions
outside the diff are explicit errors. An invalid inline effect never becomes a
general comment. Historical core findings retain their creation revision,
location and suggestion. GitLab can move a surviving note's API position after
a push. The adapter validates the original intent against its historical diff,
then separately checks the tracked position against its named complete diff
and its anchor coordinates. If the tracked anchor or suggestion source changes,
discovery classifies that finding's position as outdated. The finding keeps its
retained version, creation revision, location, suggestion and thread identity.
GitLab's suggestion metadata can describe changed content or a mixed old/new
range; it does not replace the retained suggestion. Exact root authorship,
marker and body checks still apply. Missing or duplicate roots remain conflicts.

For unchanged source, suggestion API line numbers follow the tracked range;
retained raw suggestion contents remain exact. A multiline API range must equal
the original validated range or the current validated range. Traced context
endpoints permit GitLab's null line type with exact coordinates and line codes.
Unavailable named revisions and incompatible anchor coordinates still stop discovery.
`discover_with_outdated_positions` returns the verified snapshot and affected finding IDs.
The CLI exposes those IDs through its [capture output and draft context](../../../bin/cli/README.md#review-runs).

Outdated positions grant no authority for position-dependent writes.
New inline discussions still require exact current source and diff validation.
Existing threads use their verified discussion IDs for replies and resolution;
the summary retains their original finding links.
[GitLab position tracking](https://gitlab.com/gitlab-org/gitlab/-/blob/master/app/services/discussions/update_diff_position_service.rb).
[GitLab range tracing](https://gitlab.com/gitlab-org/gitlab/-/blob/master/lib/gitlab/diff/position_tracer/line_strategy.rb).

### Retained source for offline checking

`GitLab::capture_source(subject, revision)` reads the complete named diff and
changed head-side committed blobs. It checks the current comparison before and
after capture. Deleted files and unsupported binary or nonregular blobs remain
explicit entries; transport and authentication failures fail capture.
The collection respects the configured aggregate byte limit.

`RetainedSource::encode` and `RetainedSource::decode` exchange an opaque v1 JSON
envelope. Decoding validates revision references, complete diff counts, changed
paths, file roles and committed text. The envelope preserves Unicode, carriage
returns, terminal line endings and empty files. The caller stores these local
bytes beside the assessment and retained discovery snapshot.

`RetainedSource` implements `CommittedSource` without a credential, HTTP client
or filesystem fallback. Calls require the exact subject and ordered revision
references. Position and suggestion checks use the same diff parser and position
builder as live GitLab source checks. Unsupported, deleted or absent paths return
source errors. Offline checks establish consistency with retained evidence;
publication still requires fresh provider discovery and committed source.

The retained-source tests in [support/source.rs](tests/support/source.rs) exercise
capture through the HTTP adapter, decoding and offline checks after the fixture
server stops. Run the source tests with:

```sh
cargo test -p review-kit-gitlab-adapter --test http_workflow source::
```

## Conversation capture

`GitLab::capture_conversation(subject, lookup)` returns the application `Conversation`
for the bound MR. It checks the subject, refreshes and corroborates the MR
identity, and reads the title and description. A `null` description becomes
empty text; an absent title or description fails capture. It then reads
`/discussions` to the end within the collection limits above. A skipped page,
a failed page or an exhausted page limit fails capture without a partial result.

Every discussion with at least one non-system note is kept, including notes by
bot users. System notes are omitted, and so is a discussion that contains only
system notes. Each note keeps its ID, numeric author ID as text, the author's
username, the author's bot status, body and verbatim `position` object, or no
position. A missing or blank username is recorded as unknown. Every note in a discussion passes
the discovery scope check: target project, MR, noteable type and IID, and
nonzero note and author IDs. Duplicate discussion or note IDs, empty
discussions, non-object positions and notes without `resolvable` fail capture.
A discussion is resolvable when any of its notes is resolvable. It is resolved
when every resolvable note is resolved; each resolvable note must state
`resolved`.

Positions become canonical text through the JSON adapter's
[`canonical_position`](../json-adapter/README.md#supported-conversation), which
retained conversation documents also use. Publication freshness follows the
[application conversation model](../../application/README.md#conversation-context-and-freshness):
it compares only the summary note and the finding threads that a publication
writes to, and it identifies note authors by numeric ID only.

The MR read and the discussion pages are separate requests, not an atomic
provider snapshot. A change that lands during capture can appear only in a
later capture. The conversation is context: capture parses no markers, verifies
no ownership, establishes no managed finding identity and grants no reply,
answer, policy or effect authority. A bot status is context too; it grants or
removes no authority. Discovery remains the only source of managed review
state. Capture makes no writes.

### Note author accounts

The note `author` object in discussion responses is GitLab's basic user entity:
`id`, `username`, `name`, `state`, `locked`, `public_email`, `avatar_url` and
`web_url`. It has no bot indicator or user type. Capture keeps `id` and
`username` only. `GET /users/:id` returns `bot`, which GitLab sets for every
bot user type. Project and group access tokens create `project_bot` users named
`<project or group>_<ID>_bot_<hex>`; service accounts and internal users such
as GitLab Duo are bot types too. `GET /users/:id` does not return the user type
to an ordinary token.

With `BotLookup::Resolve`, capture asks `GET /users/:id` once for each distinct
non-system note author, in discussion order, for at most `BOT_LOOKUP_LIMIT` (50)
authors. A response for that user with a boolean `bot` sets `Bot` or `NotBot`.
Every other result leaves the author `Unknown`. A missing, hidden or malformed
account affects only that author. Any other failure, such as a rate limit,
transport loss or a server error, stops the lookups, and the remaining authors
stay `Unknown`. A lookup never fails capture. Freshness ignores accounts, so the
CLI resolves only during `run start`; its publication preflight captures with
`BotLookup::Skip`, which sends no lookup and leaves every bot status `Unknown`.

Verified against the pinned CE 19.3.3 server: Developer tokens with `api` or
`read_api` scope read `bot` for a project access token bot (`true`, shown in
notes with a masked name), for humans and for the administrator (`false`).
The `api` token also reads `true` for the private-profile `GitLabDuo` user and
`false` for the deleted-user placeholder `ghost`. GitLab rate limits `GET /users/:id` for non-administrators, by default
300 requests per user every 10 minutes. In the 19.3.3 source, GitLab Duo Code
Review writes as the internal user `GitLabDuo` (name `GitLab Duo`, user type
`duo_code_review_bot`, private profile), which is a bot type, and this server's
`GitLabDuo` user reports `true`. Duo comments on GitLab.com and other
deployments are not verified.

## Publication and uncertain writes

`GitLabHost` binds the MR, authenticated reviewer and explicit authority.
It has no filesystem directory, lock or request journal.
The caller serializes publication for the same review and reviewer.
The [application recovery contract](../../application/README.md#publication-and-recovery)
defines caller-carried `PublicationProgress`, observed settlement and controlled creation retries.
The caller retains that progress across invocations.
The [filesystem adapter](../filesystem-adapter/README.md) retains exact intent
before dispatch and restores caller progress across CLI invocations.

Transport loss, redirects, HTTP 202, HTTP 408 and server errors report uncertainty.
The HTTP client never retries a mutation automatically.
The application may authorize one controlled creation retry under its retained progress contract.
Read-only `GitLab` discovery remains available while publication progress is unresolved.
Matching PUT state cannot prove which request caused it or whether an earlier request can still commit.
Recovery can accept the exact desired state while recording that causation remains unknown.
No provider transaction or idempotency guarantee is implied.

`write_with_checkpoint` completes preflight reads, body validation and request construction before invoking the caller's dispatch checkpoint.
Checkpoint failure prevents the HTTP send. Transport can run only after the caller persists possible dispatch.

Fresh reads check revisions and owned comparison values before effects.
Explicit readbacks verify results afterward.
Expected summary comparison uses a GET followed by a PUT, without atomic compare-and-swap.
Another writer can change the summary between those requests.
Before submitting an answer, the adapter also checks the retained question against the fresh exact note record.
A changed or missing question prevents the answer POST.
Historical discovery and abandonment can still inspect unanswered changed targets.

## Operation logs

The shared [observer](../../observation/README.md) records GitLab operations for library callers inside an observation scope.
Boundaries cover connection, discovery, authority, source capture, conversation capture, retained source decoding, source reads, position checks, body validation, write preparation and HTTP requests.
Each HTTP request belongs to its calling operation through the parent execution ID.
Paginated reads also record the complete collection operation.

Inputs disclose operation kinds, counts, suggestion presence and permitted effect names.
Conversation capture records discussion and note counts only.
URLs, paths, credentials, provider identities, revisions, source text and comment bodies remain masked or absent.
Errors disclose typed categories, rejected HTTP status codes and, for a finding history conflict, the finding number and conflict kind.

`gitlab.http.write` records the transport operation after local request preparation succeeds.
Its start alone does not prove dispatch; the durable checkpoint can still prevent the send.
Its completion distinguishes acknowledgement, rejection and uncertainty.
Acknowledgement does not establish a verified effect; application readback checks establish that result.
Local authority rejection produces no HTTP write attempt.

## Verification

[HTTP workflow tests](tests/http_workflow.rs) exercise the real application use
cases through an owned local HTTP server. They cover clean and multiple-finding
reviews, restart discovery, unchanged reruns, follow-up history, answers,
resolution/reopening, interrupted and lost-response recovery, rejection,
unresolved progress, actor/subject binding, pagination, positions, forked source,
authority limits, redirects, size limits and request timeouts. Unit tests cover
wire validation, source/diff invariants and credentials.
Additional HTTP regressions cover quick actions, extra reply suggestions,
provider-tracked positions after pushes and question edits between application
and adapter reads.
[Conversation tests](tests/support/conversation.rs) cover discussions beyond
page one, system-note exclusion, bot and owned notes, author usernames and bot
lookups with their limit and failures, verbatim inline positions,
resolution mapping, freshness differences, pagination and protocol failures,
and notes outside the bound MR.
Observation regressions inspect emitted JSON, correlation, redaction, rejected and uncertain writes, and publication with a failed log writer.

Run the [workspace checks](../../../../README.md#workspace-checks).

The opt-in [real GitLab suite](../../../../tests/gitlab/README.md) runs these
ports and application use cases against the pinned local CE image. It verifies
stored reviews, suggestions, tracked positions, follow-ups, ownership,
pagination, fork source, conversation capture, write-target freshness scope
and read-only-token rejection with an ordinary reviewer.
The fixture renderer supplies deterministic bodies; no product renderer or CLI
is exercised. Other versions and hosted configurations need their own deployed
verification. Controlled HTTP fixtures retain transport-failure coverage.
The [CLI acceptance fixtures](../../../../tests/cli/README.md) exercise the
production executable with the concrete renderer and filesystem adapters.
The [CLI guide](../../../bin/cli/README.md) defines that command workflow.
