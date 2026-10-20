# GitLab adapter

Status: `review-kit-gitlab-adapter` implements the application ports with real HTTP.
Controlled HTTP tests verify publication and recovery through application use
cases. The [local CE suite](../tests/gitlab/README.md) also verifies stored results
and reviewer-token behavior on its pinned server image.

The [adapter README](../crates/lib/infrastructure/gitlab-adapter/README.md) owns configuration, exact v1
records, supported source/position cases, publication limits and
verification. The [architecture](architecture.md) owns the component boundaries;
the [application README](../crates/lib/application/README.md) owns use-case contracts.

## GitLab concepts that need distinct types

The adapter distinguishes deployment/API URLs, target and source project IDs,
project-scoped MR IID, authenticated user ID, root note ID and discussion ID.
Its [identity types](../crates/lib/infrastructure/gitlab-adapter/src/identity.rs) preserve those roles.
The application uses `ProviderId` for opaque references; the adapter translates
each in the operation's scope. Diff positions retain head/base/start SHAs,
old/new paths, both context coordinates and optional line ranges.

## Operation mapping

All paths below are relative to the configured deployment's `/api/v4`.
`P` is the target project, `S` the source project, and `M` the MR IID.

| Operation | GitLab API |
| --- | --- |
| Authenticate | `GET /user` |
| Corroborate project | `GET /projects/:encoded_path` |
| Read MR identity/revision | `GET /projects/P/merge_requests/M` |
| Select exact complete diff | Paginated `GET .../versions`, then `GET .../versions/:id` |
| Discover summary | Paginated `GET .../notes`, direct `GET .../notes/:id` |
| Discover roots and replies | Paginated `GET .../discussions`, direct `GET .../discussions/:id` |
| Capture supported conversation | `GET /projects/P/merge_requests/M` for title and description, paginated `GET .../discussions`, then `GET /users/:id` once per distinct note author for the bot status |
| Read committed source | Paginated `GET /projects/S/repository/tree`, `GET /projects/S/repository/files/:encoded_path/raw` at head SHA |
| Create/update owned summary | `POST .../notes`, `PUT .../notes/:id` |
| Publish inline finding | `POST .../discussions` with text position |
| Publish answer/transition | `POST .../discussions/:id/notes` |
| Resolve/reopen | `PUT .../discussions/:id` with `resolved` |

Discovery and source operations implement `ReviewReader`, `ReviewChanges` and
`CommittedSource`. [Conversation capture](../crates/lib/infrastructure/gitlab-adapter/README.md#conversation-capture)
returns the application's `Conversation` for run capture and the publication freshness check. Publication implements `PublicationHost` and its acquired
`PublicationSession`. The session binds identity and authority.
The application decides effects, ordering and results, with caller-carried publication progress.

## Deployment boundary

The local CE suite establishes its tested server/token behavior through real
API calls. Other deployments need their own authorized disposable-MR smoke test.
Neither suite establishes visibility guarantees for unresolved requests. Unknown PUTs remain a concrete
recovery limit described in the
[publication contract](../crates/lib/infrastructure/gitlab-adapter/README.md#publication-and-uncertain-writes).
The [CLI](../crates/bin/cli/README.md) wires this adapter with review-run storage,
retained JSON and embedded or external rendering profiles. GitLab is the only supported provider.
