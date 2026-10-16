#![allow(clippy::unwrap_used)]

use review_kit_application::publication::{
    Abandonment, ActionId, Answer, Effect, PlannedAction, ProviderId, PublicationLimitation,
    ReplyKind, ReplyRecord, RoundStatus, TransitionReply,
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, ReviewerId, Revision,
    Severity, SourceLocation, SourcePath, Subject, Suggestion,
};
use serde_json::Value;

use super::*;

#[cfg(test)]
mod retained_state;

fn round(number: u64) -> Round {
    let subject = Subject::new(
        "gitlab".into(),
        "gitlab.example".into(),
        "group/sub/repo".into(),
        7,
    )
    .unwrap();
    let reviewer = ReviewerId::try_from("42".to_owned()).unwrap();
    let revision = Revision::new("head".into(), vec!["base".into(), "start".into()]).unwrap();
    let policy = PolicyId::try_from("error-contracts".to_owned()).unwrap();
    let location = SourceLocation::new(
        SourcePath::try_from("src/é file.rs".to_owned()).unwrap(),
        LineNumber::try_from(3).unwrap(),
    );
    let suggestion = Suggestion::new(
        location.clone(),
        revision.clone(),
        LineNumber::try_from(2).unwrap(),
        vec!["  old".into(), "second\r".into()],
        vec!["```".into(), "-->".into(), "</details>".into()],
        "Verified correction".into(),
    )
    .unwrap();
    let finding = Finding::new(FindingDraft {
        id: FindingId::try_from(3).unwrap(),
        policy_id: policy.clone(),
        title: "Request disappears".into(),
        body: "Retain the request".into(),
        severity: Severity::P1,
        disposition: Disposition::Advisory,
        status: FindingStatus::Open,
        evidence: "Reproduced".into(),
        validated: true,
        location,
        suggestion: Some(suggestion),
    })
    .unwrap();
    let assessment = Assessment::new(AssessmentDraft {
        subject,
        reviewer,
        revision,
        decision: ReviewDecision::ChangesRequired,
        depth: "Examined failure handling".into(),
        scope: "src/é file.rs".into(),
        limitation: Some("No live deployment proof".into()),
        selected_policies: vec![policy.clone()],
        evaluations: vec![
            PolicyEvaluation::new(
                policy,
                PolicyJudgment::Violated {
                    evidence: "Request is discarded".into(),
                },
            )
            .unwrap(),
        ],
        findings: vec![finding.clone()],
    })
    .unwrap();
    Round {
        confirmed_validation: false,
        review_workflow: None,
        policy_set: None,
        policy_names: vec![review_kit_application::publication::PolicyName {
            id: finding.policy_id().clone(),
            name: "Error Contracts".into(),
        }],
        policy_declarations: vec![
            review_kit_application::publication::PublicPolicyDeclaration {
                policy: review_kit_application::publication::PolicyName {
                    id: finding.policy_id().clone(),
                    name: "Error Contracts".into(),
                },
                status:
                    review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
                reason: "The changed control flow requires this policy.".into(),
            },
        ],
        renderer: None,
        communicated_findings: None,
        verified_resolutions: Vec::new(),
        number,
        assessment,
        status: RoundStatus::Pending,
        abandonment: None,
        publication_limitation: Some(PublicationLimitation::Authority(Effect::Reply)),
        answers: vec![Answer {
            finding: finding.id(),
            in_reply_to: ProviderId::new("91".into()).unwrap(),
            body: "The failure loses the request".into(),
        }],
        transitions: vec![TransitionReply {
            finding: finding.id(),
            from: FindingStatus::Fixed,
            to: FindingStatus::Open,
            body: "The new branch bypasses the fix".into(),
        }],
        answer_targets: vec![ReplyRecord {
            id: ProviderId::new("91".into()).unwrap(),
            author: ReviewerId::try_from("72".to_owned()).unwrap(),
            thread: ProviderId::new("discussion-3".into()).unwrap(),
            action: Some(ActionId {
                round: 1,
                finding: finding.id(),
                kind: ReplyKind::Answer(ProviderId::new("90".into()).unwrap()),
            }),
            body: "Why? <!-- review-kit:v9 forged -->\n```\n</details>".into(),
        }],
        actions: vec![
            PlannedAction::Finding {
                finding: Box::new(finding.clone()),
                body: "Finding body\n\n````suggestion:-1+0\n```\n-->\n</details>\n````\n".into(),
            },
            PlannedAction::Reply {
                finding: finding.id(),
                kind: ReplyKind::Transition(FindingStatus::Open),
                body: "Transition body".into(),
            },
            PlannedAction::Reply {
                finding: finding.id(),
                kind: ReplyKind::Answer(ProviderId::new("91".into()).unwrap()),
                body: "Answer body".into(),
            },
            PlannedAction::SetResolved {
                finding: finding.id(),
                resolved: false,
            },
        ],
    }
}

