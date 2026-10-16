//! Current review documents against retained context and publication contracts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use review_kit_application::{
    publication::Effects,
    review_run::{ContentRef, RunId},
    use_cases::checkpoints::{Accepted, ApplicabilityStatus, Successor},
};
use review_kit_json_adapter::{
    ContextDocument, Diagnostic,
    checkpoints::{
        AcceptanceResultDocument, AcceptanceStatus, AssessmentDocument, AssessmentInputs,
        AssessmentRevisionInputs, ReviewRunDocument, RevisionRequest, RunLifecycle, UtcTimestamp,
        check_assessment, revise_assessment,
    },
    decode, encode,
};
use serde_json::{Value, json};

fn fixture(case: &str, file: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(format!(
            "{}/tests/fixtures/reviews/{case}/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap()
}
fn run(value: &Value) -> RunId {
    RunId::try_from(value["run_id"].as_str().unwrap().to_owned()).unwrap()
}
fn context(case: &str) -> ContextDocument {
    serde_json::from_value(fixture(case, "review-context.json")).unwrap()
}
fn complete(
    value: &Value,
    case: &str,
) -> Result<review_kit_application::use_cases::publishing::PublishRequest, Vec<Diagnostic>> {
    check_assessment(
        &serde_json::to_vec(value).unwrap(),
        &AssessmentInputs {
            run_id: &run(value),
            context: &context(case),
            successor: Successor::Current(None),
        },
        Effects::review(),
        false,
    )
}
fn codes(errors: Vec<Diagnostic>) -> Vec<String> {
    errors.into_iter().map(|d| d.code).collect()
}
fn reference(digit: char) -> ContentRef {
    ContentRef::try_from(format!("sha256:{}", digit.to_string().repeat(64))).unwrap()
}
fn schema(file: &str) -> jsonschema::Validator {
    let value: Value = serde_json::from_slice(
        &std::fs::read(format!("{}/schemas/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap(),
    )
    .unwrap();
    jsonschema::validator_for(&value).unwrap()
}

#[test]
fn complete_current_fixtures_preserve_public_declaration_order_and_private_content() {
    for case in ["blocked", "follow-up", "comment"] {
        let value = fixture(case, "review-assessment.json");
        let result = complete(&value, case).unwrap();
        assert!(result.confirmed_validation, "{case}");
        let ids = result
            .policy_declarations
            .iter()
            .map(|row| row.policy.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            value["policies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["policy_id"].as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(result.local_context, value["local_context"]);
        for (decl, row) in result
            .policy_declarations
            .iter()
            .zip(value["policies"].as_array().unwrap())
        {
            assert_eq!(decl.policy.name, row["name"]);
            assert_eq!(decl.reason, row["reason"]);
        }
        let decoded: AssessmentDocument = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decode::<AssessmentDocument>(&encode(&decoded).unwrap()).unwrap())
                .unwrap(),
            value
        );
        assert!(schema("review-assessment-v4.schema.json").is_valid(&value));
    }
}

#[test]
fn one_outcome_distinguishes_unresolved_from_applicable_blocked() {
    let mut value = fixture("blocked", "review-assessment.json");
    let row = value["policies"][0].clone();
    let mut extra = row.clone();
    extra["policy_id"] = json!("missing-definition");
    extra["required"] = json!(false);
    extra["outcome"] = json!({"kind":"unresolved","limitation":"Definition unavailable."});
    value["policies"].as_array_mut().unwrap().push(extra);
    value["policies"][0]["outcome"] =
        json!({"kind":"blocked","limitation":"Regression unavailable."});
    let result = complete(&value, "blocked").unwrap();
    assert_eq!(
        result.policy_declarations[0].status,
        ApplicabilityStatus::Applicable
    );
    assert_eq!(
        result.policy_declarations.last().unwrap().status,
        ApplicabilityStatus::Unresolved
    );
    value["policies"][0]["outcome"] = Value::Null;
    assert!(
        codes(complete(&value, "blocked").unwrap_err())
            .contains(&"incomplete_applicability".into())
    );
}

#[test]
fn initial_document_has_only_captured_revision_and_carried_ids() {
    let ctx = context("follow-up");
    let original = fixture("follow-up", "review-assessment.json");
    let draft = AssessmentDocument::initial(&run(&original), &ctx).unwrap();
    let value = serde_json::to_value(&draft).unwrap();
    assert_eq!(value["revision"], original["revision"]);
    assert_eq!(value["version"], json!(4));
    assert_eq!(
        value["findings"],
        json!([{"id":1,"reusable_confirmation":false,"reassessment":null}])
    );
    assert_eq!(value["confirmations"], json!([]));
    for absent in ["context", "subject", "reviewer", "policy_applicability"] {
        assert!(value.get(absent).is_none());
    }
    assert!(complete(&value, "follow-up").is_err());
}

#[test]
fn unknown_and_omitted_nullable_fields_are_rejected_at_decode_and_schema_boundaries() {
    let original = fixture("follow-up", "review-assessment.json");
    let validator = schema("review-assessment-v4.schema.json");
    for key in [
        "subject",
        "reviewer",
        "context",
        "policy_applicability",
        "private_criteria_copy",
    ] {
        let mut value = original.clone();
        value[key] = json!("not an authored field");
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!validator.is_valid(&value));
    }
    for key in [
        "supersedes",
        "decision",
        "limitation",
        "depth",
        "scope",
        "confirmations",
        "local_context",
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(key);
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!validator.is_valid(&value));
    }
    for key in ["required", "outcome"] {
        let mut value = original.clone();
        value["policies"][0].as_object_mut().unwrap().remove(key);
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!validator.is_valid(&value));
    }
    for (key, retired) in [
        (
            "sources",
            json!(["https://review.example.test/requests/review-policy-list.md"]),
        ),
        ("previous", json!({})),
    ] {
        let mut value = original.clone();
        if key == "sources" {
            value[key] = retired;
        } else {
            value["findings"][0][key] = retired;
        }
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!validator.is_valid(&value));
    }
    for (key, retired) in [("source", json!(null)), ("introduced_by", json!([]))] {
        let mut value = original.clone();
        value["policies"][0][key] = retired;
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!validator.is_valid(&value));
    }
    let mut value = original.clone();
    value["findings"].as_array_mut().unwrap()[0]
        .as_object_mut()
        .unwrap()
        .remove("reassessment");
    assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(!validator.is_valid(&value));
    value = original;
    value["findings"][0]["reassessment"] = json!("kept");
    assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(!validator.is_valid(&value));
}

