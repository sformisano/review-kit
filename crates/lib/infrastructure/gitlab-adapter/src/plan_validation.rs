use crate::observation;
use review_kit_application::publication::Plan;
use review_kit_observation::Fields;

use crate::{GitLabError, body, wire};

/// Check rendered plan bodies against GitLab's publication constraints without I/O.
/// This check does not establish revision currentness or publication authority.
#[review_kit_observation::operations]
#[operation(
    name = "gitlab.validate_plan_bodies",
    input = Fields::new().count("actions", plan.actions.len()),
    output = |result| observation::result(result, |_| Fields::new()),
)]
pub fn validate_plan_bodies(plan: &Plan) -> Result<(), GitLabError> {
    for summary in [&plan.summary_body, &plan.terminal_summary_preview] {
        wire::validate_body(summary)?;
        body::validate_no_suggestions(summary)?;
    }
    wire::validate_actions(&plan.round, &plan.history)?;
    for action in &plan.actions {
        wire::validate_action(action)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "plan_validation/tests.rs"]
mod tests;
