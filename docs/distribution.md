# Binary releases

Review Kit packages `rkc` with its source, documentation, renderer assets and license notices.
Version `0.1.0` is distributed through [GitHub Releases](https://github.com/sformisano/review-kit/releases/tag/v0.1.0).
The crates are not published on crates.io, and no Homebrew package is provided.

The binary target is `x86_64-unknown-linux-gnu`.
The release build uses Debian Bullseye. Runtime checks cover Rocky Linux 9,
Ubuntu 24.04 LTS and Ubuntu 26.04.1 LTS on x86_64.
Other distributions, macOS, Windows and ARM require separate verification.
`BUILD.json` records the binary's runtime libraries and required glibc symbol versions.
An older glibc installation can reject the executable before `rkc` starts.

The [GitLab CE suite](../tests/gitlab/README.md) defines the pinned local server used for provider checks.
The [CLI suite](../tests/cli/README.md#repeatable-cli-suite) uses disposable merge requests on that server.
These checks do not establish support for GitLab.com or another server version.

## Build identity

`BUILD.json` identifies the exact source commit and executable checksum in
`source_commit` and `binary_sha256`. `SHA256SUMS` identifies the downloaded archive.

Keep the complete installed package and its checksum file for rollback.
Record the source commit and executable checksum when reporting a problem.
The download URL and version string identify the release; checksums identify its bytes.

## Build an archive

The release builder requires Linux, Bash, Git, GNU coreutils and Docker.
It uses a digest-pinned Debian Bullseye image with Rust 1.98.1 and its `rust-docs` component.
Cargo downloads the versions fixed by `Cargo.lock` when its cache lacks them.
The build requires a clean committed checkout and a new output directory outside the checkout.

From the repository root, set a new local output path:

```sh
set -e
release_output="$HOME/review-kit-0.1.0-build"
bash scripts/build-linux.sh "$release_output"
```

Use your workstation's task storage directory when it has one.
Build output and the container's Cargo cache stay in the checkout's `target/linux-release/` directory.
The output directory contains the expanded package, its archive and `SHA256SUMS`.
If assembly fails, retain that directory for diagnosis and retry with a new directory.
The builder performs no upload, tag creation or release publication.

For a native build, `python3 scripts/build-candidate.py --output NEW_DIRECTORY`
uses the host toolchain and libraries. It requires Python 3.9 or later, Rust 1.98.1
with `rust-docs`, a C compiler, CMake, Make and GNU binutils.
It rejects the environment overrides listed in [the builder](../scripts/build-candidate.py).
Other native compiler and Cargo settings remain the caller's responsibility.
A native build can require a newer glibc than the release binary.

The script uses this build command:

```sh
cargo +1.98.1 build --release --locked --package review-kit-cli --bin rkc --target x86_64-unknown-linux-gnu
```

`BUILD.json` binds the executable to the exact source commit, source tree, lockfile, target and compiler.
`SOURCE.tar.gz` retains that commit's source.
The expanded package also contains tracked source files so documentation links resolve offline.
Internal file aliases become ordinary files in the expanded package; the source archive preserves their original form.
`FILES.json` records package file checksums; it excludes itself.
The outer `SHA256SUMS` records the final archive checksum.
These checksums detect changed bytes; they do not establish the publisher's identity.

The script retains dependency license texts and notices under `licenses/`.
`THIRD-PARTY.json` records dependency versions, license declarations and notice checksums.
Its dependency graph includes build dependencies.
Pinned upstream license files cover dependencies whose registry packages omit them.
The package also retains the Rust standard library's copyright and license notices.

Assembly normalizes archive ordering, ownership and timestamps.
Repeat builds require the same source commit and build environment.
Compare resulting archive checksums before claiming that a build is reproducible.

## Download and install

Download the archive and its checksum file into a fresh directory:

```sh
set -e
release_url=https://github.com/sformisano/review-kit/releases/download/v0.1.0
curl --fail --location --remote-name "$release_url/review-kit-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
curl --fail --location --remote-name "$release_url/SHA256SUMS"
```

Verify the archive and its extracted files. These commands require Python 3.9 or later:

```sh
set -e
sha256sum --check SHA256SUMS
tar -xzf review-kit-0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd review-kit-0.1.0-x86_64-unknown-linux-gnu
python3 - <<'PY'
import hashlib, json
from pathlib import Path
for name, expected in json.loads(Path("FILES.json").read_text()).items():
    actual = hashlib.sha256(Path(name).read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"checksum mismatch: {name}")
print("package files verified")
PY
./rkc --version
./rkc --help
```

The version command returns `rkc 0.1.0`.
Read `BUILD.json` for the [build identity](#build-identity).
Read [the release notes](releases/0.1.0.md) before installation.
The executable embeds rendering schemas and templates.
Runtime operation requires no source checkout, separate json2md executable or Rust installation.

Install the complete package under a directory named for its version and source commit:

```sh
set -e
release_commit=$(python3 -c 'import json; print(json.load(open("BUILD.json"))["source_commit"])')
release_home="$HOME/.local/opt/review-kit/0.1.0-$release_commit"
mkdir -p "$HOME/.local/opt/review-kit" "$HOME/.local/bin"
test ! -e "$release_home"
cp -a . "$release_home"
ln -s "$release_home/rkc" "$HOME/.local/bin/rkc.next"
mv -Tf "$HOME/.local/bin/rkc.next" "$HOME/.local/bin/rkc"
```

Stop if an installation command fails.
Keep `$HOME/.local/bin` on `PATH`.
Run `rkc --version` from another directory to check the selected executable.
Exercise the commands in the verified installed package’s README against a disposable local GitLab merge request.
That package’s CLI guide defines its commands.
Keep provider writes within the permissions explicitly granted for that test.

## Upgrade and rollback

Stop Review Kit commands before switching executables.
Record the current executable path with `readlink "$HOME/.local/bin/rkc"`.
Retain that package and its checksums.
Finish any unfinished publication with the same executable and profile files before upgrading.

Back up trusted configuration, review runs and the shared `publication_state` directory together.
Each worktree keeps its review runs in `.review-kit/` at its root.
Retain the original assessment and its declared policy names for unfinished publication.
The [CLI configuration](../crates/bin/cli/README.md#trusted-configuration) defines their paths.
The shared directory can contain unresolved provider write intent from another review run or worktree.
Changing or deleting it cannot settle an unknown provider write.

Verify the new archive before switching the symlink.
Check custom rendering profiles against the new package's [renderer contract](../crates/lib/infrastructure/json2md-adapter/README.md) before publication.
Exercise the offline commands defined by that verified package's CLI guide with copies of its retained review files.
Exercise provider behavior against a disposable local GitLab merge request.
Keep configuration paths and the shared progress directory unchanged when installing an update.
The [state compatibility contract](render-profiles.md#retained-state-compatibility) defines supported formats and recovery requirements.
Executable versions do not replace document versions.

To roll back, set `previous_rkc` to the recorded executable path:

```sh
set -e
previous_rkc="$HOME/.local/opt/review-kit/RETAINED-VERSION-AND-COMMIT/rkc"
test -x "$previous_rkc"
ln -s "$previous_rkc" "$HOME/.local/bin/rkc.next"
mv -Tf "$HOME/.local/bin/rkc.next" "$HOME/.local/bin/rkc"
rkc --version
```

Stop if a rollback command fails.
Rollback replaces only the executable selection.
Retain current workspaces and shared publication progress, including writes attempted after the upgrade.
Restoring an older progress backup after a provider write can erase unresolved intent and permit a duplicate write.
Use a previous executable only after testing it against copies of the current documents.
If it cannot read current progress, retain the current executable for recovery and stop publication.
rkc 0.1.1 and later write `review-conversation.json` version 2, which rkc 0.1.0 cannot read;
the [captured conversation reference](../crates/bin/cli/README.md#captured-conversation) lists the symptoms,
including when 0.1.0 blocks other runs.
Before rolling back to 0.1.0, finish or stop every run that the newer executable started.
Keep the newer executable for any such run that owns unfinished publication and finish that publication with it first.
An executable without review runs cannot read the run publication journal, as the
[state compatibility contract](render-profiles.md#retained-state-compatibility) describes.
The [publication recovery reference](../crates/bin/cli/README.md#publication-and-recovery) defines recovery decisions.

## Distribution gates

The following checks gate distribution:

1. Run the [workspace checks](../README.md#workspace-checks) against the exact release source commit.
2. Run the pinned [GitLab CE suite](../tests/gitlab/README.md) and [CLI suite](../tests/cli/README.md).
3. Build the archive and verify its final extracted bytes outside the checkout.
4. Test installation, upgrade and rollback against supported documents and retained progress.
5. Review project licenses, third-party notices and [release limitations](releases/0.1.0.md).

Unknown or failing checks block distribution. An unavailable live server check remains a proof gap.
GitHub provider support and other operating systems remain outside this release's scope.

Use the repository URL from workspace package metadata as the hosting identity.
Point `v0.1.0` at the archive's exact source commit.
Upload the verified archive and its matching `SHA256SUMS` together.
Download both assets through the public release URLs without authentication.
Repeat checksum, extraction, executable and fixture checks against those downloaded bytes.
Record the resulting verification before declaring distribution complete.

If the source changes, repeat source checks, archive assembly and consumer verification.
If package bytes change, repeat archive checks and downstream consumer checks.
If uploaded bytes change, repeat the consumer download checks.
