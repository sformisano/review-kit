# Documentation

Start with the [CLI overview](../README.md#cli-usage) for commands, inputs and outputs.
Use the [CLI reference](../crates/bin/cli/README.md#review-runs) for review runs, configuration and document details.

To learn the model, start with the [local teaching page](learn/index.html). Open it in a browser;
it works offline without installing dependencies. Follow F1 through its values,
identity, policy judgments, assessment, and publication. Each lesson links to
the source to read next.

[Follow one review](guide.md) is the text walkthrough.

## Read in this order

1. [Local teaching page](learn/index.html): start with the domain, then follow the application flow.
2. [Feature contract](features.md): read the product requirements, supported effects, and limits.
3. [Architecture](architecture.md): see where those rules live and how the components work together.

The guide is enough for a first overview. Use the references below when a
specific task needs them.

For a domain-first reading path, start with the
[domain model and source reading order](../crates/lib/domain/README.md#how-the-model-fits-together).
Continue with the [application use cases](../crates/lib/application/README.md), then
the [GitLab implementation](../crates/lib/infrastructure/gitlab-adapter/README.md).

## Find an answer

| Question | Read |
| --- | --- |
| How do I collect logs or enable them for library calls? | [Operation logging](../crates/lib/observation/README.md) |
| What commands does Review Kit provide? | [CLI overview](../README.md#cli-usage) |
| How do I install the CLI from source? | [Installation](../README.md#install-and-run) |
| How do I configure the CLI? | [Configuration](../crates/bin/cli/README.md#trusted-configuration) |
| Where do review policies come from? | [Combined policy assessment](../crates/bin/cli/README.md#assessment-editing) |
| Where does a review run keep its files? | [Runtime files](../crates/bin/cli/README.md#runtime-files) |
| What does checkpoint acceptance prove? | [Assessment acceptance](../crates/bin/cli/README.md#assessment-acceptance) |
| Where does the CLI write logs, and how do I change that? | [Logging preferences](../crates/bin/cli/README.md#logging-preferences) |
| How do I see progress during long commands? | [Progress output](../crates/bin/cli/README.md#progress-output) |
| How do I resume an interrupted publication? | [Publication and recovery](../crates/bin/cli/README.md#publication-and-recovery) |
| Can a new container publish a follow-up review without earlier state? | [Follow-up reviews in a new environment](../crates/bin/cli/README.md#follow-up-reviews-in-a-new-environment) |
| How do I change comment formatting without rebuilding? | [Rendering profiles](render-profiles.md) |
| How do I download, verify, install, upgrade or roll back a binary? | [Binary releases](distribution.md) |
| What changed in 0.1.2, and how do I upgrade from 0.1.1? | [0.1.2 release notes](releases/0.1.2.md) |
| What changed in 0.1.1, and how do I upgrade from 0.1.0? | [0.1.1 release notes](releases/0.1.1.md) |
| What does 0.1.0 include? | [0.1.0 release notes](releases/0.1.0.md) |
| What is implemented, and where does verification stop? | [Implementation status](plan.md) |
| Which scope and architecture choices govern the implementation? | [Decisions](decisions.md) |
| What belongs to the agent, and how do commands divide the work? | [Review workflow](workflow.md) |
| How do entities and value types form the domain model? | [Domain model](../crates/lib/domain/README.md#how-the-model-fits-together) |
| How do checking, publication, and recovery work in Rust? | [Application crate](../crates/lib/application/README.md) |
| How do I configure GitLab, and what does the adapter support? | [GitLab crate](../crates/lib/infrastructure/gitlab-adapter/README.md) |
| How are local JSON, files and rendering implemented? | [JSON](../crates/lib/infrastructure/json-adapter/README.md), [filesystem](../crates/lib/infrastructure/filesystem-adapter/README.md), [json2md](../crates/lib/infrastructure/json2md-adapter/README.md) |
| Which GitLab endpoint implements each operation? | [GitLab operation mapping](gitlab-adapter.md) |
| What setup does an agent need before coding? | [Agent skills](agent-skills.md) |
| How do I run the workspace checks? | [Workspace checks](../README.md#workspace-checks) |
| How do I test the installed CLI against GitLab? | [CLI E2E suite](../tests/cli/README.md#repeatable-cli-suite) |

## Where facts belong

The project README describes the tool's commands, inputs and outputs. The guide explains finding
identity and publication through one example. The feature contract states product requirements.
Crate READMEs define implemented Rust contracts and their tests.
The implementation status page owns component status and verification boundaries.
The decisions page records accepted scope and architecture choices.

When a fact changes, update its home and link to it from other documents.
