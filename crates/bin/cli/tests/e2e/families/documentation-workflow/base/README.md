# Release Handbook

This repository holds the `version-info` crate, its release procedure and the
agent skill that walks through that procedure.

- [docs/release.md](docs/release.md) is the authoritative release procedure.
- [skills/release-check/SKILL.md](skills/release-check/SKILL.md) guides an
  agent through the same procedure and must agree with it.
- `scripts/verify.sh` is the only supported verification entry point. Run
  `./scripts/verify.sh --help` to list its options.

The verification script needs Rust and Cargo. It uses no network.
