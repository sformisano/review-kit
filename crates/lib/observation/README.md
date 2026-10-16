# review-kit-observation

This crate records synchronous operations through slog, slog-json, and Redactable.
The CLI writes newline-delimited JSON to its configured destination.
Libraries emit nothing unless the caller installs an observation scope.
Command results go to stdout; the [CLI reference](../../bin/cli/README.md#output-and-exit-status) defines their format and exit codes.

## Collect CLI logs

Point your logging agent at the file selected by
[CLI logging preferences](../../bin/cli/README.md#logging-preferences).
For the default file, inspect records with:

```sh
tail -F "$HOME/.review-kit/logs/review-kit.jsonl"
```

Configure the agent to parse newline-delimited JSON and send each record to your sink.
Keep the nested `execution` object intact. Use `execution_id` and `parent_execution_id` to correlate operations.
The agent owns remote delivery, collection buffering, rotation and retention.

Keep command-result stdout separate from logs. Results can contain review content and are not redacted log records.
When using stderr delivery or fallback, collect stderr separately from stdout.
If a collector reads stderr through a pipe, drain it while the command runs: writes are synchronous.
The [delivery limits](#delivery-and-limits) describe failures and incomplete records.

## Library use

```rust
use review_kit_observation::{Observer, with_observer};

let observer = Observer::json(std::io::stderr());
with_observer(&observer, || {
    // Call Review Kit application or adapter operations here.
});
```

Instrumented application and adapter operations share the scope automatically.
Observation projects typed business values into log fields; callers receive the business results.
Custom ports receive application-level records. Their internal work needs its own instrumentation.
Nested calls inherit the current execution ID as their parent.
Replacing an observation scope starts a new root and restores the previous scope afterward, including during unwinding.
New threads require their own scope; context is not inherited implicitly.
After a thread's observation context is destroyed, operations and scope calls run their business bodies without logging.

For a file sink, pass an append-mode file to `Observer::json`:

```rust
use std::fs::OpenOptions;
use review_kit_observation::{Observer, with_observer};

fn main() -> std::io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open("operations.jsonl")?;
    let observer = Observer::json(file);
    with_observer(&observer, || {
        // Call Review Kit application or adapter operations here.
    });
    Ok(())
}
```

`Observer::json` accepts a writer implementing `std::io::Write + Send + 'static`.
The caller controls file permissions and where the logging agent reads it.

## Generated operation boundaries

Application workflows, internal stages, adapter operations and CLI handlers use generated boundaries.
Business callers invoke ordinary functions or managed port methods; they do not call `observe`.
The domain crate has no logging code or dependency.

[`#[operations]`](../observation-macros/README.md#operation-declarations) generates the shared observation boundary around an operation's body.
The macro README defines declaration syntax, required disclosure policies and supported function forms.
Input projection panics produce a fixed fallback and preserve business execution; completion projection panics produce `unclassified`.
Without an observer scope, neither projection runs.

[`#[observed_port]`](../observation-macros/README.md#provider-declarations) generates managed access to application ports.
Public application entry points bind caller-owned ports before internal composition. Internal stages reuse managed handles.
This covers provider reads, acquisition, authority, writes and readbacks, plus source, renderer, changes, draft and evidence ports.
Checking places managed handles around its per-invocation caches. Each managed request logs once, including cache hits.
An acquired publication session borrows its host.

See the [discovery operations](../application/src/use_cases/discovery/mod.rs), [port declarations](../application/src/ports.rs), and [generator contract](../observation-macros/README.md).
The [architecture tests](../application/tests/operation_architecture.rs) discover production source files across application, infrastructure and CLI.
They check declarations, direct runtime bypasses and stored raw ports, with explicit exemptions for pure models and boundary owners.
These are source-structure checks, not a Rust name resolver.
New meaningful operations need a declared boundary; helper exemptions and disclosure policies require review.
A caller invoking its own raw adapter outside Review Kit's managed path receives no automatic interception.

Run the offline [custom-reader example](../application/examples/observed_discovery.rs) from the repository root:

```sh
cargo run -p review-kit-application --example observed_discovery --locked --quiet
```

The caller and custom reader contain no `observe` calls.
Stderr includes discovery, provider-read and snapshot-validation records with parent IDs.
Nested validation stages appear as children.
Stdout reports the discovered finding count. The example contacts no provider and changes no files.

## Record format

Each started operation attempts a `started` record and one `completed` record.
The JSON envelope contains `ts`, `level`, `msg`, and an `execution` object.

| Execution field | Meaning |
| --- | --- |
| `version` | Numeric log schema version, currently `1` |
| `operation` | Fixed operation name authored at its boundary |
| `phase` | `started` or `completed` |
| `execution_id` | Diagnostic UUID shared by start and completion |
| `parent_execution_id` | Enclosing execution UUID, or `null` at a root |
| `input` | Selected redacted facts on start; `null` on completion |
| `output` | Selected redacted result facts on completion, or `null` |
| `error` | Declared type, variant, fields, cause and opacity; otherwise `null` |
| `duration_us` | Integer elapsed microseconds on completion; `null` on start |
| `outcome` | Completion classification; `null` on start |

Objects and arrays remain nested JSON. Numbers, booleans, and nulls retain their JSON types.
Execution IDs correlate logs within an invocation; they do not identify persisted review or recovery records.

Completion outcomes include `success`, `failure`, `rejected`, `uncertain`, `partial`, `blocked`, `panicked`, and `interrupted`.
`unclassified` means the body returned but its logging projection failed.
An outer `Ok` with a partial or uncertain publication result retains that publication classification.
Failure, uncertainty and panic use error level. Partial, rejected, interrupted and unclassified outcomes use warning level.
Starts and other completions use info level.

## Disclosure policy

`Fields` admits counts, numbers, booleans, fixed labels, nested objects, lists, nulls, declared errors, and fully masked strings.
Keys and labels must be authored constants. Review those declarations as disclosure decisions.
Dynamic text uses `secret` or is omitted.
Redactable's `.slog_redacted_json()` produces an owned redacted value before the drain receives it.
Both slog and slog-json enable `nested-values`.

Projections retain modes, effect kinds, counts, validation categories, publication states, and result flags.
They omit or fully mask credentials, URLs, provider identities, paths, revisions, policy names, source text, review prose, and rendered bodies.
Operations never serialize whole configurations, assessments, provider objects, or CLI result documents into logs.
Business values remain available to their callers; redaction applies only to logging projections.

Errors use explicit category matches. Unknown adapter causes are opaque, including their entire source chain.
Logging never formats those causes with `Display` or `Debug`.
Serialization failure produces `"[REDACTED]"` without serializer details or partial input.
The CLI omits panic payloads from its panic hook; operation guards report unwinding without the payload.
Library callers own their panic hook and any diagnostics it prints independently.

## Operation coverage

| Family | Boundaries |
| --- | --- |
| CLI and input | Parsing, commands, result emission, configuration, connection setup, assessment/context/source loading |
| Application workflows | Policy selection, discovery, initialization, assessment preparation/completion/checking, planning, publication and abandonment |
| Internal application stages | Snapshot/finding/shape/reply validation, plan building, history/progress reconciliation, effect application and verification |
| Application ports | Acquisition, authority, discovery, writes, readbacks, source/position checks, rendering, changes, drafts and evidence checkpoints, including custom ports |
| GitLab adapter | Connection, discovery, HTTP reads/mutations, source capture, retained-source decoding, position checks and publication validation |
| Filesystem/JSON adapters | File/document loads, atomic writes, context/draft/progress/evidence operations and validated document conversions |
| Renderer adapter | Profile loading, compilation, export, role rendering and nested suggestion rendering |

Pure domain constructors, getters, loops, and small transformations have no individual records.
Counts and declared categories are projections, not complete business-object dumps.
Adapter READMEs describe their boundary-specific details.

## Effects and evidence

An `application.provider.write` start records invocation of the publication port, including adapters that reject before sending HTTP.
A `gitlab.http.write` start records the transport operation after local request checks, before its durable dispatch checkpoint.
Their completions distinguish acknowledgement, rejection, and uncertainty.
Readback verification records whether the observed effect matches the intended write.
A matching readback can settle retained intent under the [recovery contract](../application/README.md#publication-and-recovery), without proving request causation.
An existing matching summary can verify without a new write attempt.
Publication records separate attempted effects, verified matches, unresolved intent, and finalization failures.
Final evidence failure remains separate from the actual publication outcome.

Logging is independent of `ExecutionEvidence`.
Evidence persistence can prevent a provider write. Logging failure cannot grant authority, suppress evidence, retry a write, or change a returned result.

## Delivery and limits

`Observer::with_listener` adds typed lifecycle notifications without parsing log text.
The listener receives operation names and start/completion events, with the completion outcome.
Notifications contain no input fields, paths, provider identifiers or bodies.
Callbacks run before log delivery and must return promptly; unwinding callback panics are contained.
The CLI uses a bounded queue to send these notifications to its [progress worker](../../bin/cli/README.md#progress-output).
Excess progress events can be dropped without delaying the operation.

The sink is synchronous and flushes each record. The CLI returns normally after its final observation; no logging queue remains pending.
An external agent can collect the CLI's configured destination or a library caller's writer.
Review Kit does not export traces, metrics, or logs to remote services.

Write and flush errors are discarded without fallback diagnostics.
Projection and writer panics are contained when Rust unwinds; the host's panic hook still runs.
Caught logging panic payloads are retained without destruction because their destructors can panic again.
This leaks the caught payload on that failure path. Business panics propagate their original payloads.
Unused input and completion closures also dispose of their captures inside the protected logging boundary.
The observer owns its sink and contains unwinding failures from final sink destruction.
Recursive observation from a writer is suppressed to avoid reacquiring the sink.
Writers must return from writes and flushes; a blocked sink can delay an operation.
The CLI also joins its progress worker on normal completion; blocked stderr can delay that shutdown.
Failed sinks can produce missing or partial records. Process aborts cannot guarantee terminal records.
Logs provide no durable delivery or recovery authority.

## Verification

`cargo test --workspace --locked` exercises production JSON output, library scopes, CLI subprocesses, HTTP fixtures, and storage failures.
Tests check JSON types, parent IDs, masking, opaque errors, projection failures, write/flush failures, coverage, and business results when logging fails.
Real GitLab suites run separately under the [GitLab test guide](../../../tests/gitlab/README.md).
