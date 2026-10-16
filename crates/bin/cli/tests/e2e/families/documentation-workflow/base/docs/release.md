# Release procedure

1. Run `./scripts/verify.sh --complete` on the release commit. Continue only
   when it exits with status 0.
2. Add the release notes to `CHANGELOG.md` and commit them.
3. Create the annotated tag `vMAJOR.MINOR.PATCH` on that commit.
4. Push the tag with `git push origin vMAJOR.MINOR.PATCH`.

Never create or push a release tag before step 1 succeeds on the commit that
the tag names. A pushed tag starts the publishing pipeline and cannot be
withdrawn from consumers who already fetched it.
