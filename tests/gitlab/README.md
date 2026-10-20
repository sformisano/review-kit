# Real GitLab API tests

This opt-in suite runs the GitLab adapter and application publication use cases
against a real GitLab Community Edition server. The image version and digest
are pinned in the [Rust service helper](support/service.rs) and
[compose.yml](compose.yml). Both services publish HTTP on loopback only.
They publish no SSH port.

## Run

From an isolated worktree, with a local Docker daemon and Rust:

```sh
cargo test -p review-kit-gitlab-adapter --test live_gitlab managed_gitlab_suite \
  -- --ignored --test-threads=1 --nocapture
```

The suite owns one disposable GitLab container through Rust testcontainers.
It selects an available loopback port, waits for reconfiguration to finish, checks readiness,
and bootstraps its administrator token in memory.
It runs the adapter scenarios sequentially against that container.
The container is limited to eight CPUs and 16 GiB of RAM.
Puma uses two workers for the sequential scenarios.
First startup includes the image download and database initialization.
Reconfiguration and the subsequent readiness check each have a 15-minute limit.
Token bootstrap has a three-minute limit.
Dropping the service removes its container and anonymous volumes, including after a scenario panics.
The [CLI E2E suite](../cli/README.md#repeatable-cli-suite) uses the same helper.

## Reuse an existing server

Set both variables to use an existing server without creating or removing a container:

```sh
REVIEW_KIT_GITLAB_URL=http://127.0.0.1:8929 \
REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE="$PWD/tests/gitlab/.local/admin-token" \
cargo test -p review-kit-gitlab-adapter --test live_gitlab managed_gitlab_suite \
  -- --ignored --exact --test-threads=1 --nocapture
```

The endpoint must use HTTP, the literal host `127.0.0.1`, and an explicit port.
Credentials, paths, query strings, and fragments are rejected.
Run one suite at a time against an existing server.
Set neither variable to select a disposable server.
Setting only one variable fails setup.
Malformed credentials and rejected API requests fail setup immediately.

To create a persistent external server, start Compose:

```sh
docker compose --project-name "${REVIEW_KIT_GITLAB_PROJECT:-review-kit-gitlab-live}" \
  --file tests/gitlab/compose.yml up --detach
```

Compose publishes the fixed endpoint `http://127.0.0.1:8929`.
The Compose project defaults to `review-kit-gitlab-live`.
`REVIEW_KIT_GITLAB_PROJECT` overrides that name.

Probe readiness inside the container:

```sh
docker compose --project-name "${REVIEW_KIT_GITLAB_PROJECT:-review-kit-gitlab-live}" \
  --file tests/gitlab/compose.yml exec -T gitlab \
  curl --fail --silent --max-time 5 --output /dev/null \
  'http://127.0.0.1:8929/-/readiness?all=1'
```

Repeat the probe until it exits successfully before bootstrapping the administrator credential.
First initialization can take several minutes.
The probe runs inside the container because GitLab restricts monitoring endpoints by client IP.
[GitLab health checks](https://docs.gitlab.com/administration/monitoring/health_check/)
describe that restriction.

Save the fixture credential privately:

```sh
umask 077
mkdir -p tests/gitlab/.local
chmod 700 tests/gitlab/.local
install -m 600 /dev/null tests/gitlab/.local/admin-token
docker compose --project-name "${REVIEW_KIT_GITLAB_PROJECT:-review-kit-gitlab-live}" \
  --file tests/gitlab/compose.yml exec -T gitlab \
  timeout --signal=KILL 180 gitlab-rails runner - \
  < tests/gitlab/bootstrap.rb > tests/gitlab/.local/admin-token
```

The server-side Ruby script creates the token without printing it to the terminal.
Use the external-server variables above to run Cargo tests against this service.
To stop it while preserving its data:

```sh
docker compose --project-name "${REVIEW_KIT_GITLAB_PROJECT:-review-kit-gitlab-live}" \
  --file tests/gitlab/compose.yml stop
```

[GitLab Docker installation](https://docs.gitlab.com/install/docker/installation/)
describes the image configuration.

## What the suite proves

Fixtures create private nested projects and separate author and reviewer users.
The author has Maintainer access to seed the protected default branch. The
adapter uses the reviewer's Developer token, never the administrator token.
A second token for the same reviewer has only `read_api` scope.
The conversation scenarios add a Developer commenter and a project access
token bot user. When the server refuses the token, a scenario skips the bot
note and says so in its output.

The scenarios verify:

- Clean and inline reviews, exact committed-source reads and complete diffs.
- Modified binary blobs remain unsupported entries in retained source capture,
  while changed text remains usable and binary source/position checks reject it.
- Stored author identities, positions, bodies, summaries and root IDs.
- Restart discovery and unchanged reruns without new effects or rounds.
- Single-line, multiline and deletion suggestion contents from GitLab.
- Author answers, tracked positions after a push, resolution, reopening and history.
- Preservation of foreign comments and rejection of stale assessments/read-only writes.
- Fork source reads with publication in the target project, and owned records
  beyond the first notes/discussions page.
- Conversation capture beyond the first discussions page. General, project
  bot, unmanaged inline and resolvable thread notes match independent REST
  reads, with verbatim positions and each author's username and `bot` flag
  from independent account reads. System notes are excluded. Edited and
  deleted notes are reflected. A later edit and resolution change are
  reported as differences from the capture.
- Publication freshness scoped to the reviewer's threads with real discovery.
  After a published round, notes by the commenter and the bot, a diff thread
  that copies a finding marker, a foreign resolution and title and description
  edits are outside the scope; the author's reply and resolution in the
  reviewer's finding thread are the only owned changes.

An independent REST fixture client inspects GitLab state after application
operations. A deterministic renderer supplies test bodies; this suite does not
test a product renderer, filesystem adapter or CLI. Controlled HTTP tests remain
responsible for deterministic transport loss and timeout cases.

Passing establishes behavior on the pinned self-managed CE version. It does not
establish other versions or GitLab.com-specific settings. The adapter's complete
[supported-source contract](../../crates/lib/infrastructure/gitlab-adapter/README.md#source-and-suggestion-support)
still applies.

## Credentials, failures and cleanup

The disposable service keeps its administrator token in memory.
The Compose bootstrap command saves its administrator token under ignored `.local/` with mode `0600`.
Reviewer tokens exist only in test memory. No credential is printed.
Fixture clients restrict writes to the validated loopback endpoint.

Each scenario creates a new namespace. A successful scenario requests deletion
of only its own resources; GitLab applies its deletion policy. A project bot
user is removed with its project under that policy. A failed scenario
retains its namespace and users for inspection and prints the namespace name.
An external server preserves failed fixtures after the suite ends.
A disposable service removes them when its container is dropped.
Readiness and credential-bootstrap failures save logs under ignored `.local/` with mode `0600` on Unix.
Set `TESTCONTAINERS_COMMAND=keep` before a managed run to retain its container for private inspection.
This setting also preserves containers after successful tests.
Remove only the printed container ID when inspection finishes.
GitLab logs and files can contain credentials, so keep inspection output private.

The Compose volumes remain until explicitly removed. They contain local test
credentials and repository data.

For a targeted run after `up`, set the endpoint and token-file path:

```sh
REVIEW_KIT_GITLAB_URL=http://127.0.0.1:8929 \
REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE="$PWD/tests/gitlab/.local/admin-token" \
cargo test -p review-kit-gitlab-adapter --test live_gitlab initial_and_clean \
  -- --ignored --test-threads=1 --nocapture
```

Individual scenario tests require both external-server variables.
The managed suite starts a disposable server when those variables are absent.
Ordinary workspace tests list these cases as ignored and require no GitLab server.
Keep local logs and execution evidence under ignored `.local/` or outside the
repository. Record the image digest, source revision and results when claiming
live compatibility.
