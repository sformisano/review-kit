#![allow(clippy::unwrap_used)]

use review_kit_application::{
    publication::{Effects, PolicyName, Snapshot, Write},
    use_cases::{
        assessment::{AssessmentPolicies, prepare},
        discovery_bundle::ObservedChanges,
        initialization::InitializedAssessment,
        publishing::{PublicationProgress, UnresolvedWrite},
    },
};
use review_kit_domain::{PolicyId, ReviewerId, Revision, Subject};
use review_kit_json_adapter::{
    ContextDocument, DraftDocument, JsonError, ProgressDocument, decode,
    domain::{Decision, Judgment},
    encode,
};

fn fixture() -> (ContextDocument, AssessmentPolicies, DraftDocument) {
    let subject = Subject::new(
        "gitlab".into(),
        "https://example.test".into(),
        "team/project".into(),
        1,
    )
    .unwrap();
    let reviewer = ReviewerId::try_from("9".to_owned()).unwrap();
    let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
    let snapshot = Snapshot {
        subject: subject.clone(),
        reviewer: reviewer.clone(),
        revision: revision.clone(),
        summaries: Vec::new(),
        findings: Vec::new(),
        replies: Vec::new(),
        permissions: Effects::review(),
    };
    let changes = ObservedChanges {
        subject,
        revision,
        files: Vec::new(),
        unified_diff: String::new(),
    };
    let context = ContextDocument::new(&snapshot, &changes);
    let proof = PolicyId::try_from("proof".to_owned()).unwrap();
    let set = AssessmentPolicies::new(
        "rust".into(),
        vec![PolicyName {
            id: proof.clone(),
            name: "Proof".into(),
        }],
    )
    .unwrap();
    let initialized = InitializedAssessment {
        template: prepare(snapshot, reviewer, set.clone()).unwrap(),
    };
    let draft = DraftDocument::from_initialized(&initialized, "discovery/one/context.json");
    (context, set, draft)
}

#[test]
fn evidence_preserves_per_write_renderer_identity_and_reads_unattributed_events() {
    let (context, _, _) = fixture();
    let mut document = serde_json::json!({
        "version": 1, "kind": "evidence", "subject": context.snapshot.subject,
        "reviewer": "9", "events": [{
            "kind": "before_write", "write": {"kind": "set_resolved", "thread": "root", "resolved": true}
        }]
    });
    let legacy: review_kit_json_adapter::EvidenceDocument =
        decode(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(
        serde_json::to_value(legacy).unwrap()["events"][0]
            .get("renderer")
            .is_none()
    );
    for renderer in ["profile/v1", "profile/v2"] {
        let mut event = document["events"][0].clone();
        event["renderer"] = renderer.into();
        document["events"].as_array_mut().unwrap().push(event);
    }
    let restored: review_kit_json_adapter::EvidenceDocument =
        decode(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), document);
}

#[test]
fn initialized_draft_retains_unassessed_inputs_and_collects_repairs() {
    let (context, _set, draft) = fixture();
    let draft: DraftDocument = decode(&encode(&draft).unwrap()).unwrap();
    let error = draft
        .complete(&context, Effects::review(), false)
        .err()
        .unwrap();
    let codes: Vec<_> = error
        .diagnostics()
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    for expected in [
        "missing_decision",
        "missing_depth",
        "missing_scope",
        "missing_policy_judgment",
    ] {
        assert!(codes.iter().any(|code| code == expected));
    }
}