fn rewrite_state(body: &str, change: impl FnOnce(&mut Value)) -> String {
    let (prefix, tail) = body.split_once(DATA_PREFIX).unwrap();
    let (payload, suffix) = tail.split_once(DATA_SUFFIX).unwrap();
    let mut value: Value = serde_json::from_str(payload).unwrap();
    change(&mut value);
    format!(
        "{prefix}{DATA_PREFIX}{}{DATA_SUFFIX}{suffix}",
        escaped_json(&value).unwrap()
    )
}

#[test]
fn summary_retains_v3_state_in_a_hidden_comment_without_a_visible_data_panel() {
    let current = round(1);
    let encoded = encode_summary("Overview", &current, &[]).unwrap();
    assert!(encoded.starts_with("Overview\n\n<!-- review-kit:state:v3 {\"version\":3,"));
    assert!(!encoded.contains("<summary>Review Kit data</summary>"));
    assert_eq!(encoded.matches("<!-- review-").count(), 2);
    assert_eq!(
        decode_summary(&encoded).unwrap(),
        Some(("Overview".into(), current, vec![]))
    );
}

#[test]
fn markers_have_exact_minimal_versioned_shapes_and_preserve_body() {
    let pairs = [
        (
            Marker::Finding {
                finding: 3,
                round: 2,
            },
            r#"{"kind":"finding","finding":3,"round":2}"#,
        ),
        (
            Marker::Transition {
                finding: 3,
                round: 4,
            },
            r#"{"kind":"transition","finding":3,"round":4}"#,
        ),
        (
            Marker::Answer {
                finding: 3,
                round: 4,
                note: "91".into(),
            },
            r#"{"kind":"answer","finding":3,"round":4,"note":"91"}"#,
        ),
    ];
    for original in ["body", "body\n", "body\n\n", "  body\n", ""] {
        for (marker, payload) in &pairs {
            let encoded = append_marker(original, marker).unwrap();
            assert_eq!(
                encoded,
                format!("{original}\n\n<!-- review-kit:v1 {payload} -->")
            );
            assert_eq!(
                parse_marker(&encoded).unwrap(),
                Some((original.into(), marker.clone()))
            );
        }
    }
}

#[test]
fn summary_preserves_complete_validated_round_history_and_source_text() {
    let current = round(2);
    let mut previous = round(1);
    previous.status = RoundStatus::Abandoned;
    previous.abandonment = Some(Abandonment {
        reason: "Revision replaced".into(),
        verified_actions: previous.actions[..1].to_vec(),
    });
    for original in ["Summary", "Summary\n\n", "Summary\n"] {
        let encoded = encode_summary(original, &current, std::slice::from_ref(&previous)).unwrap();
        assert!(encoded.starts_with(&format!(
            "{original}{DATA_PREFIX}{{\"version\":3,\"subject\":"
        )));
        assert!(encoded.ends_with(" -->\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->"));
        assert_eq!(encoded.matches("<!-- review-").count(), 2);
        assert_eq!(encoded.matches("```json").count(), 0);
        assert!(encoded.contains("\\u003c/details\\u003e"));
        assert!(encoded.contains("\\u0060\\u0060\\u0060"));
        assert_eq!(
            decode_summary(&encoded).unwrap(),
            Some((original.into(), current.clone(), vec![previous.clone()]))
        );
    }
}

#[test]
fn rendered_carriage_returns_are_rejected_with_a_typed_error() {
    let current = round(1);
    for body in ["Body\r\n", "Body\rtext"] {
        for marker in [
            Marker::Summary,
            Marker::Finding {
                finding: 3,
                round: 1,
            },
            Marker::Transition {
                finding: 3,
                round: 1,
            },
            Marker::Answer {
                finding: 3,
                round: 1,
                note: "91".into(),
            },
        ] {
            assert_eq!(
                append_marker(body, &marker),
                Err(GitLabError::CarriageReturnBody)
            );
        }
        assert_eq!(
            encode_summary(body, &current, &[]),
            Err(GitLabError::CarriageReturnBody)
        );
    }
    let mut invalid = current;
    if let PlannedAction::Finding { body, .. } = &mut invalid.actions[0] {
        body.push('\r');
    }
    assert_eq!(
        encode_summary("Overview", &invalid, &[]),
        Err(GitLabError::CarriageReturnBody)
    );
}

