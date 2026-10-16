//! The effective effect set shown by planning and publication.

use serde_json::{Value, json};

use super::harness::{Harness, read_json, write_json};

const ALL: [&str; 6] = [
    "create_summary",
    "update_summary",
    "finding",
    "reply",
    "resolve",
    "reopen",
];

fn set_authority(harness: &Harness, authority: &[&str]) {
    let mut config = read_json(&harness.config);
    config["gitlab"]["authority"] = json!(authority);
    write_json(&harness.config, &config);
}

fn summary(start: &str, start_within: bool, final_within: bool) -> Value {
    json!([
        {"write": "start", "effect": start, "within": start_within},
        {"write": "final", "effect": "update_summary", "within": final_within},
    ])
}

fn finding(within: bool) -> Value {
    json!([{"index": 0, "finding": 1, "effect": "finding", "within": within}])
}

#[test]
fn a_plan_marks_the_planned_effects_outside_the_requested_set() {
    let harness = Harness::new();
    harness.accepted_with_finding();
    let review = harness.ok(&["publish", "--effects", "review", "--plan"]);
    let summary_only = harness.ok(&["publish", "--effects", "create_summary", "--plan"]);
    assert_eq!(
        review["run"]["outcome"]["plan"]["actions"],
        summary_only["run"]["outcome"]["plan"]["actions"],
        "the planned actions do not depend on the effect set"
    );
    assert_eq!(review["effects"]["requested"], json!(ALL), "{review:#}");
    assert_eq!(review["effects"]["configured_authority"], json!(ALL));
    assert_eq!(review["effects"]["provider_permissions"], json!(ALL));
    assert_eq!(review["effects"]["round_ceiling"], Value::Null);
    assert_eq!(review["effects"]["effective"], json!(ALL));
    assert_eq!(
        review["effects"]["summary_writes"],
        summary("create_summary", true, true)
    );
    assert_eq!(review["effects"]["actions"], finding(true));
    assert_eq!(review["effects"]["outside"], json!([]));

    let effects = &summary_only["effects"];
    assert_eq!(
        effects["requested"],
        json!(["create_summary"]),
        "{effects:#}"
    );
    assert_eq!(effects["effective"], json!(["create_summary"]));
    assert_eq!(
        effects["summary_writes"],
        summary("create_summary", true, false)
    );
    assert_eq!(effects["actions"], finding(false));
    assert_eq!(effects["outside"], json!(["update_summary", "finding"]));
    assert_eq!(harness.writes(), 0);

    let (output, published) = harness.rkc(&["publish", "--effects", "create_summary"]);
    assert_eq!(output.status.code(), Some(4), "{published:#}");
    assert_eq!(
        published["effects"]["outside"], effects["outside"],
        "{published:#}"
    );
    assert_eq!(harness.writes_to("POST", "/notes"), 1, "the summary starts");
    assert_eq!(harness.writes_to("POST", "/discussions"), 0, "no finding");

    // The round's ceiling is the request of its first write; a broader request
    // cannot widen it, and the started summary now needs an update.
    let recovery = harness.ok(&["publish", "--effects", "review", "--plan"]);
    let effects = &recovery["effects"];
    assert_eq!(recovery["recovery"], true, "{recovery:#}");
    assert_eq!(
        effects["round_ceiling"],
        json!({"round": 1, "effects": ["create_summary"], "retained": true}),
        "{effects:#}"
    );
    assert_eq!(effects["effective"], json!(["create_summary"]));
    assert_eq!(
        effects["summary_writes"],
        summary("update_summary", false, false)
    );
    assert_eq!(effects["outside"], json!(["update_summary", "finding"]));
}

#[test]
fn a_plan_marks_effects_outside_a_narrower_configured_authority_and_its_ceiling() {
    let harness = Harness::new();
    set_authority(&harness, &["create_summary", "update_summary"]);
    harness.accepted_with_finding();
    let planned = harness.ok(&["publish", "--effects", "review", "--plan"]);
    let effects = &planned["effects"];
    assert_eq!(effects["requested"], json!(ALL), "{effects:#}");
    assert_eq!(
        effects["configured_authority"],
        json!(["create_summary", "update_summary"])
    );
    assert_eq!(
        effects["effective"],
        json!(["create_summary", "update_summary"])
    );
    assert_eq!(
        effects["summary_writes"],
        summary("create_summary", true, true)
    );
    assert_eq!(effects["actions"], finding(false));
    assert_eq!(effects["outside"], json!(["finding"]));

    let (output, published) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(4), "{published:#}");
    assert_eq!(published["effects"]["outside"], json!(["finding"]));
    assert_eq!(
        published["effects"]["round_ceiling"],
        json!({"round": 1, "effects": ["create_summary", "update_summary"], "retained": true}),
        "{published:#}"
    );
    assert_eq!(harness.writes_to("POST", "/discussions"), 0, "no finding");

    // Broader configuration later cannot exceed the unfinished round's ceiling.
    set_authority(&harness, &ALL);
    let recovery = harness.ok(&["publish", "--effects", "review", "--plan"]);
    let effects = &recovery["effects"];
    assert_eq!(effects["configured_authority"], json!(ALL), "{effects:#}");
    assert_eq!(
        effects["round_ceiling"],
        json!({"round": 1, "effects": ["create_summary", "update_summary"], "retained": true})
    );
    assert_eq!(
        effects["effective"],
        json!(["create_summary", "update_summary"])
    );
    assert_eq!(
        effects["summary_writes"],
        summary("update_summary", true, true)
    );
    assert_eq!(effects["outside"], json!(["finding"]));

    // Without a retained ceiling, an unfinished provider round permits nothing.
    let journal = harness
        .journal_dir()
        .unwrap()
        .join("publication-progress.json");
    let mut progress = read_json(&journal);
    progress["authorization"] = Value::Null;
    write_json(&journal, &progress);
    let unknown = harness.ok(&["publish", "--effects", "review", "--plan"]);
    let effects = &unknown["effects"];
    assert_eq!(
        effects["round_ceiling"],
        json!({"round": 1, "effects": [], "retained": false}),
        "{effects:#}"
    );
    assert_eq!(effects["effective"], json!([]));
    assert_eq!(effects["outside"], json!(["update_summary", "finding"]));
    let writes = harness.writes();
    let (output, denied) = harness.rkc(&["publish", "--effects", "review"]);
    assert_eq!(output.status.code(), Some(4), "{denied:#}");
    assert_eq!(denied["effects"]["effective"], json!([]));
    assert_eq!(
        harness.writes(),
        writes,
        "publication permits nothing either"
    );
}