#[test]
fn outcome_payloads_do_not_admit_contradictory_status_or_judgment_fields() {
    let original = fixture("follow-up", "review-assessment.json");
    for invalid in [
        json!({"kind":"excluded","evidence":"ignored"}),
        json!({"kind":"satisfied","limitation":"ignored"}),
        json!({"kind":"violated"}),
        json!({"kind":"unresolved"}),
        json!({"kind":"unknown"}),
    ] {
        let mut value = original.clone();
        value["policies"][0]["outcome"] = invalid;
        assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(!schema("review-assessment-v4.schema.json").is_valid(&value));
    }
    let mut value = original;
    value["policies"][0]["judgment"] = json!({"kind":"satisfied"});
    assert!(decode::<AssessmentDocument>(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn malformed_wrong_revision_old_format_and_stale_successor_never_complete() {
    let value = fixture("follow-up", "review-assessment.json");
    let ctx = context("follow-up");
    let id = run(&value);
    let input = AssessmentInputs {
        run_id: &id,
        context: &ctx,
        successor: Successor::Current(None),
    };
    let errors = check_assessment(b"{\ninvalid", &input, Effects::review(), false).unwrap_err();
    assert_eq!(errors[0].code, "malformed_draft");
    assert!(errors[0].input.as_ref().unwrap().starts_with("2:"));
    let mut wrong = value.clone();
    wrong["revision"]["head"] = json!("f".repeat(40));
    assert!(
        codes(complete(&wrong, "follow-up").unwrap_err()).contains(&"revision_mismatch".into())
    );
    let other = RunId::try_from("00000000-0000-4000-8000-000000000099".to_owned()).unwrap();
    assert!(
        codes(
            check_assessment(
                &serde_json::to_vec(&value).unwrap(),
                &AssessmentInputs {
                    run_id: &other,
                    ..input
                },
                Effects::review(),
                false
            )
            .unwrap_err()
        )
        .contains(&"run_binding".into())
    );
    wrong = value.clone();
    wrong["version"] = json!(2);
    assert_eq!(
        codes(complete(&wrong, "follow-up").unwrap_err()),
        ["unsupported_version"]
    );
    wrong = value.clone();
    wrong["supersedes"] = json!(reference('a').as_str());
    assert!(codes(complete(&wrong, "follow-up").unwrap_err()).contains(&"stale_successor".into()));
    assert!(
        check_assessment(
            &serde_json::to_vec(&wrong).unwrap(),
            &AssessmentInputs {
                successor: Successor::Retained,
                ..input
            },
            Effects::review(),
            false
        )
        .is_ok()
    );
    wrong["supersedes"] = json!("invalid");
    assert!(
        check_assessment(
            &serde_json::to_vec(&wrong).unwrap(),
            &AssessmentInputs {
                successor: Successor::Retained,
                ..input
            },
            Effects::review(),
            false
        )
        .is_err()
    );
}

#[test]
fn duplicate_rows_and_required_exclusion_fail_with_current_codes() {
    let original = fixture("follow-up", "review-assessment.json");
    let mut value = original.clone();
    let row = value["policies"][0].clone();
    value["policies"].as_array_mut().unwrap().push(row);
    assert!(codes(complete(&value, "follow-up").unwrap_err()).contains(&"duplicate_policy".into()));
    value = original.clone();
    value["policies"][0]["outcome"] = json!({"kind":"excluded"});
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"required_policy_excluded".into())
    );
}

#[test]
fn carried_coverage_and_previous_identity_cannot_be_substituted() {
    let original = fixture("follow-up", "review-assessment.json");
    let mut value = original.clone();
    value["policies"]
        .as_array_mut()
        .unwrap()
        .retain(|row| row["policy_id"] != "rust-invariant-preservation");
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"missing_active_finding_policy".into())
    );
    value = original.clone();
    value["findings"] = json!([]);
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"missing_carried_finding".into())
    );
    value = original.clone();
    let duplicate = value["findings"][0].clone();
    value["findings"].as_array_mut().unwrap().push(duplicate);
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"duplicate_carried_finding".into())
    );
    value = original.clone();
    value["findings"][0]["id"] = json!(99);
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"unknown_carried_finding".into())
    );
    value = original;
    value["findings"][0]["reassessment"]["id"] = json!(99);
    assert!(
        codes(complete(&value, "follow-up").unwrap_err()).contains(&"changed_finding_id".into())
    );
}