#[test]
fn completed_draft_uses_declared_coverage_and_excludes_private_context() {
    let (context, _set, mut draft) = fixture();
    draft.decision = Some(Decision::Ready);
    draft.depth = Some("complete".into());
    draft.scope = Some("changed source".into());
    draft.policies[0].judgment = Some(Judgment::Satisfied {});
    draft.local_context = "private context sentinel".into();
    for debug in [format!("{draft:?}"), format!("{draft:#?}")] {
        assert!(debug.contains("private context sentinel"));
        assert!(debug.contains("policy_set"));
    }
    let retained: DraftDocument = decode(&encode(&draft).unwrap()).unwrap();
    assert_eq!(retained.local_context, "private context sentinel");
    let request = draft.complete(&context, Effects::review(), false).unwrap();
    assert_eq!(request.local_context, "private context sentinel");
    let declarations = request
        .policies
        .policies()
        .iter()
        .map(
            |policy| review_kit_application::publication::PublicPolicyDeclaration {
                policy: policy.clone(),
                status:
                    review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
                reason: "The change affects this policy.".into(),
            },
        )
        .collect();
    assert!(
        !request
            .clone()
            .with_policy_declarations(declarations)
            .unwrap()
            .confirmed_validation,
        "draft completion keeps authored validation, so its rounds record no confirmations"
    );
    let public = serde_json::to_string(&review_kit_json_adapter::domain::AssessmentData::from(
        &request.assessment,
    ))
    .unwrap();
    assert!(!public.contains("private context sentinel"));
    draft.policies.clear();
    let empty = draft.complete(&context, Effects::review(), false).unwrap();
    assert!(empty.assessment.selected_policies().is_empty());
}

#[test]
fn context_binding_versions_and_unknown_fields_reject_input() {
    let (mut context, _set, mut draft) = fixture();
    context.version = 2;
    assert!(matches!(context.validate(), Err(JsonError::Version(2))));
    context.version = 1;
    draft.reviewer = "10".into();
    assert!(matches!(
        draft.complete(&context, Effects::none(), false),
        Err(JsonError::ContextBinding)
    ));
    draft.context = "../other/context.json".into();
    assert!(matches!(
        draft.validate_reference(),
        Err(JsonError::ContextBinding)
    ));
    let mut value = serde_json::to_value(&context).unwrap();
    value["snapshot"]["unexpected"] = true.into();
    assert!(serde_json::from_value::<ContextDocument>(value).is_err());
    let judgment = serde_json::json!({"kind":"satisfied", "evidence":"invented"});
    assert!(serde_json::from_value::<Judgment>(judgment).is_err());
}

#[test]
fn progress_round_trip_preserves_exact_unresolved_write_and_binding() {
    let (context, _, _) = fixture();
    let snapshot = context.bundle().unwrap().snapshot().clone();
    let write = Write::SetResolved {
        thread: review_kit_application::publication::ProviderId::new("discussion".into()).unwrap(),
        resolved: true,
    };
    let progress = PublicationProgress {
        verified_resolutions: vec![review_kit_application::publication::ResolutionReceipt {
            provenance: Default::default(),
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            round: 2,
            thread: review_kit_application::publication::ProviderId::new("discussion".into())
                .unwrap(),
            resolved: true,
        }],
        unresolved: Some(UnresolvedWrite {
            renderer: Some(review_kit_application::ports::RenderProfileIdentity {
                id: "custom".into(),
                input_contract: "review-kit-render-input/v1".into(),
                fingerprint: "exact-retained-profile".into(),
            }),
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            write,
            phase: Default::default(),
            baseline: None,
        }),
        ..Default::default()
    };
    let document = ProgressDocument::new(&snapshot.subject, &snapshot.reviewer, &progress);
    let restored: ProgressDocument = decode(&encode(&document).unwrap()).unwrap();
    assert_eq!(
        restored
            .clone()
            .into_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap(),
        progress
    );
    let other = ReviewerId::try_from("different".to_owned()).unwrap();
    assert!(matches!(
        restored.into_progress(&snapshot.subject, &other),
        Err(JsonError::ContextBinding)
    ));
}

