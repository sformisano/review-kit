#![allow(clippy::unwrap_used)]

use review_kit_application::publication::{
    Abandonment, Plan, PlannedAction, ReplyKind, Round, RoundStatus,
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, ReviewerId, Revision,
    Severity, SourceLocation, SourcePath, Subject, Suggestion,
};

use super::validate_plan_bodies;
use crate::GitLabError;

fn finding(replacement: Option<Vec<String>>) -> Finding {
    let location = SourceLocation::new(
        SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
        LineNumber::try_from(2).unwrap(),
    );
    let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
    let suggestion = replacement.map(|replacement| {
        Suggestion::new(
            location.clone(),
            revision,
            LineNumber::try_from(1).unwrap(),
            vec!["one".into(), "two".into(), "three".into()],
            replacement,
            "Verified correction".into(),
        )
        .unwrap()
    });
    Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: PolicyId::try_from("contract".to_owned()).unwrap(),
        title: "Keep the value".into(),
        body: "The value is discarded.".into(),
        severity: Severity::P1,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Observed return path".into(),
        validated: true,
        location,
        suggestion,
    })
    .unwrap()
}

fn plan(finding: Finding, body: &str) -> Plan {
    let policy = finding.policy_id().clone();
    let assessment = Assessment::new(AssessmentDraft {
        subject: Subject::new(
            "gitlab".into(),
            "https://gitlab.example".into(),
            "group/repo".into(),
            7,
        )
        .unwrap(),
        reviewer: ReviewerId::try_from("42".to_owned()).unwrap(),
        revision: Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Checked the changed path".into(),
        scope: "src/lib.rs".into(),
        limitation: None,
        selected_policies: vec![policy.clone()],
        evaluations: vec![
            PolicyEvaluation::new(
                policy,
                PolicyJudgment::Violated {
                    evidence: "Observed discarded value".into(),
                },
            )
            .unwrap(),
        ],
        findings: vec![finding.clone()],
    })
    .unwrap();
    Plan {
        round: Round {
            confirmed_validation: false,
            review_workflow: None,
            policy_set: None,
            policy_names: vec![review_kit_application::publication::PolicyName { id: finding.policy_id().clone(), name: "Correctness".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: finding.policy_id().clone(), name: "Correctness".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
            renderer: None,
            communicated_findings: None,
            verified_resolutions: Vec::new(),
            number: 1,
            assessment,
            status: RoundStatus::Pending,
            abandonment: None,
            publication_limitation: None,
            answers: vec![],
            transitions: vec![],
            answer_targets: vec![],
            actions: vec![],
        },
        history: vec![],
        actions: vec![PlannedAction::Finding {
            finding: Box::new(finding),
            body: body.into(),
        }],
        unchanged: false,
        summary_body: "Review pending".into(),
        terminal_summary_preview: "Review complete".into(),
        unresolved_findings: vec![FindingId::try_from(1).unwrap()],
    }
}

#[test]
fn untyped_finding_suggestions_fail_before_provider_serialization() {
    let plan = plan(finding(None), "Finding\n\n```suggestion\nreplacement\n```");
    assert_eq!(
        validate_plan_bodies(&plan),
        Err(GitLabError::UnsupportedPosition)
    );
}

#[test]
fn replies_and_both_summaries_reject_active_suggestions() {
    let suggestion = "Reply\n\n```suggestion\nreplacement\n```";
    let mut reply_plan = plan(finding(None), "Finding body");
    reply_plan.actions = vec![PlannedAction::Reply {
        finding: FindingId::try_from(1).unwrap(),
        kind: ReplyKind::Transition(FindingStatus::Fixed),
        body: suggestion.into(),
    }];
    assert_eq!(
        validate_plan_bodies(&reply_plan),
        Err(GitLabError::UnsupportedPosition)
    );
    for terminal in [false, true] {
        let mut plan = plan(finding(None), "Finding body");
        if terminal {
            plan.terminal_summary_preview = suggestion.into();
        } else {
            plan.summary_body = suggestion.into();
        }
        assert_eq!(
            validate_plan_bodies(&plan),
            Err(GitLabError::UnsupportedPosition)
        );
    }
}

#[test]
fn quick_actions_are_rejected_and_literal_code_is_accepted() {
    for location in [0, 1, 2] {
        let mut plan = plan(finding(None), "Finding body");
        match location {
            0 => plan.summary_body = "Review\n/close".into(),
            1 => plan.terminal_summary_preview = "Review\n/approve".into(),
            _ => {
                plan.actions = vec![PlannedAction::Finding {
                    finding: Box::new(finding(None)),
                    body: "Finding\n/close".into(),
                }]
            }
        }
        assert_eq!(validate_plan_bodies(&plan), Err(GitLabError::UnsafeBody));
    }
    assert!(validate_plan_bodies(&plan(finding(None), "Finding\n```rust\n/close\n```")).is_ok());
}

#[test]
fn closed_container_fences_are_accepted_in_findings_replies_and_summaries() {
    for body in [
        "- ```rust\n  /close\n  ```",
        "1. ```rust\n   /close\n   ```",
        "> ```rust\n> /close\n> ```",
        "- > ```rust\n  > /close\n  > ```",
        "- ~~~rust\n  /close\n  ~~~",
        "- ````rust\n  ```\n  /close\n  ````",
        "- ```rust\n  ```",
        "> ```\n> ```",
        "- ```\n  ```",
        "> ~~~~\n> ~~~~~",
    ] {
        let mut plan = plan(finding(None), body);
        plan.summary_body = body.into();
        plan.terminal_summary_preview = body.into();
        plan.actions.push(PlannedAction::Reply {
            finding: FindingId::try_from(1).unwrap(),
            kind: ReplyKind::Transition(FindingStatus::Fixed),
            body: body.into(),
        });
        plan.round.actions = plan.actions.clone();
        assert_eq!(validate_plan_bodies(&plan), Ok(()), "body: {body:?}");
    }
}

#[test]
fn unclosed_container_fences_fail_before_provider_serialization() {
    for body in [
        "```rust\nvalue",
        "- ```rust\n  value",
        "1. ```rust\n   value",
        "> ```rust\n> value",
        "- > ```rust\n  > value",
        "- ```rust\n  value\n\nOutside the list",
        "> ```rust\n> value\n\nOutside the quote",
        "- ```rust",
        "> ```\n>",
        "- ```\n  ",
        "> ~~~\n>\t",
        "> ````\n> ```\n>",
        "> ~~~~\n> ~~~~~\t",
    ] {
        assert_eq!(
            validate_plan_bodies(&plan(finding(None), body)),
            Err(GitLabError::Marker),
            "body: {body:?}"
        );
    }
}

#[test]
fn retained_round_history_and_abandonment_share_action_validation() {
    let invalid = PlannedAction::Reply {
        finding: FindingId::try_from(1).unwrap(),
        kind: ReplyKind::Transition(FindingStatus::Fixed),
        body: "Reply\n/close".into(),
    };
    for location in [0, 1, 2] {
        let mut plan = plan(finding(None), "Finding body");
        match location {
            0 => plan.round.actions.push(invalid.clone()),
            1 => {
                let mut historical = plan.round.clone();
                historical.actions.push(invalid.clone());
                plan.history.push(historical);
            }
            _ => {
                plan.round.abandonment = Some(Abandonment {
                    reason: "Replaced assessment".into(),
                    verified_actions: vec![invalid.clone()],
                })
            }
        }
        assert_eq!(validate_plan_bodies(&plan), Err(GitLabError::UnsafeBody));
    }
}

#[test]
fn matching_typed_suggestions_accept_backticks_and_slash_code() {
    let finding = finding(Some(vec![
        "```".into(),
        "/close".into(),
        "replacement".into(),
    ]));
    let body = "Finding\n\n````suggestion:-1+1\n```\n/close\nreplacement\n````";
    let mut plan = plan(finding, body);
    plan.round.actions = plan.actions.clone();
    assert!(validate_plan_bodies(&plan).is_ok());
}