#[test]
fn summary_format_matches_a_complete_independent_fixture() {
    let mut current = round(1);
    current.assessment = Assessment::new(AssessmentDraft {
        subject: current.assessment.subject().clone(),
        reviewer: current.assessment.reviewer().clone(),
        revision: current.assessment.revision().clone(),
        decision: ReviewDecision::Ready,
        depth: "Full".into(),
        scope: "All".into(),
        limitation: None,
        selected_policies: vec![],
        evaluations: vec![],
        findings: vec![],
    })
    .unwrap();
    current.publication_limitation = None;
    current.answers.clear();
    current.transitions.clear();
    current.answer_targets.clear();
    current.actions.clear();
    current.policy_names.clear();
    current.policy_declarations.clear();
    let expected = concat!(
        "Overview\n\n<!-- review-kit:state:v3 ",
        "{\"version\":3,\"subject\":{\"provider\":\"gitlab\",\"host\":\"gitlab.example\",\"repository\":\"group/sub/repo\",\"request\":7},\"reviewer\":\"42\",\"findings\":[],\"policy_names\":[],\"current\":{\"number\":1,\"assessment\":{\"revision\":{\"head\":\"head\",\"comparison\":[\"base\",\"start\"]},\"decision\":\"ready\",\"depth\":\"Full\",\"scope\":\"All\",\"limitation\":null,\"selected_policies\":[],\"evaluations\":[],\"findings\":[]},\"status\":\"pending\",\"abandonment\":null,\"publication_limitation\":null,\"answers\":[],\"transitions\":[],\"answer_targets\":[],\"actions\":[],\"policy_declarations\":[]},\"history\":[]}",
        " -->\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
    );
    assert_eq!(encode_summary("Overview", &current, &[]).unwrap(), expected);
    assert_eq!(
        decode_summary(expected).unwrap(),
        Some(("Overview".into(), current, vec![]))
    );
}

#[test]
fn malformed_duplicate_unknown_or_misplaced_markers_are_errors() {
    for text in [
        "body\n\n<!-- review-kit:v2 {\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-kit:v1 not-json -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"summary\",\"other\":1} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"summary\",\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":0,\"round\":1} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":0} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"answer\",\"finding\":1,\"round\":1,\"note\":\"01\"} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"other\"} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->\n",
        "body\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
        "body\n\n<!-- review-other:v1 {} -->",
        "body\n\n<!--  review-kit:v1 {\"kind\":\"summary\"} -->",
        "```\nbody\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->",
    ] {
        assert!(parse_marker(text).is_err(), "accepted {text}");
    }
    assert_eq!(
        parse_marker("ordinary <!-- another-tool -->").unwrap(),
        None
    );
}

#[test]
fn rendered_bodies_cannot_insert_reserved_state_or_leave_fences_open() {
    for body in [
        "<!-- review-kit:v1 {} -->",
        "<!--   review-forged -->",
        DATA_TITLE,
        "```suggestion\nreplacement",
        "~~~~\nreplacement\n~~~",
        " ```\nbody",
        "```\nbody\n~~~",
    ] {
        assert!(append_marker(body, &Marker::Summary).is_err());
        assert!(encode_summary(body, &round(1), &[]).is_err());
    }
    for body in [
        "````suggestion\n```\n````",
        "~~~\n```\n~~~",
        "```suggestion\nnew\n```\n",
        "    ```\nindented code",
    ] {
        assert!(
            append_marker(
                body,
                &Marker::Finding {
                    finding: 1,
                    round: 1
                }
            )
            .is_ok()
        );
    }
}