#[test]
fn excluded_active_carried_policy_requires_current_closure_and_explanation() {
    let mut value = fixture("follow-up", "review-assessment.json");
    let index = value["policies"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["policy_id"] == "rust-invariant-preservation")
        .unwrap();
    value["policies"][index]["outcome"] = json!({"kind":"excluded"});
    assert!(complete(&value, "follow-up").is_ok());
    let saved = value["transitions"].clone();
    value["transitions"] = json!([]);
    assert!(
        codes(complete(&value, "follow-up").unwrap_err())
            .contains(&"excluded_active_finding".into())
    );
    value["transitions"] = saved;
    value["findings"][0]["reassessment"]["status"] = json!("open");
    value["decision"] = json!("changes_required");
    assert!(complete(&value, "follow-up").is_err());
}

#[test]
fn completion_preserves_exact_reply_targets_and_transition_content() {
    let value = fixture("comment", "review-assessment.json");
    let result = complete(&value, "comment").unwrap();
    assert_eq!(
        result.answers.len(),
        value["answers"].as_array().unwrap().len()
    );
    assert_eq!(result.answer_targets.len(), result.answers.len());
    assert_eq!(
        result.transitions.len(),
        value["transitions"].as_array().unwrap().len()
    );
    for (actual, expected) in result
        .answers
        .iter()
        .zip(value["answers"].as_array().unwrap())
    {
        assert_eq!(actual.in_reply_to.as_str(), expected["in_reply_to"]);
        assert_eq!(actual.body, expected["body"]);
    }
}

