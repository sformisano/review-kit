# Create a review draft

Initialization gives the agent a place to record a new assessment without
overwriting an existing draft. It binds the draft to discovered review context
and the coverage declared by the agent.

Use `initialize` after collecting a `DiscoveryBundle` and choosing applicable
policy IDs. The resulting draft contains prompts for fresh policy judgments,
finding reassessments, and assessment metadata.

## Inputs and result

The caller supplies the bundle, validated `AssessmentPolicies`, and an
`AssessmentDraftWriter`. The agent owns the policy IDs and display names.

`InitializedAssessment` contains the prepared `AssessmentTemplate`. Its
`policies` accessor exposes declared coverage and display names.
`InitializationResult` contains that prepared draft and the writer's outcome:

- `DraftCreation::Created` means the writer created the destination.
- `DraftCreation::Existing` means the destination already exists and remains unchanged.
  The returned draft describes the newly prepared input, not the existing destination's contents.

A discovery conflict prevents the writer call.
Writer failure returns `InitializationError::Write` without claiming creation.

## Example

An agent declares a source-safety policy for a follow-up review. Initialization
prepares that policy's judgment and prior findings for reassessment.
If the draft already exists, the writer preserves the agent's saved work even
when the declared coverage differs.

## Boundaries

The writer must create the destination atomically only when absent. The use
case relies on that adapter contract to prevent overwrites.
It performs no provider discovery or publication itself; the supplied writer
owns draft storage and any storage side effects.

Prior finding status does not complete a reassessment. Initialization does
not supply a review decision, complete the assessment, check source positions,
or authorize publication.
