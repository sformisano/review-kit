use super::*;
use review_kit_application::{
    publication::{PolicyName, ResolutionProvenance, ResolutionReceipt},
    summary::{HistoricalFinding, PublishedAction},
};
use review_kit_json_adapter::{JsonError, domain::AssessmentData};

fn state(current: &Round, history: &[Round]) -> Value {
    serde_json::to_value(rounds::State::from_rounds(current, history).unwrap()).unwrap()
}

fn restore(value: Value) -> Result<(Round, Vec<Round>), JsonError> {
    serde_json::from_value::<rounds::State>(value)
        .map_err(JsonError::Syntax)?
        .into_rounds()
}

#[test]
fn identical_finding_content_is_shared_by_assessment_intent_snapshot_and_abandonment() {
    let mut current = round(1);
    current.status = RoundStatus::Abandoned;
    current.abandonment = Some(Abandonment {
        reason: "Review stopped".into(),
        verified_actions: vec![current.actions[0].clone()],
    });
    current.communicated_findings = Some(vec![HistoricalFinding {
        finding: current.assessment.findings()[0].clone(),
        root_note: ProviderId::new("30".into()).unwrap(),
        thread: ProviderId::new("discussion-3".into()).unwrap(),
        actions: vec![PublishedAction::Created],
    }]);
    let value = state(&current, &[]);
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["current"]["assessment"]["findings"],
        serde_json::json!([0])
    );
    assert_eq!(value["current"]["actions"][0]["finding"], 0);
    assert_eq!(value["current"]["communicated_findings"][0]["finding"], 0);
    assert_eq!(
        value["current"]["abandonment"]["verified_actions"][0]["finding"],
        0
    );
    assert_eq!(restore(value).unwrap(), (current.clone(), vec![]));
    let body = encode_summary("Overview", &current, &[]).unwrap();
    let (visible, decoded, history) = decode_summary(&body).unwrap().unwrap();
    assert_eq!(encode_summary(&visible, &decoded, &history).unwrap(), body);
}

#[test]
fn same_id_with_different_complete_content_keeps_distinct_versions() {
    for (field, replacement) in [
        ("status", serde_json::json!("fixed")),
        ("title", serde_json::json!("Another title")),
        ("body", serde_json::json!("Another explanation")),
        ("evidence", serde_json::json!("Another probe")),
        ("suggestion", Value::Null),
    ] {
        let mut current = round(2);
        let historical = round(1);
        let mut assessment =
            serde_json::to_value(AssessmentData::from(&current.assessment)).unwrap();
        assessment["findings"][0][field] = replacement;
        current.assessment = serde_json::from_value::<AssessmentData>(assessment)
            .unwrap()
            .into_assessment()
            .unwrap();
        let value = state(&current, std::slice::from_ref(&historical));
        assert_eq!(value["findings"].as_array().unwrap().len(), 2, "{field}");
        assert_eq!(value["findings"][0]["id"], value["findings"][1]["id"]);
        assert_eq!(value["current"]["assessment"]["findings"][0], 0);
        assert_eq!(value["current"]["actions"][0]["finding"], 1);
        assert_eq!(value["history"][0]["assessment"]["findings"][0], 1);
        assert_eq!(restore(value).unwrap(), (current, vec![historical]));
    }
}

#[test]
fn policy_names_share_exact_rows_and_retain_historical_renaming() {
    let mut current = round(3);
    let mut first = round(1);
    let mut second = round(2);
    for (retained, name) in [
        (&mut current, "New name"),
        (&mut first, "Original name"),
        (&mut second, "Original name"),
    ] {
        retained.policy_names = vec![PolicyName {
            id: retained.assessment.selected_policies()[0].clone(),
            name: name.into(),
        }];
        retained.policy_declarations[0].policy = retained.policy_names[0].clone();
    }
    let history = vec![first, second];
    let value = state(&current, &history);
    assert_eq!(value["policy_names"].as_array().unwrap().len(), 2);
    assert_eq!(value["current"]["policy_names"], serde_json::json!([0]));
    assert_eq!(value["history"][0]["policy_names"], serde_json::json!([1]));
    assert_eq!(value["history"][1]["policy_names"], serde_json::json!([1]));
    assert_eq!(restore(value).unwrap(), (current, history));
}

