---
name: release-check
description: Walk through the version-info release procedure and stop at the first failed step.
---
# Release check

Use this checklist for every `version-info` release. Stop at the first step
that fails and report it.

## Prepare

1. Confirm that the working tree is clean: `git status --short` prints nothing.
2. Run the fast local check: `./scripts/verify.sh --full`.
3. Confirm that `CHANGELOG.md` has a section for the new version.

## Publish

4. Create the annotated tag: `git tag -a vMAJOR.MINOR.PATCH -m "vMAJOR.MINOR.PATCH"`.
5. Push the tag so that the pipeline starts: `git push origin vMAJOR.MINOR.PATCH`.
6. Run `./scripts/verify.sh --complete` on the tagged commit. If it fails,
   delete the tag locally and on the remote, then report the failure.
