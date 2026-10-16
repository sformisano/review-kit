# Agent skills

The repository declares its authoring skills in [.skillcatalog/skillcatalog.yml](../.skillcatalog/skillcatalog.yml).
SkillCatalog installs them from the existing `ai-skills` catalog.

## Installation

Run these commands from the checkout where the agent works:

```sh
skc install
skc validate --manifest
skc validate --drift
```

Installation requires SkillCatalog and access to the configured catalog.
Enable the delivery target for the agent before installation.
For Codex, use `skc settings enable codex`.
For Claude Code, use `skc settings enable claude-code`.

Codex reads installed skills from `.agents/skills/`.
Claude Code reads installed skills from `.claude/skills/`.
Generated skill files are ignored by Git; the tracked manifest defines the selection.
Do not hand-edit delivered skills.

SkillCatalog binds each project profile ID to one checkout on a machine.
For a concurrent checkout, create a distinct personal profile with `skc profile create`.
Use that checkout's directory and the same manifest entries.

Before changing source, read the installed skills that apply to the change.
If installation fails, resolve the missing catalog or delivery failure before coding.
Installing skills does not authorize implementation beyond the [feature contract](features.md).

## Required Rust guidance

The links below resolve after installation for Codex.
Claude Code receives the same skills under its own delivery directory.

| Concern | Skills to apply |
| --- | --- |
| Small, focused files and clear responsibility boundaries | [Code Structure Policy](../.agents/skills/code-structure-policy/SKILL.md) |
| Typed failures and recovery decisions without matching error messages | [Rust Error Contracts Policy](../.agents/skills/rust-error-contracts-policy/SKILL.md) |
| Preserved causes, useful context, and one reporting owner | [Rust Error Reporting Policy](../.agents/skills/rust-error-reporting-policy/SKILL.md) |
| Enums for controlled alternatives and valid state combinations | [Rust Data Modeling Policy](../.agents/skills/rust-data-modeling-policy/SKILL.md) |
| Newtypes and existing semantic types for distinct identities and units | [Rust Semantic Types Policy](../.agents/skills/rust-semantic-types-policy/SKILL.md) |
| Validated construction, conversion, deserialization, and mutation | [Rust Invariant Preservation Policy](../.agents/skills/rust-invariant-preservation-policy/SKILL.md) |
| Ownership, borrowing, and deliberate abstraction choices | [Rust Ownership Design Policy](../.agents/skills/rust-ownership-design-policy/SKILL.md), [Rust Abstraction Selection Policy](../.agents/skills/rust-abstraction-selection-policy/SKILL.md) |
| Blocking work, shared resources, and task cleanup | [Rust Async Work Placement Policy](../.agents/skills/rust-async-work-placement-policy/SKILL.md), [Rust Shared Resource Design Policy](../.agents/skills/rust-shared-resource-design-policy/SKILL.md), [Rust Task Lifecycle Policy](../.agents/skills/rust-task-lifecycle-policy/SKILL.md) |
| Rust contracts and supported consumers | [Rust Conventions Policy](../.agents/skills/rust-conventions-policy/SKILL.md) |
| Behavior tests and reusable test harnesses | [Rust Test Design Policy](../.agents/skills/rust-test-design-policy/SKILL.md), [Rust Test Harness Design Policy](../.agents/skills/rust-test-harness-design-policy/SKILL.md), [Risk Based Testing](../.agents/skills/risk-based-testing/SKILL.md) |
| Comments, proof, and completion claims | [Code Comment Authoring](../.agents/skills/code-comment-authoring/SKILL.md), [Baseline Proof Policy](../.agents/skills/baseline-proof-policy/SKILL.md), [Completion Verification](../.agents/skills/completion-verification/SKILL.md) |

[Policy Contract](../.agents/skills/policy-contract/SKILL.md) supplies the shared contract referenced by the engineering policies.

The skills guide agent decisions. They do not prove that future code follows the rules.
Implementation checks must verify the approved behavior and applicable engineering contracts.