#[test]
fn public_declarations_round_trip_excluded_names_and_original_per_round_decisions() {
    use review_kit_application::{
        publication::PublicPolicyDeclaration, use_cases::checkpoints::ApplicabilityStatus,
    };
    let mut old = round(1);
    let excluded = PublicPolicyDeclaration {
        policy: PolicyName {
            id: review_kit_domain::PolicyId::try_from("async-work".to_owned()).unwrap(),
            name: "Async Work | original".into(),
        },
        status: ApplicabilityStatus::Excluded,
        reason: "No asynchronous work\n<details> @reviewer".into(),
    };
    old.policy_declarations.push(excluded.clone());
    let mut current = old.clone();
    current.number = 2;
    current.policy_declarations[1].policy.name = "Async Work renamed".into();
    current.policy_declarations[1].reason = "Only synchronous decoding changes.".into();
    let value = state(&current, std::slice::from_ref(&old));
    assert_eq!(value["policy_names"].as_array().unwrap().len(), 3);
    assert_eq!(value["current"]["policy_declarations"][1]["policy"], 1);
    assert_eq!(value["history"][0]["policy_declarations"][1]["policy"], 2);
    assert_eq!(
        restore(value.clone()).unwrap(),
        (current.clone(), vec![old.clone()])
    );
    let local: review_kit_json_adapter::rounds::RoundData = serde_json::from_value(
        serde_json::to_value(review_kit_json_adapter::rounds::RoundData::from(&current)).unwrap(),
    )
    .unwrap();
    assert_eq!(local.into_round().unwrap(), current);
    let body = encode_summary("Overview", &current, std::slice::from_ref(&old)).unwrap();
    let (_, restored, history) = decode_summary(&body).unwrap().unwrap();
    assert_eq!(restored, current);
    assert_eq!(history, vec![old]);
    for field in [
        "source",
        "introduced_by",
        "policy_inventory",
        "local_context",
        "confirmations",
        "basis",
    ] {
        assert!(!body.contains(&format!("\"{field}\"")));
    }
    for mutation in 0..5 {
        let mut invalid = value.clone();
        match mutation {
            0 => invalid["current"]["policy_declarations"][1]["reason"] = " ".into(),
            1 => invalid["current"]["policy_declarations"][1]["status"] = "unknown".into(),
            2 => invalid["current"]["policy_declarations"][1]["policy"] = u64::MAX.into(),
            3 => invalid["current"]["policy_declarations"][0]["policy"] = 1.into(),
            _ => invalid["current"]["policy_declarations"][0]["status"] = "unresolved".into(),
        }
        assert!(restore(invalid).is_err(), "mutation {mutation}");
    }
}

#[test]
fn excluded_declaration_name_amplification_is_bounded_before_reconstruction() {
    use review_kit_application::{
        publication::PublicPolicyDeclaration, use_cases::checkpoints::ApplicabilityStatus,
    };
    let mut current = round(1);
    current.policy_declarations.push(PublicPolicyDeclaration {
        policy: PolicyName {
            id: review_kit_domain::PolicyId::try_from("async-work".to_owned()).unwrap(),
            name: "x".repeat(1024 * 1024),
        },
        status: ApplicabilityStatus::Excluded,
        reason: "No asynchronous work.".into(),
    });
    let mut value = state(&current, &[]);
    let retained = value["current"].clone();
    value["current"]["number"] = 40.into();
    value["history"] = serde_json::json!(
        (1..40)
            .map(|number| {
                let mut round = retained.clone();
                round["number"] = number.into();
                round
            })
            .collect::<Vec<_>>()
    );
    assert!(matches!(restore(value), Err(JsonError::RetainedStateLimit)));
}