#[test]
fn incomplete_assessment_reports_every_invalid_reply_with_its_input() {
    let (context, _set, mut draft) = fixture();
    draft.answers = serde_json::from_value(serde_json::json!([
        {"finding":1,"in_reply_to":"note-one","body":" "},
        {"finding":2,"in_reply_to":"","body":"Answer"}
    ]))
    .unwrap();
    draft.transitions = serde_json::from_value(serde_json::json!([
        {"finding":3,"from":"open","to":"fixed","body":""},
        {"finding":4,"from":"fixed","to":"open","body":" "}
    ]))
    .unwrap();
    let diagnostics = draft
        .complete(&context, Effects::none(), false)
        .err()
        .unwrap()
        .diagnostics();
    for (code, count) in [
        ("missing_decision", 1),
        ("missing_policy_judgment", 1),
        ("invalid_answer", 2),
        ("invalid_transition", 2),
    ] {
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == code)
                .count(),
            count
        );
    }
    for (input, finding) in [
        ("answers[0]", 1),
        ("answers[1]", 2),
        ("transitions[0]", 3),
        ("transitions[1]", 4),
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.input.as_deref() == Some(input)
                    && diagnostic.finding == Some(finding))
        );
    }
}

#[test]
fn invalid_new_findings_keep_constructor_causes_and_independent_repairs() {
    let (context, _set, mut draft) = fixture();
    let finding = serde_json::json!({"id":1,"policy_id":"proof","title":"","body":"Finding","severity":"p2","disposition":"blocking","status":"open","evidence":"Evidence","validated":true,"location":{"path":"src.rs","line":1},"suggestion":null});
    let mut second = finding.clone();
    second["id"] = 2.into();
    second["title"] = "Title".into();
    second["evidence"] = "".into();
    draft.new_findings = serde_json::from_value(serde_json::json!([finding, second])).unwrap();
    let diagnostics = draft
        .complete(&context, Effects::none(), false)
        .err()
        .unwrap()
        .diagnostics();
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "invalid_new_finding")
            .count(),
        2
    );
    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic.input.as_deref() == Some("new_findings[0]")
                && diagnostic.message.contains("title")
        )
    );
    assert!(
        diagnostics.iter().any(
            |diagnostic| diagnostic.input.as_deref() == Some("new_findings[1]")
                && diagnostic.message.contains("evidence")
        )
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_decision")
    );
}

#[test]
fn shipped_inputs_decode_and_preserve_unassessed_judgments() {
    let context: ContextDocument = decode(include_bytes!("../examples/context.json")).unwrap();
    let draft: DraftDocument = decode(include_bytes!("../examples/assessment.json")).unwrap();
    context.validate().unwrap();
    assert!(draft.complete(&context, Effects::none(), false).is_err());
    assert!(draft.decision.is_none());
    assert!(draft.policies[0].judgment.is_none());
}

#[test]
fn independently_retained_v1_progress_stays_ambiguous_without_an_invented_grant() {
    let bytes = br#"{
      "version":1,"kind":"progress",
      "subject":{"provider":"gitlab","host":"https://example.test","repository":"team/project","request":1},
      "reviewer":"9",
      "unresolved":{"subject":{"provider":"gitlab","host":"https://example.test","repository":"team/project","request":1},
        "reviewer":"9","write":{"kind":"set_resolved","thread":"retained-root","resolved":true}},
      "verified_resolutions":[{"subject":{"provider":"gitlab","host":"https://example.test","repository":"team/project","request":1},
        "reviewer":"9","round":1,"thread":"previous-root","resolved":true}]
    }"#;
    let (context, _, _) = fixture();
    let snapshot = context.bundle().unwrap().snapshot().clone();
    let retained: ProgressDocument = decode(bytes).unwrap();
    let restored = retained
        .into_progress(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    let pending = restored.unresolved.as_ref().unwrap();
    assert_eq!(
        pending.phase,
        review_kit_application::use_cases::publishing::WritePhase::LegacyAmbiguous
    );
    assert!(pending.baseline.is_none());
    assert!(restored.authorization.is_none());
    assert!(restored.retried_creations.is_empty());
    assert_eq!(
        restored.verified_resolutions[0].provenance,
        review_kit_application::publication::ResolutionProvenance::Acknowledged
    );
    let upgraded = ProgressDocument::new(&snapshot.subject, &snapshot.reviewer, &restored);
    assert_eq!(upgraded.version, 2);
    assert_eq!(
        decode::<ProgressDocument>(&encode(&upgraded).unwrap())
            .unwrap()
            .into_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap(),
        restored
    );
}

