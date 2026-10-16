#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(dead_code, unused_imports)]
mod support;

use support as server;

const TOKEN: &str = "fixture-private-secret";

#[path = "render_profiles/fixture.rs"]
#[allow(dead_code)]
mod fixture;

use fixture::{Fixture, assert_no_writes, write_json};
use serde_json::json;

#[test]
fn live_plan_rejects_a_question_edited_after_retained_checking() {
    rejects_edited_question(true);
}

#[test]
fn first_publication_rejects_a_question_edited_after_retained_checking() {
    rejects_edited_question(false);
}

#[test]
fn completed_answers_replay_after_question_edits_or_deletion_without_writes() {
    for delete in [false, true] {
        let fixture = Fixture::new();
        let initial = fixture.initial("initial", None);
        fixture.publish(&initial);
        let question = fixture.http.state.lock().unwrap().question(
            "discussion-1",
            "Does the regression cover the failure branch?",
        );
        let (path, mut value) = fixture.followup("answer", "open");
        value["answers"] = json!([{"finding":1,"in_reply_to":question.to_string(),"body":"The regression covers the failure branch."}]);
        write_json(&path, &value);
        fixture.check(&path);
        let first = fixture.publish(&path);
        assert_eq!(first["run"]["outcome"]["result"]["status"], "completed");
        let before = fixture.writes();
        {
            let mut state = fixture.http.state.lock().unwrap();
            let notes = state.discussions[0]["notes"].as_array_mut().unwrap();
            let index = notes
                .iter()
                .position(|note| note["id"] == question)
                .unwrap();
            if delete {
                notes.remove(index);
            } else {
                notes[index]["body"] = json!("Does the regression also cover cancellation?");
            }
        }
        let plan = fixture.ok(&["publish", "--effects", "review", "--plan"]);
        assert_eq!(plan["status"], "already_settled");
        let replay = fixture.publish(&path);
        assert_eq!(replay["status"], "already_settled");
        assert_eq!(fixture.writes(), before);
    }
}

fn rejects_edited_question(plan: bool) {
    let fixture = Fixture::new();
    let initial = fixture.initial("initial", None);
    fixture.publish(&initial);
    let question = fixture.http.state.lock().unwrap().question(
        "discussion-1",
        "Does the regression cover the failure branch?",
    );
    let (path, mut value) = fixture.followup("answer", "open");
    value["answers"] = json!([{
        "finding":1,"in_reply_to":question.to_string(),
        "body":"The regression covers the failure branch."
    }]);
    write_json(&path, &value);
    fixture.check(&path);
    fixture.accept(&path);
    let before = fixture.writes();
    {
        let mut state = fixture.http.state.lock().unwrap();
        let target = state.discussions[0]["notes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|note| note["id"] == question)
            .unwrap();
        target["body"] = json!("Does the regression also cover cancellation?");
    }
    let mut args = vec!["publish", "--effects", "review"];
    if plan {
        args.push("--plan");
    }
    let (output, result) = fixture.run(&args);
    assert_no_writes(&fixture, before, &output, &result);
    assert_eq!(output.status.code(), Some(3), "{result}");
    assert_eq!(
        result["diagnostics"][0]["code"], "conversation_changed",
        "{result}"
    );
    assert!(
        result["diagnostics"][0]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["kind"] == "note_body_changed"),
        "{result}"
    );

    let (fresh, mut value) = fixture.followup("reassessed-answer", "open");
    value["answers"] = json!([{
        "finding":1,"in_reply_to":question.to_string(),
        "body":"Cancellation remains outside this regression."
    }]);
    write_json(&fresh, &value);
    fixture.check(&fresh);
    let published = fixture.publish(&fresh);
    assert_eq!(published["run"]["outcome"]["result"]["status"], "completed");
    let state = fixture.http.state.lock().unwrap();
    let replies = state.discussions[0]["notes"].as_array().unwrap();
    assert_eq!(
        replies
            .iter()
            .filter(|note| note["body"]
                .as_str()
                .unwrap()
                .contains("Cancellation remains outside this regression."))
            .count(),
        1
    );
    assert!(replies.iter().all(|note| {
        !note["body"]
            .as_str()
            .unwrap()
            .contains("The regression covers the failure branch.")
    }));
}