#[test]
fn revision_restores_the_generated_reusable_confirmation() {
    let mut value = fixture("blocked", "review-assessment.json");
    value["findings"][0]["reusable_confirmation"] = json!(false);
    let ctx = context("blocked");
    let id = run(&value);
    let revised = revise_assessment(
        Some(&serde_json::to_vec(&value).unwrap()),
        &AssessmentRevisionInputs {
            run_id: &id,
            context: &ctx,
            accepted: None,
        },
        RevisionRequest {
            reason: "Correct the generated field",
            regenerate: false,
        },
    )
    .unwrap();
    let result = serde_json::to_value(&revised.document).unwrap();
    assert_eq!(result["findings"][0]["reusable_confirmation"], true);
    assert_eq!(
        result["findings"][0]["reassessment"]["id"],
        value["findings"][0]["reassessment"]["id"]
    );
}

#[test]
fn revision_preserves_relevance_but_resets_substantive_outcomes_and_every_validation_flag() {
    let mut value = fixture("blocked", "review-assessment.json");
    value["policies"][0]["outcome"] =
        json!({"kind":"unresolved","limitation":"Definition is currently unavailable."});
    let document: AssessmentDocument = serde_json::from_value(value.clone()).unwrap();
    let accepted = Accepted {
        reference: reference('a'),
        content: document,
    };
    let ctx = context("blocked");
    let id = run(&value);
    let revised = revise_assessment(
        None,
        &AssessmentRevisionInputs {
            run_id: &id,
            context: &ctx,
            accepted: Some(&accepted),
        },
        RevisionRequest {
            reason: "Renew current review",
            regenerate: false,
        },
    )
    .unwrap();
    let result = serde_json::to_value(&revised.document).unwrap();
    assert_eq!(result["supersedes"], accepted.reference.as_str());
    assert_eq!(result["revision"], value["revision"]);
    assert_eq!(result["decision"], Value::Null);
    for (row, prior) in result["policies"]
        .as_array()
        .unwrap()
        .iter()
        .zip(value["policies"].as_array().unwrap())
    {
        let kind = prior["outcome"]["kind"].as_str().unwrap();
        assert_eq!(
            row["outcome"],
            if ["excluded", "unresolved"].contains(&kind) {
                prior["outcome"].clone()
            } else {
                Value::Null
            }
        );
        assert_eq!(row["reason"], prior["reason"]);
    }
    assert!(
        result["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["reassessment"].is_null() || row["reassessment"]["validated"] == false)
    );
    assert!(
        result["new_findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["validated"] == false)
    );
    assert_eq!(result["confirmations"], json!([]));
    assert_eq!(result["local_context"], value["local_context"]);
    let mut kept = value.clone();
    kept["findings"][0]["reassessment"] = json!("unchanged");
    let revised = revise_assessment(
        Some(&serde_json::to_vec(&kept).unwrap()),
        &AssessmentRevisionInputs {
            run_id: &id,
            context: &ctx,
            accepted: Some(&accepted),
        },
        RevisionRequest {
            reason: "Renew current review",
            regenerate: false,
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&revised.document).unwrap()["findings"][0]["reassessment"],
        json!("unchanged")
    );
}

#[test]
fn regeneration_retains_accepted_facts_and_reports_kept_incompatible_candidates() {
    let value = fixture("blocked", "review-assessment.json");
    let accepted = Accepted {
        reference: reference('a'),
        content: serde_json::from_value(value.clone()).unwrap(),
    };
    let ctx = context("blocked");
    let id = run(&value);
    let inputs = AssessmentRevisionInputs {
        run_id: &id,
        context: &ctx,
        accepted: Some(&accepted),
    };
    assert!(
        revise_assessment(
            Some(b"malformed"),
            &inputs,
            RevisionRequest {
                reason: "Renew",
                regenerate: false
            }
        )
        .is_err()
    );
    let revised = revise_assessment(
        Some(b"malformed"),
        &inputs,
        RevisionRequest {
            reason: "Renew",
            regenerate: true,
        },
    )
    .unwrap();
    assert_eq!(revised.document.policy_set, accepted.content.policy_set);
    assert!(
        revise_assessment(
            None,
            &inputs,
            RevisionRequest {
                reason: " ",
                regenerate: true
            }
        )
        .is_err()
    );
    let mut edited = value.clone();
    edited["findings"][0]["id"] = json!(999);
    let revised = revise_assessment(
        Some(&serde_json::to_vec(&edited).unwrap()),
        &inputs,
        RevisionRequest {
            reason: "Keep correction candidates",
            regenerate: false,
        },
    )
    .unwrap();
    assert!(
        revised
            .diagnostics
            .iter()
            .any(|d| d.code == "unknown_carried_finding")
    );
    assert!(revised.document.findings.iter().any(|row| row.id == 999));
}

fn manifest() -> Value {
    let ctx = fixture("follow-up", "review-context.json");
    json!({"version":1,"kind":"review_run","run_id":"00000000-0000-4000-8000-000000000002","subject":ctx["snapshot"]["subject"],"reviewer":"7","revision":ctx["snapshot"]["revision"],"context":"review-context.json","created_at":"2026-10-13T12:00:00Z","status":"open","completion":null,"reason":null,"snapshot_files":{"review-context.json":reference('a').as_str(),"reviewed-source.json":reference('b').as_str(),"merge-request.diff":reference('c').as_str(),"review-conversation.json":reference('d').as_str()},"review_assessment":null,"publication_attempted":false})
}

#[test]
fn manifest_has_one_accepted_reference_and_enforces_lifecycle_and_hash_shapes() {
    let mut value = manifest();
    let decoded: ReviewRunDocument = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), value);
    assert!(schema("review-run-v1.schema.json").is_valid(&value));
    value["review_assessment"] = json!(reference('e').as_str());
    assert!(serde_json::from_value::<ReviewRunDocument>(value.clone()).is_ok());
    for (status, completion, reason, valid) in [
        ("open", Value::Null, Value::Null, true),
        ("open", json!("published"), Value::Null, false),
        ("stopped", Value::Null, json!("Stop locally"), true),
        ("stopped", Value::Null, Value::Null, false),
        ("completed", json!("local_only"), Value::Null, true),
        ("completed", json!("abandoned"), Value::Null, false),
        (
            "completed",
            json!("abandoned"),
            json!("Abandon current review"),
            true,
        ),
    ] {
        let mut case = value.clone();
        case["status"] = json!(status);
        case["completion"] = completion;
        case["reason"] = reason;
        assert_eq!(
            serde_json::from_value::<ReviewRunDocument>(case).is_ok(),
            valid
        );
    }
    for key in ["run_id", "review_assessment", "created_at"] {
        let mut invalid = value.clone();
        invalid[key] = json!("invalid");
        assert!(serde_json::from_value::<ReviewRunDocument>(invalid).is_err());
    }
    value["checkpoints"] = json!({});
    assert!(serde_json::from_value::<ReviewRunDocument>(value).is_err());
    let mut invalid = decoded;
    invalid.lifecycle = RunLifecycle::Stopped { reason: " ".into() };
    assert!(encode(&invalid).is_err());
}