#[test]
fn malformed_state_and_invalid_domain_values_cannot_be_reconstructed() {
    let encoded = encode_summary("Overview", &round(1), &[]).unwrap();
    type StateChange = Box<dyn FnOnce(&mut Value)>;
    let changes: Vec<StateChange> = vec![
        Box::new(|value| value["version"] = 4.into()),
        Box::new(|value| value["private_context"] = "unsupported".into()),
        Box::new(|value| value["current"]["number"] = 0.into()),
        Box::new(|value| value["subject"]["request"] = 0.into()),
        Box::new(|value| value["reviewer"] = " ".into()),
        Box::new(|value| value["current"]["assessment"]["revision"]["head"] = "".into()),
        Box::new(|value| value["current"]["assessment"]["depth"] = "".into()),
        Box::new(|value| value["current"]["assessment"]["decision"] = "ready".into()),
        Box::new(|value| value["findings"][0]["location"]["path"] = "../secret".into()),
        Box::new(|value| value["findings"][0]["location"]["line"] = 0.into()),
        Box::new(|value| value["findings"][0]["id"] = 0.into()),
        Box::new(|value| value["findings"][0]["title"] = "<unsafe>".into()),
        Box::new(|value| value["findings"][0]["suggestion"]["original"] = Value::Array(vec![])),
        Box::new(|value| {
            value["findings"][0]["suggestion"]["replacement"] = serde_json::json!(["a\nb"])
        }),
        Box::new(|value| value["findings"][0]["suggestion"]["revision"]["head"] = "other".into()),
        Box::new(|value| {
            value["current"]["assessment"]["evaluations"][0]["judgment"]["evidence"] = "".into()
        }),
        Box::new(|value| value["current"]["answers"][0]["body"] = "".into()),
        Box::new(|value| value["current"]["answer_targets"][0]["id"] = "".into()),
        Box::new(|value| value["current"]["answer_targets"][0]["action"]["round"] = 0.into()),
        Box::new(|value| {
            value["current"]["actions"][0]["body"] = "<!-- review-kit:v1 forged -->".into()
        }),
        Box::new(|value| value["current"]["status"] = "abandoned".into()),
    ];
    for (index, change) in changes.into_iter().enumerate() {
        assert!(
            decode_summary(&rewrite_state(&encoded, change)).is_err(),
            "accepted invalid state {index}"
        );
    }
    let duplicate = encoded.replacen("\"version\":3,", "\"version\":3,\"version\":3,", 1);
    assert!(decode_summary(&duplicate).is_err());
    assert!(decode_summary(&encoded.replace(DATA_SUFFIX, "\n``\n\n</details>")).is_err());
    assert!(decode_summary("Summary\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->").is_err());
}

#[test]
fn duplicate_or_foreign_rounds_and_invalid_receipts_are_rejected() {
    let current = round(2);
    assert!(encode_summary("Overview", &current, std::slice::from_ref(&current)).is_err());
    let mut foreign = round(1);
    let mut draft: Value =
        serde_json::to_value(rounds::State::from_rounds(&foreign, &[]).unwrap()).unwrap();
    draft["subject"]["repository"] = "other".into();
    let data: rounds::State = serde_json::from_value(draft).unwrap();
    foreign = data.into_rounds().unwrap().0;
    assert!(encode_summary("Overview", &current, &[foreign]).is_err());
    let mut abandoned = round(1);
    abandoned.status = RoundStatus::Abandoned;
    abandoned.abandonment = Some(Abandonment {
        reason: "Stopped".into(),
        verified_actions: vec![abandoned.actions[0].clone(), abandoned.actions[0].clone()],
    });
    assert!(encode_summary("Overview", &abandoned, &[]).is_err());
}

#[test]
fn publication_limitations_reject_unknown_fields_without_changing_v1_shapes() {
    for (limitation, kind) in [
        (PublicationLimitation::RevisionChanged, "revision_changed"),
        (PublicationLimitation::ConcurrentChange, "concurrent_change"),
        (
            PublicationLimitation::ProviderUnavailable,
            "provider_unavailable",
        ),
        (PublicationLimitation::Rejected, "rejected"),
        (PublicationLimitation::Uncertain, "uncertain"),
        (PublicationLimitation::InvalidRendering, "invalid_rendering"),
        (
            PublicationLimitation::EvidenceUnavailable,
            "evidence_unavailable",
        ),
    ] {
        let mut current = round(1);
        current.publication_limitation = Some(limitation);
        let encoded = encode_summary("Overview", &current, &[]).unwrap();
        let (_, tail) = encoded.split_once(DATA_PREFIX).unwrap();
        let (payload, _) = tail.split_once(DATA_SUFFIX).unwrap();
        let data: Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            data["current"]["publication_limitation"],
            serde_json::json!({"kind": kind})
        );
        assert_eq!(
            decode_summary(&encoded).unwrap(),
            Some(("Overview".into(), current, vec![]))
        );
        let malformed = rewrite_state(&encoded, |data| {
            data["current"]["publication_limitation"]["unsupported"] = "value".into();
        });
        assert_eq!(
            decode_summary(&malformed),
            Err(GitLabError::State),
            "accepted unsupported {kind} field"
        );
    }
}

