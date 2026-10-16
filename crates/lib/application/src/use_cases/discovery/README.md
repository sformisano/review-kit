# Read review observations

Discovery reads the provider's current review observations for a requested
subject and authenticated reviewer. It establishes the basic context needed
before drafting or planning publication.

Use `discover` when the caller needs a `Snapshot` of revision references,
permissions, summaries, finding roots, and replies. The snapshot preserves
observed records. It does not reconstruct the latest communicated finding judgments.
`discovery_bundle::discover_bundle` adds that reconciliation and the changed-file context.

## Inputs and result

The caller supplies a `ReviewReader`, the canonical `Subject`, and the expected
`ReviewerId`. The reader supplies provider observations and authenticated identity.
The use case checks that the snapshot belongs to the requested subject and reviewer.

It also checks owned record identities and summary round structure. Duplicate
owned summaries, finding IDs, provider IDs, invalid round numbers, and foreign
assessment identities produce `DiscoveryConflict`. A reader failure produces
`DiscoveryError::Read`; structural conflicts produce `DiscoveryError::Conflict`.

The reader's contract includes complete pagination, resolved discussions,
API-confirmed authorship, and current provider permissions. Review markers alone
cannot establish ownership.

## Example

A follow-up review starts after a merge request changes. Discovery reads the
current revision and retained review records. An owned summary from another
review subject causes a conflict before the caller uses that context to draft.

## Boundaries

Discovery calls only the reader port and performs no provider writes.
The reader can perform network I/O. The returned snapshot is an observation,
not a lock, authorization grant, or guarantee that the provider remains unchanged.

Basic discovery does not read changed files, compare committed suggestion text,
validate immutable completed effects, or reserve the next finding number.
Those checks belong to the bundle and later assessment or publication use cases.
