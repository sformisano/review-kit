# Prepare and validate an assessment

An assessment records the agent's review judgment for one subject, reviewer,
and revision. This use case turns retained review context into a draft, requires
fresh judgments, and checks whether active findings can be rendered and anchored.

Use it when starting a review or reassessing findings after a follow-up change.
The agent supplies the review decision. Completion validates that decision and
the supporting records; it does not infer readiness from publication status.

## Inputs and results

- `prepare` takes a validated discovery snapshot and agent-declared `AssessmentPolicies`.
  It returns an `AssessmentTemplate` containing unassessed policy and finding slots.
- Each carried finding includes its last verified communicated judgment.
  Its prior status and validation flag do not satisfy the fresh reassessment requirement.
- `complete` takes the filled template and new findings. It returns an `Assessment`
  or collects missing inputs and independent consistency failures as `DraftIssue` values.
- `check` takes that assessment, the declared coverage, committed-source access,
  and a renderer. It returns checked bodies for open findings or independent `CheckIssue` values.

The template also requires decision, depth, and scope. A blocked assessment
requires an overall limitation under the domain rules. `local_context` retains
private notes outside the completed assessment and renderer inputs.

## Example

A reviewer returns to a finding about an unsafe query. `prepare` carries the
last communicated open finding but leaves its reassessment empty. The agent
records a fixed judgment, evaluates each selected policy, and supplies the
review decision. `complete` validates those inputs together with any new findings.

## Boundaries

Preparation and completion perform no adapter calls. They reject conflicting
retained context or incomplete judgments without creating provider content.
`check` reads exact committed source, validates diff positions, and renders open
findings through ports. A source adapter can use network access.

Checking preserves whitespace when comparing suggestion originals. It rejects
blank bodies and reserved review markers. It does not check summary history,
answer targets, or publication authority; `publishing::check_review` checks the
complete publication request. A completed assessment still needs publication
checks and explicit transition explanations before communicating changed findings.
