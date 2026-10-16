use std::fs;

use super::fixture::{Fixture, read_json};

#[test]
fn toml_config_resolves_profile_and_default_or_configured_progress_relative_to_its_file() {
    for state in [None, Some("custom-progress")] {
        let fixture = Fixture::new();
        let profile = fixture.export("config/profile");
        profile.prefix("summary", "TOML CONFIGURED PROFILE");
        fixture.initial("review", None);
        let json = read_json(&fixture.config);
        let config = fixture.config.with_extension("toml");
        let state_field = state
            .map(|directory| format!("publication_state = {directory:?}\n"))
            .unwrap_or_default();
        fs::write(
            &config,
            format!(
                "version = 1\nrender_profile = \"profile/profile.json\"\nreview_workflow = \"codebase-policies-review\"\n{state_field}[gitlab]\nserver = {:?}\ntoken_env = \"RKC_PROFILE_TEST_TOKEN\"\nauthority = [\"create_summary\", \"update_summary\", \"finding\", \"reply\", \"resolve\", \"reopen\"]\n",
                json["gitlab"]["server"].as_str().unwrap()
            ),
        )
        .unwrap();
        let invocation = fixture.root.path().join("elsewhere");
        fs::create_dir(&invocation).unwrap();
        let (output, checked) = fixture.command(&config, &invocation, &["check"]);
        assert!(output.status.success(), "{checked}");
        assert!(
            checked["check"]["plan"]["summary_body"]
                .as_str()
                .unwrap()
                .starts_with("TOML CONFIGURED PROFILE\n")
        );
        let (output, accepted) = fixture.command(&config, &invocation, &["accept"]);
        assert!(output.status.success(), "{accepted}");
        assert_eq!(accepted["status"], "stored");
        let (output, published) =
            fixture.command(&config, &invocation, &["publish", "--effects", "review"]);
        assert!(output.status.success(), "{published}");
        assert_eq!(published["run"]["outcome"]["result"]["status"], "completed");
        let progress = std::path::Path::new(published["progress"].as_str().unwrap());
        assert!(
            progress.starts_with(
                config
                    .parent()
                    .unwrap()
                    .join(state.unwrap_or("publication-state"))
            )
        );
        assert!(progress.is_file());
        assert!(
            !invocation
                .join(state.unwrap_or("publication-state"))
                .exists()
        );
    }
}