#[test]
fn receipt_headers_restore_exactly_and_foreign_headers_are_rejected_before_stripping() {
    let mut current = round(1);
    current.verified_resolutions = vec![ResolutionReceipt {
        subject: current.assessment.subject().clone(),
        reviewer: current.assessment.reviewer().clone(),
        round: 1,
        thread: ProviderId::new("discussion-3".into()).unwrap(),
        resolved: false,
        provenance: ResolutionProvenance::Observed,
    }];
    let value = state(&current, &[]);
    assert_eq!(
        value["current"]["verified_resolutions"][0],
        serde_json::json!({"thread":"discussion-3","resolved":false,"provenance":"observed"})
    );
    assert_eq!(restore(value).unwrap(), (current.clone(), vec![]));
    current.verified_resolutions[0].provenance = ResolutionProvenance::Acknowledged;
    assert_eq!(
        restore(state(&current, &[])).unwrap(),
        (current.clone(), vec![])
    );
    for property in ["subject", "reviewer", "round"] {
        let mut invalid = current.clone();
        match property {
            "subject" => {
                invalid.verified_resolutions[0].subject =
                    Subject::new("gitlab".into(), "gitlab.example".into(), "other".into(), 7)
                        .unwrap()
            }
            "reviewer" => {
                invalid.verified_resolutions[0].reviewer =
                    ReviewerId::try_from("99".to_owned()).unwrap()
            }
            _ => invalid.verified_resolutions[0].round = 2,
        }
        assert!(
            matches!(
                rounds::State::from_rounds(&invalid, &[]),
                Err(JsonError::ContextBinding)
            ),
            "{property}"
        );
    }
}

#[test]
fn duplicate_unused_dangling_and_noncanonical_tables_fail() {
    let mut current = round(1);
    current.policy_names = vec![PolicyName {
        id: current.assessment.selected_policies()[0].clone(),
        name: "Name".into(),
    }];
    current.policy_declarations[0].policy = current.policy_names[0].clone();
    let valid = state(&current, &[]);
    for table in ["findings", "policy_names"] {
        let mut duplicate = valid.clone();
        let first = duplicate[table][0].clone();
        duplicate[table].as_array_mut().unwrap().push(first);
        assert!(restore(duplicate).is_err(), "duplicate {table}");
        let mut unused = valid.clone();
        let mut extra = unused[table][0].clone();
        extra[if table == "findings" { "title" } else { "name" }] = "Unused".into();
        unused[table].as_array_mut().unwrap().push(extra);
        assert!(restore(unused).is_err(), "unused {table}");
    }
    for index in [
        serde_json::json!(-1),
        serde_json::json!(0.5),
        serde_json::json!(1),
        serde_json::json!(u64::MAX),
    ] {
        let mut dangling = valid.clone();
        dangling["current"]["assessment"]["findings"][0] = index;
        assert!(restore(dangling).is_err());
    }
    let mut reordered = valid;
    let mut second = reordered["findings"][0].clone();
    second["title"] = "Other version".into();
    reordered["findings"].as_array_mut().unwrap().push(second);
    reordered["current"]["assessment"]["findings"][0] = 1.into();
    assert!(restore(reordered).is_err());
}

#[test]
fn expanded_bytes_and_reference_count_are_bounded_before_restoration() {
    let current = round(1);
    let mut oversized = state(&current, &[]);
    oversized["findings"][0]["evidence"] = "e".repeat(512 * 1024).into();
    oversized["current"]["actions"] = serde_json::json!(vec![
        serde_json::json!({"kind":"finding","finding":0,"body":"body"});
        65
    ]);
    assert!(matches!(
        restore(oversized),
        Err(JsonError::RetainedStateLimit)
    ));
    let mut numerous = state(&current, &[]);
    numerous["findings"][0]["suggestion"] = Value::Null;
    numerous["current"]["actions"] = serde_json::json!(vec![
        serde_json::json!({"kind":"finding","finding":0,"body":"body"});
        65_536
    ]);
    assert!(matches!(
        restore(numerous),
        Err(JsonError::RetainedStateLimit)
    ));
    let mut control = state(&current, &[]);
    control["current"]["actions"] = serde_json::json!(vec![
        serde_json::json!({"kind":"finding","finding":0,"body":"body"});
        10
    ]);
    assert!(restore(control).is_ok());
}