#[test]
fn receipt_uses_one_snapshot_path_and_rejects_inconsistent_paths() {
    let receipt = AcceptanceResultDocument {
        run_id: run(&fixture("follow-up", "review-assessment.json")),
        reference: reference('a'),
        status: AcceptanceStatus::Stored,
    };
    let mut value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        value["path"],
        format!("accepted/{}/review-assessment.json", "a".repeat(64))
    );
    assert!(value.get("stage").is_none());
    assert_eq!(
        decode::<AcceptanceResultDocument>(&encode(&receipt).unwrap()).unwrap(),
        receipt
    );
    assert!(schema("checkpoint-acceptance-result-v1.schema.json").is_valid(&value));
    value["path"] = json!("accepted/wrong/review-assessment.json");
    assert!(serde_json::from_value::<AcceptanceResultDocument>(value).is_err());
}

#[test]
fn utc_timestamps_require_the_current_utc_representation() {
    for value in ["2026-10-13T12:00:00Z", "2026-10-13T12:00:00.123Z"] {
        assert!(UtcTimestamp::try_from(value.to_owned()).is_ok());
    }
    for value in [
        "2026-00-13T12:00:00Z",
        "2026-10-00T12:00:00Z",
        "2026-10-13T25:00:00Z",
        "2026-10-13T12:00:00+00:00",
        "2026-10-13T12:00:00.Z",
    ] {
        assert!(UtcTimestamp::try_from(value.to_owned()).is_err());
    }
}