fn complete_progress_fixture() -> (Snapshot, PublicationProgress) {
    use review_kit_application::{
        publication::*,
        use_cases::publishing::{RoundAuthorization, WritePhase},
    };
    let (context, _set, mut draft) = fixture();
    draft.decision = Some(Decision::Ready);
    draft.depth = Some("complete".into());
    draft.scope = Some("changed source".into());
    draft.policies[0].judgment = Some(Judgment::Satisfied {});
    let request = draft.complete(&context, Effects::review(), false).unwrap();
    let snapshot = context.bundle().unwrap().snapshot().clone();
    let round = Round {
        confirmed_validation: false,
        number: 1,
        assessment: request.assessment,
        status: RoundStatus::Pending,
        abandonment: None,
        publication_limitation: None,
        answers: Vec::new(),
        transitions: Vec::new(),
        answer_targets: Vec::new(),
        actions: Vec::new(),
        review_workflow: None,
        policy_set: Some("rust".into()),
        policy_names: request.policies.policies().to_vec(),
        policy_declarations: request
            .policies
            .policies()
            .iter()
            .map(|policy| PublicPolicyDeclaration {
                policy: policy.clone(),
                status:
                    review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
                reason: "The changed source requires this policy.".into(),
            })
            .collect(),
        renderer: None,
        communicated_findings: Some(Vec::new()),
        verified_resolutions: Vec::new(),
    };
    let write = Write::Summary {
        expected: None,
        round: Box::new(round),
        history: Vec::new(),
        body: "Pending review.".into(),
    };
    let progress = PublicationProgress {
        unresolved: Some(UnresolvedWrite {
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            write: write.clone(),
            renderer: None,
            phase: WritePhase::DispatchPossible,
            baseline: Some(snapshot.clone()),
        }),
        authorization: Some(RoundAuthorization {
            round: 1,
            effects: Effects::review(),
        }),
        retried_creations: vec![write],
        verified_resolutions: vec![ResolutionReceipt {
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            round: 1,
            thread: ProviderId::new("earlier-thread".into()).unwrap(),
            resolved: true,
            provenance: ResolutionProvenance::Observed,
        }],
        ..Default::default()
    };
    (snapshot, progress)
}

#[test]
fn v2_restores_exact_baseline_original_grant_retry_budget_and_observed_provenance() {
    let (snapshot, progress) = complete_progress_fixture();
    let document = ProgressDocument::new(&snapshot.subject, &snapshot.reviewer, &progress);
    assert_eq!(document.version, 2);
    let restored: ProgressDocument = decode(&encode(&document).unwrap()).unwrap();
    assert_eq!(
        restored
            .into_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap(),
        progress
    );
}

#[test]
fn v2_rejects_incomplete_or_misbound_preparation_and_malformed_retry_budgets() {
    let (snapshot, progress) = complete_progress_fixture();
    let value = serde_json::to_value(ProgressDocument::new(
        &snapshot.subject,
        &snapshot.reviewer,
        &progress,
    ))
    .unwrap();
    for defect in [
        "baseline",
        "grant",
        "reviewer",
        "round",
        "retry_duplicate",
        "retry_update",
        "legacy_version",
    ] {
        let mut invalid = value.clone();
        match defect {
            "baseline" => invalid["unresolved"]["baseline"] = serde_json::Value::Null,
            "grant" => invalid["authorization"] = serde_json::Value::Null,
            "reviewer" => invalid["unresolved"]["baseline"]["reviewer"] = "another-reviewer".into(),
            "round" => invalid["authorization"]["round"] = 2.into(),
            "retry_duplicate" => {
                let repeated = invalid["retried_creations"][0].clone();
                invalid["retried_creations"]
                    .as_array_mut()
                    .unwrap()
                    .push(repeated);
            }
            "retry_update" => {
                invalid["retried_creations"][0] =
                    serde_json::json!({"kind":"set_resolved","thread":"root","resolved":true})
            }
            _ => invalid["version"] = 1.into(),
        }
        let decoded: ProgressDocument = decode(&serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            decoded
                .into_progress(&snapshot.subject, &snapshot.reviewer)
                .is_err(),
            "{defect}"
        );
    }
}