#[test]
fn state_record_pairing_and_escaped_payload_are_required() {
    let valid = encode_summary("Overview", &round(1), &[]).unwrap();
    let (visible, suffix) = valid.split_once(DATA_PREFIX).unwrap();
    let (payload, _) = suffix.split_once(DATA_SUFFIX).unwrap();
    let marker = "<!-- review-kit:v1 {\"kind\":\"summary\"} -->";
    for malformed in [
        format!("{visible}{DATA_PREFIX}{payload}{DATA_SUFFIX}"),
        format!("{visible}\n\n{marker}"),
        format!("{visible}\n\n{marker}{DATA_PREFIX}{payload}{DATA_SUFFIX}"),
        format!(
            "{visible}{DATA_PREFIX}{payload}{DATA_SUFFIX}{DATA_PREFIX}{payload}{DATA_SUFFIX}\n\n{marker}"
        ),
        valid.replace(" -->\n\n<!-- review-kit:v1", " -->\n<!-- review-kit:v1"),
        valid.replace("Overview", DATA_TITLE),
        valid.replace("\\u003c", "<"),
        valid.replacen("{\"version\":3", "{\n\"version\":3", 1),
        valid.replacen("{\"version\":3", "{\r\"version\":3", 1),
        format!("{valid}\n"),
        valid.replace(
            "{\"kind\":\"summary\"}",
            "{\"kind\":\"summary\",\"kind\":\"summary\"}",
        ),
    ] {
        assert!(
            decode_summary(&malformed).is_err(),
            "accepted malformed state framing"
        );
    }
    let oversized = format!(
        "Overview{DATA_PREFIX}{{\"version\":3,\"padding\":\"{}\"}}{DATA_SUFFIX}\n\n{marker}",
        "x".repeat(rounds::State::MAX_BYTES)
    );
    assert_eq!(decode_summary(&oversized), Err(GitLabError::Limit));
    assert_eq!(
        parse_marker(&valid).unwrap(),
        Some(("Overview".into(), Marker::Summary))
    );
}

#[test]
fn v3_expansion_limits_cover_shared_policy_names_and_lifted_headers() {
    for property in ["policy_name", "subject", "reviewer", "receipt"] {
        let mut value = state(&round(1), &[]);
        match property {
            "policy_name" => {
                value["policy_names"] =
                    serde_json::json!([{"id":"error-contracts","name":"n".repeat(600_000)}]);
                value["current"]["policy_names"] = serde_json::json!([0]);
            }
            "subject" | "receipt" => value["subject"]["repository"] = "r".repeat(600_000).into(),
            _ => value["reviewer"] = "u".repeat(600_000).into(),
        }
        let mut control = value.clone();
        if property == "receipt" {
            let receipts = (0..60).map(|index| serde_json::json!({"thread":format!("thread-{index}"),"resolved":false,"provenance":"observed"})).collect::<Vec<_>>();
            value["current"]["verified_resolutions"] = serde_json::json!(receipts);
            control["current"]["verified_resolutions"] =
                serde_json::json!([{"thread":"one","resolved":false,"provenance":"observed"}]);
        } else {
            value["history"] = serde_json::json!(
                (1..=60)
                    .map(|number| {
                        let mut retained = value["current"].clone();
                        retained["number"] = number.into();
                        retained
                    })
                    .collect::<Vec<_>>()
            );
            value["current"]["number"] = 61.into();
        }
        assert!(restore(control).is_ok(), "valid {property} control");
        assert!(
            matches!(restore(value), Err(JsonError::RetainedStateLimit)),
            "unbounded {property} expansion"
        );
    }
}

#[test]
fn v3_expansion_limits_also_bound_escaped_encoded_payloads() {
    let mut current = round(1);
    let mut assessment = serde_json::to_value(AssessmentData::from(&current.assessment)).unwrap();
    assessment["scope"] = "<".repeat(rounds::State::MAX_BYTES / 6).into();
    current.assessment = serde_json::from_value::<AssessmentData>(assessment)
        .unwrap()
        .into_assessment()
        .unwrap();
    assert!(matches!(
        encode_summary("Overview", &current, &[]),
        Err(GitLabError::Limit)
    ));
}

#[test]
fn compact_dtos_reject_unknown_fields_at_every_new_boundary() {
    let mut current = round(1);
    current.status = RoundStatus::Abandoned;
    current.abandonment = Some(Abandonment {
        reason: "Stopped".into(),
        verified_actions: vec![current.actions[0].clone()],
    });
    current.policy_names = vec![PolicyName {
        id: current.assessment.selected_policies()[0].clone(),
        name: "Name".into(),
    }];
    current.policy_declarations[0].policy = current.policy_names[0].clone();
    current.communicated_findings = Some(vec![HistoricalFinding {
        finding: current.assessment.findings()[0].clone(),
        root_note: ProviderId::new("30".into()).unwrap(),
        thread: ProviderId::new("discussion-3".into()).unwrap(),
        actions: vec![PublishedAction::Created],
    }]);
    current.verified_resolutions = vec![ResolutionReceipt {
        subject: current.assessment.subject().clone(),
        reviewer: current.assessment.reviewer().clone(),
        round: 1,
        thread: ProviderId::new("discussion-3".into()).unwrap(),
        resolved: false,
        provenance: ResolutionProvenance::Observed,
    }];
    let value = state(&current, &[]);
    for pointer in [
        "",
        "/subject",
        "/findings/0",
        "/policy_names/0",
        "/current/policy_declarations/0",
        "/current",
        "/current/assessment",
        "/current/actions/0",
        "/current/abandonment",
        "/current/communicated_findings/0",
        "/current/verified_resolutions/0",
    ] {
        let mut invalid = value.clone();
        invalid
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unsupported".into(), true.into());
        assert!(restore(invalid).is_err(), "{pointer}");
    }
}