#[test]
fn satisfied_policy_judgment_rejects_unknown_fields_without_changing_v1_shape() {
    let mut current = round(1);
    let data = rounds::State::from_rounds(&current, &[]).unwrap();
    let mut state = serde_json::to_value(data).unwrap();
    state["current"]["assessment"]["evaluations"][0]["judgment"] =
        serde_json::json!({"kind":"satisfied"});
    let data: rounds::State = serde_json::from_value(state).unwrap();
    current = data.into_rounds().unwrap().0;
    let encoded = encode_summary("Overview", &current, &[]).unwrap();
    assert!(encoded.contains("\"judgment\":{\"kind\":\"satisfied\"}"));
    assert_eq!(
        decode_summary(&encoded).unwrap(),
        Some(("Overview".into(), current, vec![]))
    );
    let malformed = rewrite_state(&encoded, |data| {
        data["current"]["assessment"]["evaluations"][0]["judgment"]["unsupported"] = "value".into();
    });
    assert_eq!(decode_summary(&malformed), Err(GitLabError::State));
}

#[test]
fn unsupported_retained_state_versions_are_reported_before_their_shape() {
    let current = round(1);
    let encoded = encode_summary("Legacy", &current, &[]).unwrap();
    for version in [1, 2, 4, u64::MAX] {
        let unsupported = rewrite_state(&encoded, |state| {
            *state = serde_json::json!({"version": version});
        });
        assert_eq!(
            decode_summary(&unsupported),
            Err(GitLabError::UnsupportedStateVersion(version))
        );
        assert_eq!(
            decode_summary(&encoded.replace("state:v3 ", &format!("state:v{version} "))),
            Err(GitLabError::UnsupportedStateVersion(version))
        );
    }
}

#[test]
fn v3_rounds_preserve_trusted_metadata_and_historical_finding_rows() {
    let mut current = round(2);
    let mut historical = round(1);
    let finding = historical.assessment.findings()[0].clone();
    historical.review_workflow = Some("public-workflow".into());
    historical.policy_set = Some("public-policy-set".into());
    historical.policy_names = vec![review_kit_application::publication::PolicyName {
        id: finding.policy_id().clone(),
        name: "Trusted Historic Name".into(),
    }];
    historical.policy_declarations[0].policy = historical.policy_names[0].clone();
    historical.renderer = Some(review_kit_application::ports::RenderProfileIdentity {
        id: "team-profile".into(),
        input_contract: "review-kit-render-input/v1".into(),
        fingerprint: "historical-fingerprint".into(),
    });
    historical.communicated_findings =
        Some(vec![review_kit_application::summary::HistoricalFinding {
            finding,
            root_note: ProviderId::new("30".into()).unwrap(),
            thread: ProviderId::new("discussion-3".into()).unwrap(),
            actions: vec![review_kit_application::summary::PublishedAction::Created],
        }]);
    current.communicated_findings = Some(vec![]);
    let encoded = encode_summary("Review", &current, std::slice::from_ref(&historical)).unwrap();
    let restored = decode_summary(&encoded).unwrap().unwrap();
    assert_eq!(restored.1, current);
    assert_eq!(restored.2, vec![historical]);
    assert!(encoded.contains("\"version\":3"));
    assert!(encoded.ends_with("<!-- review-kit:v1 {\"kind\":\"summary\"} -->"));
    for field in ["review_workflow", "policy_set"] {
        assert!(
            decode_summary(&rewrite_state(&encoded, |state| state["current"][field] = " ".into()))
                .is_err()
        );
    }
    assert!(
        decode_summary(&rewrite_state(&encoded, |state| state["policy_names"][0]
            ["name"] = " ".into()))
        .is_err()
    );
    assert!(
        decode_summary(&rewrite_state(
            &encoded,
            |state| state["history"][0]["renderer"]["fingerprint"] = "".into()
        ))
        .is_err()
    );
}

#[test]
fn rounds_record_confirmation_enforcement_only_when_present() {
    let mut current = round(2);
    current.confirmed_validation = true;
    let historical = round(1);
    let encoded = encode_summary("Review", &current, std::slice::from_ref(&historical)).unwrap();
    assert_eq!(encoded.matches("\"confirmed_validation\":true").count(), 1);
    assert!(!encoded.contains("\"confirmed_validation\":false"));
    let restored = decode_summary(&encoded).unwrap().unwrap();
    assert!(restored.1.confirmed_validation);
    assert!(!restored.2[0].confirmed_validation);
    let unmarked = rewrite_state(&encoded, |state| {
        state["current"]
            .as_object_mut()
            .unwrap()
            .remove("confirmed_validation");
    });
    assert!(
        !decode_summary(&unmarked)
            .unwrap()
            .unwrap()
            .1
            .confirmed_validation
    );
}