#[test]
fn prepared_abandonment_decoder_preserves_the_exact_round_inputs() {
    use review_kit_application::publication::{
        Abandonment, PolicyName, ProviderId, RoundStatus, SummaryRecord,
    };
    use review_kit_application::use_cases::publishing::WritePhase;
    let (mut snapshot, mut progress) = complete_progress_fixture();
    let original = match &progress.unresolved.as_ref().unwrap().write {
        Write::Summary { round, .. } => Some(round.clone()),
        _ => None,
    }
    .unwrap();
    let mut original = original;
    original.policy_names = vec![PolicyName {
        id: original.assessment.selected_policies()[0].clone(),
        name: "Proof".into(),
    }];
    let expected = SummaryRecord {
        id: ProviderId::new("retained-summary".into()).unwrap(),
        author: snapshot.reviewer.clone(),
        body: "Pending review.".into(),
        current: *original.clone(),
        history: Vec::new(),
    };
    snapshot.summaries.push(expected.clone());
    let mut closed = original;
    closed.status = RoundStatus::Abandoned;
    closed.abandonment = Some(Abandonment {
        reason: "Close this exact round.".into(),
        verified_actions: Vec::new(),
    });
    let pending = progress.unresolved.as_mut().unwrap();
    pending.write = Write::Summary {
        expected: Some(Box::new(expected)),
        round: closed,
        history: Vec::new(),
        body: "Abandoned review.".into(),
    };
    pending.baseline = Some(snapshot.clone());
    pending.phase = WritePhase::Prepared;
    progress.authorization = None;
    progress.retried_creations.clear();
    progress.verified_resolutions.clear();
    let value = serde_json::to_value(ProgressDocument::new(
        &snapshot.subject,
        &snapshot.reviewer,
        &progress,
    ))
    .unwrap();
    let restored: ProgressDocument = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        restored
            .into_progress(&snapshot.subject, &snapshot.reviewer)
            .unwrap(),
        progress
    );
    for defect in ["assessment", "actions", "workflow"] {
        let mut altered = value.clone();
        match defect {
            "assessment" => {
                altered["unresolved"]["write"]["round"]["assessment"]["scope"] =
                    "Different scope.".into()
            }
            "actions" => {
                altered["unresolved"]["write"]["round"]["actions"] =
                    serde_json::json!([{"kind":"set_resolved","finding":1,"resolved":true}])
            }
            _ => {
                altered["unresolved"]["write"]["round"]["review_workflow"] =
                    "another-workflow".into()
            }
        }
        let decoded: ProgressDocument = decode(&serde_json::to_vec(&altered).unwrap()).unwrap();
        assert!(
            decoded
                .into_progress(&snapshot.subject, &snapshot.reviewer)
                .is_err(),
            "{defect}"
        );
    }
}

