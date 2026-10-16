#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "e2e/assessments.rs"]
mod assessments;
#[path = "e2e/controls.rs"]
mod controls;
#[path = "e2e/fixture.rs"]
mod fixture;
#[path = "e2e/scenarios.rs"]
mod scenarios;
#[path = "../../../../tests/gitlab/support/service.rs"]
mod service;
#[path = "e2e/source.rs"]
mod source;
#[path = "e2e/workflow.rs"]
mod workflow;

#[test]
#[ignore = "creates disposable GitLab resources; needs Docker or an external local server"]
fn managed_cli_workflow() -> anyhow::Result<()> {
    let service = service::GitLabService::start().map_err(anyhow::Error::from_boxed)?;
    scenarios::outdated_position_followup(fixture::Fixture::setup_with(
        service.endpoint(),
        service.admin_token(),
    )?)?;
    scenarios::initial_fixed_and_recurrent_review(fixture::Fixture::setup_with(
        service.endpoint(),
        service.admin_token(),
    )?)?;
    scenarios::clean_blocked_permissions_and_controls(fixture::Fixture::setup_with(
        service.endpoint(),
        service.admin_token(),
    )?)?;
    scenarios::offline_invalid_inputs_and_private_bodies(fixture::Fixture::setup_with(
        service.endpoint(),
        service.admin_token(),
    )?)?;
    scenarios::custom_profile_review(fixture::Fixture::setup_with(
        service.endpoint(),
        service.admin_token(),
    )?)
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn external_outdated_position_followup() -> anyhow::Result<()> {
    scenarios::outdated_position_followup(fixture::Fixture::setup()?)
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn external_initial_fixed_and_recurrent_review() -> anyhow::Result<()> {
    scenarios::initial_fixed_and_recurrent_review(fixture::Fixture::setup()?)
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn external_clean_blocked_permissions_and_controls() -> anyhow::Result<()> {
    scenarios::clean_blocked_permissions_and_controls(fixture::Fixture::setup()?)
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn external_offline_invalid_inputs_and_private_bodies() -> anyhow::Result<()> {
    scenarios::offline_invalid_inputs_and_private_bodies(fixture::Fixture::setup()?)
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn external_custom_profile_review() -> anyhow::Result<()> {
    scenarios::custom_profile_review(fixture::Fixture::setup()?)
}
