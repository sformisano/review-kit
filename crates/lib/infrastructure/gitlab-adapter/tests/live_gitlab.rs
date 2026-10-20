#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "live/conversation.rs"]
mod conversation;
#[path = "live/fixture.rs"]
mod fixture;
#[path = "live/model.rs"]
mod model;
#[path = "../../../../../tests/gitlab/support/service.rs"]
mod service;
#[path = "live/state_preservation.rs"]
mod state_preservation;
#[path = "live/workflows.rs"]
mod workflows;

#[test]
#[ignore = "creates a disposable GitLab container; needs Docker or an external local server"]
fn managed_gitlab_suite() {
    let service = service::GitLabService::start().unwrap();
    workflows::initial_and_clean_reviews_survive_restart_and_unchanged_rerun(&service);
    workflows::single_multiline_and_deletion_suggestions_match_server_content(&service);
    workflows::author_answer_followup_resolution_and_reopening_preserve_roots(&service);
    workflows::stale_revision_and_read_only_token_stop_without_provider_effects(&service);
    workflows::fork_source_and_paginated_notes_use_the_correct_project(&service);
    workflows::modified_binary_source_remains_explicit_in_retained_capture(&service);
    conversation::conversation_capture_matches_independent_rest_reads(&service);
    conversation::write_target_scope_follows_the_plan(&service);
}

fn external_service() -> service::GitLabService {
    assert!(
        std::env::var_os("REVIEW_KIT_GITLAB_URL").is_some()
            && std::env::var_os("REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE").is_some(),
        "individual scenarios require both external GitLab variables; use managed_gitlab_suite for a disposable server"
    );
    service::GitLabService::start().unwrap()
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn initial_and_clean_reviews_survive_restart_and_unchanged_rerun() {
    workflows::initial_and_clean_reviews_survive_restart_and_unchanged_rerun(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn single_multiline_and_deletion_suggestions_match_server_content() {
    workflows::single_multiline_and_deletion_suggestions_match_server_content(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn author_answer_followup_resolution_and_reopening_preserve_roots() {
    workflows::author_answer_followup_resolution_and_reopening_preserve_roots(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn stale_revision_and_read_only_token_stop_without_provider_effects() {
    workflows::stale_revision_and_read_only_token_stop_without_provider_effects(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn fork_source_and_paginated_notes_use_the_correct_project() {
    workflows::fork_source_and_paginated_notes_use_the_correct_project(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn modified_binary_source_remains_explicit_in_retained_capture() {
    workflows::modified_binary_source_remains_explicit_in_retained_capture(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn conversation_capture_matches_independent_rest_reads() {
    conversation::conversation_capture_matches_independent_rest_reads(&external_service());
}

#[test]
#[ignore = "requires an external local GitLab server and administrator credential"]
fn write_target_scope_follows_the_plan() {
    conversation::write_target_scope_follows_the_plan(&external_service());
}

#[test]
#[ignore = "requires an external GitLab server, private evidence paths and later browser inspection"]
fn retained_state_preservation_and_server_rejection() {
    state_preservation::run(&external_service());
}
