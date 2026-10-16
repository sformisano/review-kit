---
name: release-check
description: Walk through the version-info release procedure and stop at the first failed step.
---
# Release check

Follow [the release procedure](../../docs/release.md) exactly.

1. Run `./scripts/verify.sh --complete` on the release commit and stop if it fails.
2. Confirm that `CHANGELOG.md` contains the release notes.
3. Create and push the release tag only after the first two steps succeed.
