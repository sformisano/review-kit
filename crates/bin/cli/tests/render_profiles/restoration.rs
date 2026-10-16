//! Refusals of a changed profile for unfinished work name the way back
//! to the exact profile that rendered the round.

use std::{fs, path::Path};

use serde_json::Value;

use super::{
    MR_PATH,
    fixture::{
        Fixture, accepted_inputs, assert_accepted_inputs_unchanged, assert_no_writes, read_json,
    },
    server::Fault,
};

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