#[test]
fn declaration_and_completion_errors_are_collected_in_one_result() {
    let mut value = fixture("follow-up", "review-assessment.json");
    value["supersedes"] = json!(reference('e').as_str());
    value["decision"] = Value::Null;
    value["findings"] = json!([]);
    let diagnostics = codes(complete(&value, "follow-up").unwrap_err());
    assert!(
        diagnostics.contains(&"stale_successor".into()),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.contains(&"missing_carried_finding".into()),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics.contains(&"missing_decision".into()),
        "{diagnostics:?}"
    );
}

fn admitted(result: &review_kit_application::use_cases::publishing::PublishRequest) -> bool {
    result
        .assessment
        .findings()
        .iter()
        .find(|finding| finding.id().get() == 1)
        .unwrap()
        .validated()
}

#[test]
fn an_unchanged_decision_reuses_the_captured_version_and_needs_a_current_confirmation() {
    let original = fixture("blocked", "review-assessment.json");
    let validator = schema("review-assessment-v4.schema.json");
    let mut value = original.clone();
    value["findings"][0]["reassessment"] = json!("unchanged");
    assert!(validator.is_valid(&value));
    assert!(admitted(&complete(&value, "blocked").unwrap()));
    value["confirmations"] = json!([]);
    assert!(!admitted(&complete(&value, "blocked").unwrap()));
    value["confirmations"] = json!([{
        "source": "retained",
        "finding": 1,
        "basis": "The earlier isolated check covers this unchanged version."
    }]);
    assert!(validator.is_valid(&value));
    assert!(admitted(&complete(&value, "blocked").unwrap()));
    value["findings"][0]["reassessment"] = original["findings"][0]["reassessment"].clone();
    value["findings"][0]["reassessment"]["body"] = json!("A changed explanation of the defect.");
    value["findings"][0]["reassessment"]["validated"] = json!(false);
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["invalid_retained_confirmation"]
    );
}

