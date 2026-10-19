//! Refusals of a changed profile for unfinished work name the way back
//! to the exact profile that rendered the round.

use std::{fs, path::Path};

use serde_json::{Value, json};

use super::{
    MR_PATH,
    fixture::{
        Fixture, accepted_inputs, assert_accepted_inputs_unchanged, assert_no_writes,
        public_summary, read_json, text, write_json,
    },
    server::Fault,
};

/// The fingerprint of the profile that rkc 0.1.0 embeds. A mismatch means another
/// default asset changed since 0.1.0; add its 0.1.0 copy to `export_previous_default`.
const PREVIOUS_DEFAULT_FINGERPRINT: &str =
    "sha256:d5d9e355e3d37feaf86b21b13ad3b7dd278d9e6bd2bf381218a0bf2e56d119ac";

/// The way back that a refusal for unfinished work names.
fn assert_restore_hint(next: &Value) {
    let next = next.as_str().unwrap_or_default();
    assert!(next.contains("exact profile"), "{next:?}");
    assert!(next.contains("--render-profile PATH"), "{next:?}");
    assert!(next.contains("render_profile in configuration"), "{next:?}");
    assert!(
        next.contains("Restore that profile's manifest and assets"),
        "{next:?}"
    );
    assert!(next.contains("until the round finishes"), "{next:?}");
}

#[test]
fn current_run_refusals_of_a_changed_profile_name_the_way_back_to_the_original() {
    let fixture = Fixture::lost_responses();
    let profile = fixture.export("original");
    fixture.select(&profile);
    let path = fixture.initial("review", None);
    fixture.accept(&path);
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let publish = ["publish", "--effects", "review"];
    let (output, first) = fixture.run(&publish);
    assert_eq!(output.status.code(), Some(6), "{first}");
    assert!(read_json(Path::new(first["progress"].as_str().unwrap()))["unresolved"].is_object());
    let progress = Path::new(first["progress"].as_str().unwrap());
    let retained = fs::read(progress).unwrap();
    let inputs = accepted_inputs(&path);
    let original_check = fixture.check(&path);
    profile.prefix("finding", "CHANGED PROFILE");
    let before = fixture.writes();
    let before_requests = fixture.http.state.lock().unwrap().requests.len();

    let checked = fixture.check(&path);
    assert_eq!(checked["status"], "valid", "{checked}");
    assert_ne!(checked["render_profile"], original_check["render_profile"]);
    assert_eq!(
        fixture.http.state.lock().unwrap().requests.len(),
        before_requests
    );
    assert_eq!(fixture.writes(), before);
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_accepted_inputs_unchanged(&inputs);

    let (output, refused) = fixture.run(&publish);
    assert_no_writes(&fixture, before, &output, &refused);
    assert_eq!(output.status.code(), Some(2), "{refused}");
    assert_eq!(refused["diagnostics"][0]["code"], "render_profile_changed");
    let next = &refused["diagnostics"][0]["next"];
    assert_restore_hint(next);

    let (output, planned) = fixture.run(&["publish", "--effects", "review", "--plan"]);
    assert_no_writes(&fixture, before, &output, &planned);
    assert_eq!(planned["diagnostics"][0]["code"], "render_profile_changed");
    assert_restore_hint(&planned["next"]);

    let (output, abandoned) =
        fixture.run(&["publish", "--abandon", "--reason", "The profile changed."]);
    assert_no_writes(&fixture, before, &output, &abandoned);
    assert_eq!(
        abandoned["abandon"]["diagnostics"][0]["code"],
        "render_profile_changed"
    );
    assert_restore_hint(&abandoned["next"]);
    assert_eq!(fs::read(progress).unwrap(), retained);
    assert_accepted_inputs_unchanged(&inputs);
}

#[test]
fn round_started_by_the_previous_default_finishes_with_its_restored_profile() {
    let fixture = Fixture::lost_responses();
    let previous = fixture.export_previous_default("previous");
    fixture.select(&previous);
    let path = fixture.initial("review", None);
    let accepted = fixture.accept(&path);
    assert_eq!(
        accepted["render_profile"],
        json!({
            "id": "review-kit-default",
            "input_contract": "review-kit-render-input/v2",
            "fingerprint": PREVIOUS_DEFAULT_FINGERPRINT,
        })
    );
    fixture
        .http
        .state
        .lock()
        .unwrap()
        .faults
        .push(Fault::lost(&format!("{MR_PATH}/discussions"), false));
    let publish = ["publish", "--effects", "review"];
    let (output, first) = fixture.run(&publish);
    assert_eq!(output.status.code(), Some(6), "{first}");

    // An upgraded executable renders with its own embedded default.
    let mut config = read_json(&fixture.config);
    config.as_object_mut().unwrap().remove("render_profile");
    write_json(&fixture.config, &config);
    let before = fixture.writes();
    let (output, refused) = fixture.run(&publish);
    assert_no_writes(&fixture, before, &output, &refused);
    assert_eq!(output.status.code(), Some(2), "{refused}");
    assert_eq!(refused["diagnostics"][0]["code"], "render_profile_changed");
    assert_restore_hint(&refused["diagnostics"][0]["next"]);

    {
        let mut state = fixture.http.state.lock().unwrap();
        let request = state
            .writes()
            .into_iter()
            .find(|request| request.path == format!("{MR_PATH}/discussions"))
            .unwrap()
            .clone();
        state.commit(&request);
    }
    let finished = fixture.ok(&[
        "--render-profile",
        text(&previous.manifest),
        "publish",
        "--effects",
        "review",
    ]);
    assert_eq!(
        finished["run"]["outcome"]["result"]["status"], "completed",
        "{finished}"
    );
    assert_eq!(finished["render_profile"], accepted["render_profile"]);
    let summary = fixture.summary();
    let body = public_summary(&summary);
    assert!(
        body.contains("<summary>Review details</summary>\n\n- Reviewed commit "),
        "{body}"
    );
    let state = fixture.http.state.lock().unwrap();
    assert_eq!(state.discussions.len(), 1);
    assert_eq!(
        state
            .writes()
            .iter()
            .filter(|request| request.method == "POST"
                && request.path == format!("{MR_PATH}/discussions"))
            .count(),
        1
    );
}
