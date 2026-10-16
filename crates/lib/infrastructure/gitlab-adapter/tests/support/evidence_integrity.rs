use review_kit_application::{
    publication::Effects,
    use_cases::{
        discovery_bundle::discover_bundle,
        publishing::{PublicationStatus, publish},
    },
};
use review_kit_domain::{Assessment, AssessmentDraft, FindingStatus};
use serde_json::Value;

use super::support::{Fixture, PublicationProgress, Renderer, finding, request};

#[test]
fn completed_creation_assessment_cannot_override_its_verified_open_root() {
    reject_uncorroborated_completed_assessment(false);
}

#[test]
fn completed_carried_assessment_cannot_close_a_root_without_a_verified_transition() {
    reject_uncorroborated_completed_assessment(true);
}

fn reject_uncorroborated_completed_assessment(carried_round: bool) {
    let fixture = Fixture::new();
    let client = fixture.connect();
    let mut progress = PublicationProgress::default();
    let mut host = client.publication_host(Effects::review());
    let mut communicated = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Open, None)],
    );
    assert_eq!(
        publish(
            &communicated,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .unwrap()
        .status,
        PublicationStatus::Completed,
    );
    if carried_round {
        let old = &communicated.assessment;
        communicated.assessment = Assessment::new(AssessmentDraft {
            subject: old.subject().clone(),
            reviewer: old.reviewer().clone(),
            revision: old.revision().clone(),
            decision: old.decision(),
            depth: old.depth().into(),
            scope: "Request handling and callers".into(),
            limitation: old.limitation().map(str::to_owned),
            selected_policies: old.selected_policies().to_vec(),
            evaluations: old.evaluations().to_vec(),
            findings: old.findings().to_vec(),
        })
        .unwrap();
        assert_eq!(
            publish(
                &communicated,
                &mut host,
                &mut client.clone(),
                &mut Renderer::default(),
                &mut progress,
            )
            .unwrap()
            .status,
            PublicationStatus::Completed,
        );
    }
    let valid = discover_bundle(
        &mut client.clone(),
        &mut client.clone(),
        client.subject(),
        client.reviewer(),
    )
    .unwrap();
    assert_eq!(valid.findings()[0].status(), FindingStatus::Open);
    assert_eq!(
        valid.snapshot().summaries[0].history.len(),
        usize::from(carried_round)
    );
    if carried_round {
        assert!(valid.snapshot().summaries[0].current.actions.is_empty());
    }
    let writes = {
        let mut state = fixture.state.lock().unwrap();
        let original_root = state.discussions[0]["notes"][0].clone();
        let body = state.notes[0]["body"].as_str().unwrap();
        let (prefix, remainder) = body.split_once("\n\n<!-- review-kit:state:v3 ").unwrap();
        let (payload, suffix) = remainder.split_once(" -->\n\n<!-- review-kit:v1 ").unwrap();
        let mut retained: Value = serde_json::from_str(payload).unwrap();
        let original_actions = expanded_actions(&retained);
        let finding_index = retained["current"]["assessment"]["findings"][0]
            .as_u64()
            .unwrap() as usize;
        let mut fixed = retained["findings"][finding_index].clone();
        fixed["status"] = "fixed".into();
        retained["findings"]
            .as_array_mut()
            .unwrap()
            .insert(0, fixed);
        let mut history = retained["history"].as_array().unwrap().clone();
        history.push(retained["current"].clone());
        for round in &mut history {
            for finding in round["assessment"]["findings"].as_array_mut().unwrap() {
                *finding = (finding.as_u64().unwrap() + 1).into();
            }
            shift_actions(&mut round["actions"]);
            if let Some(rows) = round["communicated_findings"].as_array_mut() {
                for row in rows {
                    row["finding"] = (row["finding"].as_u64().unwrap() + 1).into();
                }
            }
            if round["abandonment"].is_object() {
                shift_actions(&mut round["abandonment"]["verified_actions"]);
            }
        }
        retained["current"] = history.pop().unwrap();
        retained["history"] = history.into();
        retained["current"]["assessment"]["decision"] = "ready".into();
        retained["current"]["assessment"]["findings"][0] = 0.into();
        assert_eq!(expanded_actions(&retained), original_actions);
        state.notes[0]["body"] = format!(
            "{prefix}\n\n<!-- review-kit:state:v3 {} -->\n\n<!-- review-kit:v1 {suffix}",
            serde_json::to_string(&retained).unwrap(),
        )
        .into();
        assert_eq!(state.discussions[0]["notes"][0], original_root);
        state.writes().len()
    };
    let mut forged = request(
        &client,
        vec![finding(1, "src/lib.rs", 2, FindingStatus::Fixed, None)],
    );
    let old = &forged.assessment;
    forged.assessment = Assessment::new(AssessmentDraft {
        subject: old.subject().clone(),
        reviewer: old.reviewer().clone(),
        revision: old.revision().clone(),
        decision: old.decision(),
        depth: old.depth().into(),
        scope: communicated.assessment.scope().into(),
        limitation: old.limitation().map(str::to_owned),
        selected_policies: old.selected_policies().to_vec(),
        evaluations: old.evaluations().to_vec(),
        findings: old.findings().to_vec(),
    })
    .unwrap();
    assert!(
        discover_bundle(
            &mut client.clone(),
            &mut client.clone(),
            client.subject(),
            client.reviewer(),
        )
        .is_err(),
    );
    assert!(
        publish(
            &forged,
            &mut host,
            &mut client.clone(),
            &mut Renderer::default(),
            &mut progress,
        )
        .is_err(),
    );
    assert_eq!(fixture.state.lock().unwrap().writes().len(), writes);
}

fn shift_actions(actions: &mut Value) {
    for action in actions.as_array_mut().unwrap() {
        if action["kind"] == "finding" {
            action["finding"] = (action["finding"].as_u64().unwrap() + 1).into();
        }
    }
}

fn expanded_actions(state: &Value) -> Value {
    let mut actions = state["current"]["actions"].clone();
    for action in actions.as_array_mut().unwrap() {
        if action["kind"] == "finding" {
            let index = action["finding"].as_u64().unwrap() as usize;
            action["finding"] = state["findings"][index].clone();
        }
    }
    actions
}