#[test]
fn a_retained_confirmation_needs_a_round_that_recorded_confirmations() {
    let mut value = fixture("blocked", "review-assessment.json");
    value["findings"][0]["reassessment"] = json!("unchanged");
    value["confirmations"] = json!([{
        "source": "retained",
        "finding": 1,
        "basis": "The recorded confirmation covers this unchanged version."
    }]);
    let mut unmarked = fixture("blocked", "review-context.json");
    unmarked["snapshot"]["summaries"][0]["current"]
        .as_object_mut()
        .unwrap()
        .remove("confirmed_validation");
    let unmarked: ContextDocument = serde_json::from_value(unmarked).unwrap();
    let check = |value: &Value, context: &ContextDocument| {
        check_assessment(
            &serde_json::to_vec(value).unwrap(),
            &AssessmentInputs {
                run_id: &run(value),
                context,
                successor: Successor::Current(None),
            },
            Effects::review(),
            false,
        )
    };
    assert!(admitted(&check(&value, &context("blocked")).unwrap()));
    assert_eq!(
        codes(check(&value, &unmarked).unwrap_err()),
        [
            "changed_reusable_confirmation",
            "invalid_retained_confirmation"
        ]
    );
    value["findings"][0]["reusable_confirmation"] = json!(false);
    assert_eq!(
        codes(check(&value, &unmarked).unwrap_err()),
        ["invalid_retained_confirmation"]
    );
    assert_eq!(
        codes(check(&value, &context("blocked")).unwrap_err()),
        ["changed_reusable_confirmation"]
    );
}

#[test]
fn blocking_validation_follows_an_exact_checker_confirmation() {
    let original = fixture("blocked", "review-assessment.json");
    let mut value = original.clone();
    value["findings"][0]["reassessment"]["validated"] = json!(false);
    assert!(admitted(&complete(&value, "blocked").unwrap()));
    value["confirmations"][0]["checked"]["severity"] = json!("p3");
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["stale_confirmation"]
    );
    value["confirmations"][0]["checked"] = original["confirmations"][0]["checked"].clone();
    value["confirmations"][0]["result"] = json!("rejected");
    assert!(!admitted(&complete(&value, "blocked").unwrap()));
    value = original.clone();
    value["confirmations"] = json!([]);
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["unconfirmed_blocking_finding"]
    );
    value = original.clone();
    value["confirmations"][0]["checked"]["title"] = json!(" ");
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["invalid_confirmation", "unconfirmed_blocking_finding"]
    );
    value = original;
    value["confirmations"][0]["basis"] = json!("");
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["blank_confirmation_basis"]
    );
}

#[test]
fn private_context_is_limited_to_two_thousand_characters() {
    let mut value = fixture("follow-up", "review-assessment.json");
    value["local_context"] = json!("é".repeat(2000));
    assert!(complete(&value, "follow-up").is_ok());
    assert!(schema("review-assessment-v4.schema.json").is_valid(&value));
    value["local_context"] = json!("é".repeat(2001));
    assert_eq!(
        codes(complete(&value, "follow-up").unwrap_err()),
        ["local_context_too_long"]
    );
    assert!(!schema("review-assessment-v4.schema.json").is_valid(&value));
}

#[test]
fn only_a_confirmed_verdict_must_match_and_name_its_checked_finding() {
    let original = fixture("blocked", "review-assessment.json");
    let mut value = original.clone();
    value["findings"][0]["reassessment"]["validated"] = json!(false);
    value["findings"][0]["reassessment"]["body"] = json!("A reworded explanation of the defect.");
    value["confirmations"][0]["result"] = json!("rejected");
    assert!(!admitted(&complete(&value, "blocked").unwrap()));
    value["confirmations"][0]["finding"] = json!(7);
    assert_eq!(
        codes(complete(&value, "blocked").unwrap_err()),
        ["invalid_confirmation"]
    );
}

#[test]
fn a_captured_context_with_confirmation_marked_rounds_matches_its_schema() {
    let validator = schema("review-input-v1.schema.json");
    let context = fixture("blocked", "review-context.json");
    assert_eq!(
        context["snapshot"]["summaries"][0]["current"]["confirmed_validation"],
        true
    );
    let errors: Vec<String> = validator
        .iter_errors(&context)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}
