# review-kit-filesystem-adapter

This crate stores review documents as ordinary files.
It implements the application's draft, retained discovery, clock, and execution-evidence ports.
The [JSON adapter](../json-adapter/README.md) owns review-document formats and validated conversions.

## Workspace storage

`Workspace` resolves relative paths beneath a caller-selected root.
Paths cannot contain parent traversal or absolute components.
The caller controls the workspace and serializes publication for each subject and reviewer.
This adapter provides a local publication lock and versioned progress checkpoints.

`read_json` and `write_json` handle serialization.
`load_context` validates retained discovery.
`load_draft` validates the version and context reference.
`SavedContext` implements read-only discovery and revision-bound change access.
Provider source documents use `read_bytes` and `write_bytes` with the provider's codec.

Workspace reads and writes are limited to 64 MiB per file.
Writes use a complete sibling temporary file, file synchronization, and atomic replacement.
On Unix, temporary files have mode `0600` and the parent directory is synchronized.
A synchronization failure can follow a completed replacement; callers retain the error and refresh the file.

`AssessmentDraftWriter` creates a draft only when its destination is absent.
A hard link installs the fully written temporary file without replacing existing work.
An existing destination returns `DraftCreation::Existing` with its bytes unchanged.

Keep each discovery context and corresponding source file immutable after an assessment references them.
Review runs keep their captures in the run directory described under [Review runs](#review-runs).
Refreshing discovery does not authorize replacing an existing assessment or publication progress.


## Durable publication intent

`Workspace::load_progress` restores local `progress.json` and checks its subject and reviewer.
For a workspace without execution evidence or results, it creates an explicit bound empty progress record.
If progress is missing while `evidence.json` or `result.json` exists, it refuses publication.
A malformed or differently bound progress record also prevents publication.

`FileProgressStore` keeps authoritative progress under a caller-selected common state root.
Its directory key is SHA-256 over length-prefixed subject and reviewer fields.
`FileProgressStore::new` provides explicit generic progress and [current run-journal](#run-bound-journal) methods in the same directory.
Revision directories for the same subject and reviewer share one `progress.json`.
`root()` exposes the common root; `path()` exposes `progress.json` and `journal_path()` exposes `publication-progress.json`.
`load(workspace)` imports validated local progress only when the review's shared directory is absent.
The import leaves the local file bytes unchanged as a snapshot.
An existing shared record always takes precedence over local snapshots.
An existing shared directory without its progress record refuses loading and saving.
Malformed or differently bound shared progress also prevents publication.
With `publication-progress.json` present, `load` and `save` fail with `FileError::RunJournalExists`.
`save` also refuses progress with run-bound state instead of dropping that state.

If progress exists only in a workspace, import or recover it before publishing from another revision directory.
The store cannot discover unresolved records in arbitrary earlier directories.
Keep the common state root available across all invocations for the same review.
Call `FileProgressStore::try_lock()` before loading or importing progress.
Retain the returned `ProgressLock` through provider effects, evidence and final progress persistence.
The operating system releases the lock when its file handle closes or the process terminates.
A competing holder returns `FileError::PublicationBusy` without waiting.
The stable sibling `.lock` file survives progress replacement and remains on disk after release.
Do not delete or replace it while commands can run.
Separate roots or hosts still require caller serialization.
Library callers that omit the guard retain responsibility for excluding other writers.

`FileEvidence` preserves existing events for the same subject and reviewer.
`with_renderer` attaches the host-selected profile to new rendered events without changing existing metadata.
`with_profile` records the full profile identity in both evidence and unresolved
intent. The CLI uses it before dispatch so a crash cannot lose the profile binding.
Every application event replaces `evidence.json` atomically.
`FileEvidence::with_progress_store` keeps evidence local and sends progress checkpoints to the selected shared store in its journal format.
`ExecutionEvent::Progress` saves prepared intent, possible dispatch, original grants, consumed retries and settlement.
`BeforeWrite` saves execution evidence without replacing an existing complete intent.
Evidence or progress-storage failure prevents dispatch.
`FileEvidence::new` retains workspace-local progress for callers selecting that storage contract.
`FileEvidence::for_run(writer, journal)` keeps a review run's events in `review-publication-events.json`, read and replaced only through the `RunWriter`, and checkpoints progress into the run-bound `publication-progress.json`.
A symbolic link or non-file entry at that name fails with `FileError::RunStorage` before anything is read or written, as every [run file](#review-runs) does.
The evidence borrows the writer, and so the workspace lock, for its lifetime.
Resolution receipts and their observation provenance are checkpointed through `ExecutionEvent::Progress`.
Later write intent preserves those receipts, including across process restarts.

After publication or abandonment, the caller saves its actual `PublicationProgress` with the selected store's `save` method.
Workspace-local callers use `save_final_progress`.
The caller performs this save even when the run or final evidence recording fails.
Readback events do not clear retained intent.
The following settlement checkpoint clears it after application verification.
A crash between those events leaves intent available for exact observed recovery.
The [application contract](../../application/README.md#publication-and-recovery) defines reconciliation and bounded retries.
On Unix, atomic writes synchronize their files, containing directories and ancestor directory entries.
This includes directories left by a failed or interrupted creation.
These calls rely on the filesystem's durability guarantees; tests do not simulate physical power loss.

`SystemClock` supplies host time for recorded runs.
Storage failures retain their I/O or JSON causes for the CLI's diagnostic owner.

## Review runs

The `runs` storage keeps review runs in `<worktree-root>/.review-kit/`.
It owns file names, the workspace lock and write durability.
It does not interpret documents: JSON files use caller-supplied serde types, and drafts and accepted snapshots are exact bytes.
The [JSON adapter](../json-adapter/README.md) owns the document schemas.

### Worktree and environment

`GitWorktree::locate(directory)` runs `git rev-parse --show-toplevel` without a shell.
A subdirectory resolves to its worktree root, and each linked worktree has its own root.
Outside a worktree, including in a bare repository or a `.git` directory, it returns `RunStorageError::NotInWorktree`.

`RuntimeStore::open(worktree)` checks `.review-kit/` before any runtime file is read or written:

1. `.review-kit` is absent or a real directory. `.gitignore`, `review-workspace.json`, `review-workspace.lock`, `staging/` and `review-runs/` are not symbolic links (`Redirected`) and have the expected file type (`UnexpectedKind`).
2. `git ls-files -z -- .review-kit` lists no path (`Tracked`).
3. If `review-workspace.json` is absent, `.review-kit/` contains only the names in step 1 and temporary files (`Unrecognized`).

Entries inside a run directory are checked when they are accessed.
A symbolic link at any managed path is rejected, and file opens use `O_NOFOLLOW` on Unix.

### Layout

| Path below `.review-kit/` | API |
| --- | --- |
| `.gitignore` | Written as `*` by the lock holder when absent or different |
| `review-workspace.json` | `RuntimeStore::read_workspace`, `WorkspaceLock::write_workspace` |
| `review-workspace.lock` | `RuntimeStore::lock`, `RuntimeStore::create_and_lock` |
| `staging/<run-id>/` | `WorkspaceLock::stage_run`, `RuntimeStore::staging_leftovers` |
| `review-runs/<run-id>/<file>` | `RunFile` names, read by `RunDir` and written by `RunWriter` |
| `review-runs/<run-id>/accepted/<hex>/review-assessment.json` | `RunWriter::install_accepted`, `RunDir::read_accepted` |
| `review-runs/<run-id>/draft-history/<hex>/<draft file>` | `RunWriter::archive_draft`, `RunDir::read_archived_draft` |

`RunFile` names every fixed run file. `SnapshotFile` covers the four retained capture files, which only a staged run can write.
`WorkingFile` covers the manifest, assessment draft, diagnostics, publication files and `rendered/` previews.
Callers cannot supply other paths. `<hex>` is the lowercase SHA-256 of the exact bytes, and `content_ref` returns its `sha256:` reference.
`RunDir::path` is the run root. Retained references such as `review-context.json` resolve against it, never against a digest directory.

### Locking and access

`RuntimeStore::lock` locks an existing runtime directory and returns `Uninitialized` when `.review-kit` is absent.
`RuntimeStore::create_and_lock` creates the directory first.
Both take an operating-system lock on `review-workspace.lock` without waiting and return `WorkspaceBusy` while another handle or process holds it.
The returned `WorkspaceLock` keeps the lock until it drops or the process ends; the lock file remains.
While locked, the guard checks the environment again, restores `.gitignore`, and removes temporary files from `.review-kit/`.

Reads need no lock: `RuntimeStore` reads the workspace file and lists runs, and `RuntimeStore::open_run` returns a `RunDir`.
Every mutation goes through the guard:

- `write_workspace` replaces `review-workspace.json`.
- `stage_run` creates `staging/<run-id>/` and returns a `StagedRun`.
- `run` opens a `RunWriter`, which borrows the guard and dereferences to `RunDir`.

`RuntimeStore::list_runs` returns real directories named by a valid run ID and reports every other entry as unrecognized.
`RuntimeStore::staging_leftovers` lists staging directories the same way and deletes nothing.

### Runs and accepted assessments

`StagedRun::write` stores any run file. `StagedRun::install` requires the manifest and the four retained files (`IncompleteStaging`).
It renames the staging directory to `review-runs/<run-id>/` and synchronizes both parent directories.
It never replaces an existing run directory: `stage_run` and `install` check the target under the lock, and the rename refuses a non-empty directory (`RunExists`).
Installation does not change `review-workspace.json`; the caller selects the run with a later `write_workspace`.

`RunWriter::replace` replaces a working file. `RunWriter::create` writes it only when absent and returns false otherwise.
`RunWriter::remove_rendered` deletes a stale preview.
`RunDir::read_optional_bytes` reports an absent file as `None`; `read_bytes`, `read_json` and `hash` return `MissingFile`.
`RunDir::verify_snapshot` hashes the four retained files and returns `SnapshotMismatch` with each changed or missing file.
An unchanged revision does not prove unchanged bytes.

`RunWriter::install_accepted` stores accepted assessment bytes at `accepted/<hex>/` and returns their reference.
`RunWriter::archive_draft` stores draft bytes, valid JSON or not, at `draft-history/<hex>/` with the same rules.
Identical stored bytes return `created: false`. Stored bytes that do not hash to their directory name return `ContentCorrupt` and stay unchanged.
`RunDir::read_accepted` and `RunDir::read_archived_draft` verify the digest and return `ContentMissing` or `ContentCorrupt`.

### Durability and interrupted operations

Every write completes and synchronizes a temporary file in the destination directory.
It installs the file with one rename, or with a hard link where the destination must not be replaced.
Then it synchronizes the directory. New directories are synchronized in their parent.
Accepted and archived bytes are linked from a temporary file in `accepted/` or `draft-history/` into the digest directory.
Reads and writes are limited to 64 MiB per file (`FileTooLarge`).

After any error, reopen the store and inspect its state. Each failure leaves a defined state:

- Before installation, the destination is unchanged. A temporary file named `.rkc-*.tmp`, an empty digest directory or a staging directory can remain.
- `Unsynchronized` means the change is visible but its durability is uncertain.
- Readers never open temporary files, and a retried acceptance completes an empty digest directory.
- Opening a `RunWriter` removes temporary files from the run directory, `accepted/`, `draft-history/` and `rendered/`. It skips redirected directories.
- A staging directory never becomes a run. It stays for diagnosis until someone removes it.
- A run installed before the workspace file changed is listed and readable, and the previous selection remains.

Operations log fixed operation names, fixed file labels, byte counts and error categories.
Paths are masked; document contents and Git output are excluded.
Tests in `tests/runs/` use temporary Git repositories and cover these environment, lock, staging, checkpoint and recovery cases.

## Run-bound journal

`publication-progress.json` holds [version 3 progress](../json-adapter/README.md#publication-progress) in the review's shared directory.
It uses the same stable lock as `progress.json`; hold `try_lock()` before each call below and through the final save.
The [application binding contract](../../application/README.md#review-run-binding) defines what the journal records.

`open_journal(history)` establishes the journal from the stored files:

| Stored state | Result |
| --- | --- |
| No review-key directory; `history.admits_empty_journal()` | Creates the directory together with an empty journal: `JournalOrigin::Bootstrapped` |
| No review-key directory; any other `history` | `FileError::MissingProgress`; nothing is created |
| Directory without `publication-progress.json`, regardless of unrelated files | `FileError::MissingProgress`; an initialized store is never recreated |
| Current journal present, regardless of unrelated files | `JournalOrigin::Loaded`; unrelated files are not decoded, removed or changed |

The application's [empty journal admission](../../application/README.md#empty-journal-admission) decides which provider history admits an empty journal.
Bootstrap only checks stored files and that decision; the caller decides whether first use is legitimate.
`rkc` never opens a journal for a run that already attempted publication, because a write of that attempt may have reached the provider.

Bootstrap writes and synchronizes the journal inside the staging sibling `.rkc-<review-key>.tmp`, renames it to the review-key directory and synchronizes the shared root.
An existing review-key directory therefore always had a journal when it was created.
Every reader ignores a staging directory left by an interrupted bootstrap; the next bootstrap, under the lock, removes it without following links.
`history` comes from `ManagedHistory::observe` and must describe this store's subject and reviewer; otherwise `FileError::EvidenceBinding`.
Workspace-local progress is never imported into this journal.
An empty state root cannot show that another host has no pending request, so keep one authoritative root.

`inspect_journal()` returns `JournalView::Absent` or `Journal` without changing files.
`load_journal()` requires the current journal; unrelated files do not affect loading.
`save_journal(progress)` replaces an existing journal after validating its version 3 relations.
It refuses to drop or change a retained settlement with `FileError::JournalRegression`.
Writes use the atomic sequence described under workspace storage.

## Operation logs

Within a scoped [observer](../../observation/README.md), storage operations emit
structured start and completion records. Coverage includes file reads, JSON
storage, atomic writes, context loading, draft creation, progress
persistence, journal opening and inspection, and evidence recording. Nested operations retain parent execution IDs.
Logs include byte and item counts, required-addition counts, creation and installation flags, evidence event
kinds, and fixed error categories. Paths, policy-set names and individual policy IDs are fully masked;
document contents, provider identifiers, and formatted error chains are excluded.

An atomic write records whether the destination was installed. A later parent
synchronization failure reports uncertain durability. The original storage error
still reaches the caller. Logging failure does not change storage or evidence
results. Evidence persistence retains its authority to prevent provider dispatch.

## Verification

Run the workspace checks in the [domain README](../../domain/README.md#verification).
Tests cover competing draft creation, existing agent work, restart recovery, shared intent across revision directories, missing progress, and failed persistence before dispatch.
Journal tests cover first-use bootstrap, bootstrap after a finished round, refusal over an unfinished or ambiguous round, staging leftovers of an interrupted bootstrap, missing journals despite unrelated files, generic API ownership refusal, unrelated-file preservation, settlement retention and the durable dispatch flag observed at the provider call.