#[test]
fn mixed_finding_versions_and_historical_names_have_lossless_size_measurements() {
    let mut rounds = (1..=4).map(round).collect::<Vec<_>>();
    for (index, retained) in rounds.iter_mut().enumerate() {
        let status = ["open", "fixed", "withdrawn", "open"][index];
        let mut assessment =
            serde_json::to_value(AssessmentData::from(&retained.assessment)).unwrap();
        assessment["findings"][0]["status"] = status.into();
        assessment["findings"][0]["evidence"] = format!("Verified reassessment {index}").into();
        retained.assessment = serde_json::from_value::<AssessmentData>(assessment)
            .unwrap()
            .into_assessment()
            .unwrap();
        retained.status = RoundStatus::Completed;
        retained.policy_names = vec![PolicyName {
            id: retained.assessment.selected_policies()[0].clone(),
            name: if index == 3 {
                "Renamed policy"
            } else {
                "Original policy"
            }
            .into(),
        }];
        retained.policy_declarations[0].policy = retained.policy_names[0].clone();
        let finding = retained.assessment.findings()[0].clone();
        retained.answers.clear();
        retained.answer_targets.clear();
        retained.transitions.clear();
        if index == 0 {
            retained.actions.truncate(1);
            if let PlannedAction::Finding {
                finding: intended, ..
            } = &mut retained.actions[0]
            {
                **intended = finding.clone();
            }
        } else {
            retained.actions = vec![PlannedAction::Reply {
                finding: finding.id(),
                kind: ReplyKind::Transition(finding.status()),
                body: format!("Verified transition to {status}"),
            }];
        }
        retained.communicated_findings = Some(vec![HistoricalFinding {
            finding,
            root_note: ProviderId::new("30".into()).unwrap(),
            thread: ProviderId::new("discussion-3".into()).unwrap(),
            actions: if index == 0 {
                vec![PublishedAction::Created]
            } else {
                vec![]
            },
        }]);
    }
    let current = rounds.pop().unwrap();
    let compact = rounds::State::from_rounds(&current, &rounds).unwrap();
    let compact_raw = serde_json::to_string(&compact).unwrap();
    let compact_escaped = escaped_json(&compact).unwrap();
    let full = serde_json::json!({
        "current": review_kit_json_adapter::rounds::RoundData::from(&current),
        "history": rounds.iter().map(review_kit_json_adapter::rounds::RoundData::from).collect::<Vec<_>>(),
    });
    let full_raw = serde_json::to_string(&full).unwrap();
    let full_escaped = escaped_json(&full).unwrap();
    let body = encode_summary("Overview", &current, &rounds).unwrap();
    assert_eq!(
        decode_summary(&body).unwrap(),
        Some(("Overview".into(), current, rounds))
    );
    let compact_value = serde_json::to_value(compact).unwrap();
    assert_eq!(compact_value["findings"].as_array().unwrap().len(), 4);
    assert_eq!(compact_value["policy_names"].as_array().unwrap().len(), 2);
    println!(
        "STATE_V3_MEASUREMENT {}",
        serde_json::json!({
            "scenario":"four_round_codec_fixture_with_versions_and_renames",
            "full_wrapper_raw":full_raw.len(), "full_wrapper_escaped":full_escaped.len(),
            "compact_raw":compact_raw.len(), "compact_escaped":compact_escaped.len(),
            "visible_bytes":"Overview".len(), "framing_bytes":body.len()-"Overview".len()-compact_escaped.len(),
            "note_bytes":body.len(),
        })
    );
}