#[test]
fn agent_policy_rows_supply_names_and_coverage_without_a_catalog() {
    let (context, _, mut draft) = fixture();
    draft.decision = Some(Decision::Ready);
    draft.depth = Some("Changed source".into());
    draft.scope = Some("Agent-declared coverage".into());
    draft.policies[0].policy_id = "agent-only-rule".into();
    draft.policies[0].name = Some("Agent Rule".into());
    draft.policies[0].judgment = Some(Judgment::Satisfied {});
    let request = draft.complete(&context, Effects::none(), false).unwrap();
    assert_eq!(request.policies.name(), "rust");
    assert_eq!(request.policies.ids()[0].as_str(), "agent-only-rule");
    assert_eq!(request.policies.policies()[0].name, "Agent Rule");
    assert_eq!(
        request.assessment.selected_policies(),
        request.policies.ids()
    );
    draft.policies[0].judgment = None;
    assert!(
        draft
            .complete(&context, Effects::none(), false)
            .unwrap_err()
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "missing_policy_judgment")
    );
}

#[test]
fn v1_cli_draft_completes_without_policy_definitions() {
    let context: ContextDocument = decode(include_bytes!("fixtures/context-v1.json")).unwrap();
    let mut draft: DraftDocument = decode(include_bytes!("fixtures/assessment-v1.json")).unwrap();
    assert!(draft.complete(&context, Effects::none(), false).is_err());
    draft.decision = Some(Decision::Ready);
    draft.depth = Some("Changed source".into());
    draft.scope = Some("Retained policy coverage".into());
    for policy in &mut draft.policies {
        assert!(policy.name.is_none());
        policy.judgment = Some(Judgment::Satisfied {});
    }
    let request = draft.complete(&context, Effects::none(), false).unwrap();
    assert_eq!(
        request
            .policies
            .policies()
            .iter()
            .map(|policy| (policy.id.as_str(), policy.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("baseline-integrity-policy", "baseline-integrity-policy"),
            ("baseline-proof-policy", "baseline-proof-policy"),
        ]
    );
    assert!(
        serde_json::to_value(draft)
            .unwrap()
            .get("requested_policies")
            .is_none()
    );
}

#[test]
fn legacy_requested_ids_are_ignored_and_not_written() {
    let (context, _, mut draft) = fixture();
    draft.decision = Some(Decision::Ready);
    draft.depth = Some("Changed source".into());
    draft.scope = Some("Agent-declared coverage".into());
    draft.policies[0].name = None;
    draft.policies[0].judgment = Some(Judgment::Satisfied {});
    let mut wire = serde_json::to_value(&draft).unwrap();
    wire["requested_policies"] = serde_json::json!(["absent-unselected-rule"]);
    let restored: DraftDocument = serde_json::from_value(wire).unwrap();
    let request = restored.complete(&context, Effects::none(), false).unwrap();
    assert_eq!(request.policies.ids()[0].as_str(), "proof");
    assert_eq!(request.policies.policies()[0].name, "proof");
    assert!(
        serde_json::to_value(restored)
            .unwrap()
            .get("requested_policies")
            .is_none()
    );
}

#[test]
fn policy_metadata_and_gap_constraints_survive_json_completion() {
    let (context, _, mut draft) = fixture();
    draft.decision = Some(Decision::Blocked);
    draft.depth = Some("Changed source".into());
    draft.scope = Some("Agent-declared coverage".into());
    draft.limitation = Some("Missing test evidence".into());
    draft.policies[0].judgment = Some(Judgment::Blocked {
        limitation: "Missing test evidence".into(),
    });
    assert!(draft.complete(&context, Effects::none(), false).is_ok());
    draft.decision = Some(Decision::Ready);
    assert!(draft.complete(&context, Effects::none(), false).is_err());
    draft.decision = Some(Decision::Blocked);
    draft.policies[0].name = Some(" ".into());
    assert!(matches!(
        draft.complete(&context, Effects::none(), false),
        Err(JsonError::Policies(_))
    ));
    draft.policies[0].name = Some("Proof".into());
    draft.policies.push(draft.policies[0].clone());
    assert!(matches!(
        draft.complete(&context, Effects::none(), false),
        Err(JsonError::Policies(_))
    ));
    draft.policies.pop();
    draft.policies[0].policy_id = "\t".into();
    assert!(matches!(
        draft.complete(&context, Effects::none(), false),
        Err(JsonError::InvalidValue)
    ));
}
