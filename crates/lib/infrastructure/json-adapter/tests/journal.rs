#![allow(clippy::unwrap_used)]

//! Current run journals: encoding, schema conformance and binding relations.

use std::time::{Duration, SystemTime};

use review_kit_application::{
    publication::{
        Effects, ProviderId, ResolutionProvenance, ResolutionReceipt, Round, RoundStatus, Snapshot,
        Write,
    },
    review_run::{ContentRef, RunId, SettledOutcome, SnapshotFiles},
    use_cases::publishing::{
        PublicationProgress, RoundAuthorization, RunBinding, SettledRun, UnresolvedWrite,
        WritePhase,
    },
};
use review_kit_domain::{
    Assessment, AssessmentDraft, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision,
    ReviewerId, Revision, Subject,
};
use review_kit_json_adapter::{JsonError, ProgressDocument, decode, encode};
use serde_json::{Value, json};

const SCHEMA: &str = include_str!("../schemas/publication-progress-v3.schema.json");

fn validator() -> jsonschema::Validator {
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&serde_json::from_str(SCHEMA).unwrap())
        .unwrap()
}

fn assert_schema_valid(value: &Value) {
    let validator = validator();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

fn subject(host: &str) -> Subject {
    Subject::new("gitlab".into(), host.into(), "team/project".into(), 1).unwrap()
}

fn reviewer() -> ReviewerId {
    ReviewerId::try_from("9".to_owned()).unwrap()
}

fn content(digit: char) -> ContentRef {
    ContentRef::from_hex(&digit.to_string().repeat(64)).unwrap()
}

fn binding(run: u8, digit: char) -> RunBinding {
    RunBinding {
        run_id: RunId::try_from(format!("00000000-0000-4000-8000-{run:012}")).unwrap(),
        round: 1,
        review_assessment: content(digit),
        snapshot_files: SnapshotFiles {
            context: content('1'),
            source: content('2'),
            diff: content('3'),
            conversation: content('4'),
        },
        prior_authorization: None,
    }
}

fn settled(binding: &RunBinding, outcome: SettledOutcome, reason: Option<&str>) -> SettledRun {
    SettledRun {
        run_id: binding.run_id.clone(),
        review_assessment: binding.review_assessment.clone(),
        snapshot_files: binding.snapshot_files.clone(),
        round: binding.round,
        outcome,
        reason: reason.map(str::to_owned),
        verified_at: SystemTime::UNIX_EPOCH + Duration::new(1_791_893_400, 250_000_000),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        subject: subject("https://example.test"),
        reviewer: reviewer(),
        revision: Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap(),
        summaries: Vec::new(),
        findings: Vec::new(),
        replies: Vec::new(),
        permissions: Effects::review(),
    }
}

/// A prepared summary creation with its grant, a consumed retry and a receipt.
fn retained_state() -> (Snapshot, PublicationProgress) {
    let snapshot = snapshot();
    let policy = PolicyId::try_from("proof".to_owned()).unwrap();
    let assessment = Assessment::new(AssessmentDraft {
        subject: snapshot.subject.clone(),
        reviewer: snapshot.reviewer.clone(),
        revision: snapshot.revision.clone(),
        decision: ReviewDecision::Ready,
        depth: "complete".into(),
        scope: "changed source".into(),
        limitation: None,
        selected_policies: vec![policy.clone()],
        evaluations: vec![PolicyEvaluation::new(policy, PolicyJudgment::Satisfied).unwrap()],
        findings: Vec::new(),
    })
    .unwrap();
    let write = Write::Summary {
        expected: None,
        round: Box::new(Round {
            confirmed_validation: true,
            number: 1,
            assessment,
            status: RoundStatus::Pending,
            abandonment: None,
            publication_limitation: None,
            answers: Vec::new(),
            transitions: Vec::new(),
            answer_targets: Vec::new(),
            actions: Vec::new(),
            review_workflow: None,
            policy_set: Some("rust".into()),
            policy_names: vec![review_kit_application::publication::PolicyName { id: PolicyId::try_from("proof".to_owned()).unwrap(), name: "Proof".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: PolicyId::try_from("proof".to_owned()).unwrap(), name: "Proof".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
            renderer: None,
            communicated_findings: Some(Vec::new()),
            verified_resolutions: Vec::new(),
        }),
        history: Vec::new(),
        body: "Pending review.".into(),
    };
    let progress = PublicationProgress {
        unresolved: Some(UnresolvedWrite {
            subject: snapshot.subject.clone(),
            reviewer: snapshot.reviewer.clone(),
            write: write.clone(),
            renderer: None,
            phase: WritePhase::Prepared,
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

fn journal_value(snapshot: &Snapshot, progress: &PublicationProgress) -> Value {
    serde_json::to_value(ProgressDocument::journal(
        &snapshot.subject,
        &snapshot.reviewer,
        progress,
    ))
    .unwrap()
}

fn restore_journal(snapshot: &Snapshot, value: Value) -> Result<PublicationProgress, JsonError> {
    serde_json::from_value::<ProgressDocument>(value)
        .map_err(JsonError::Syntax)?
        .into_journal(&snapshot.subject, &snapshot.reviewer)
}

#[test]
fn bound_journal_round_trips_through_the_schema_with_exact_retained_state() {
    let (snapshot, mut progress) = retained_state();
    let earlier = binding(1, 'a');
    let current = binding(2, 'b');
    progress.coverage_binding = Some(current.clone());
    progress.settled_runs = vec![settled(&earlier, SettledOutcome::Published, None)];
    progress.dispatch_may_have_started = true;

    let document = ProgressDocument::journal(&snapshot.subject, &snapshot.reviewer, &progress);
    assert_eq!(document.version, 3);
    let bytes = encode(&document).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_schema_valid(&value);
    assert_eq!(
        value["settled_runs"][0]["verified_at"],
        "2026-10-13T12:10:00.25Z"
    );
    assert_eq!(
        value["coverage_binding"]["snapshot_files"]["review-conversation.json"],
        content('4').as_str()
    );
    let restored = decode::<ProgressDocument>(&bytes)
        .unwrap()
        .into_journal(&snapshot.subject, &snapshot.reviewer)
        .unwrap();
    assert_eq!(restored, progress);

    assert_eq!(
        ProgressDocument::new(&snapshot.subject, &snapshot.reviewer, &progress).version,
        3
    );
    let legacy = PublicationProgress {
        coverage_binding: None,
        settled_runs: Vec::new(),
        dispatch_may_have_started: false,
        ..progress
    };
    assert_eq!(
        ProgressDocument::new(&snapshot.subject, &snapshot.reviewer, &legacy).version,
        2
    );
}

#[test]
fn settled_and_abandoned_entries_are_schema_valid_and_restore() {
    let snapshot = snapshot();
    let current = binding(3, 'c');
    let progress = PublicationProgress {
        coverage_binding: Some(current.clone()),
        settled_runs: vec![
            settled(&binding(1, 'a'), SettledOutcome::NoChanges, None),
            settled(
                &binding(2, 'b'),
                SettledOutcome::Abandoned,
                Some("Superseded by a new head."),
            ),
            settled(&current, SettledOutcome::Published, None),
        ],
        dispatch_may_have_started: true,
        ..Default::default()
    };
    let value = journal_value(&snapshot, &progress);
    assert_schema_valid(&value);
    assert_eq!(value["settled_runs"][0]["outcome"], "no_changes");
    assert_eq!(restore_journal(&snapshot, value).unwrap(), progress);
}

#[test]
fn a_binding_keeps_the_round_grant_that_preceded_its_installation() {
    let snapshot = snapshot();
    let prior = RoundAuthorization {
        round: 1,
        effects: Effects::new([
            review_kit_application::publication::Effect::CreateSummary,
            review_kit_application::publication::Effect::UpdateSummary,
        ]),
    };
    let mut progress = PublicationProgress {
        coverage_binding: Some(RunBinding {
            prior_authorization: Some(prior.clone()),
            ..binding(1, 'a')
        }),
        ..Default::default()
    };

    let value = journal_value(&snapshot, &progress);
    assert_schema_valid(&value);
    assert_eq!(
        value["coverage_binding"]["prior_authorization"],
        json!({"round": 1, "effects": ["create_summary", "update_summary"]})
    );
    assert_eq!(restore_journal(&snapshot, value.clone()).unwrap(), progress);

    let mut explicit_null = value.clone();
    explicit_null["coverage_binding"]["prior_authorization"] = Value::Null;
    assert_schema_valid(&explicit_null);
    progress
        .coverage_binding
        .as_mut()
        .unwrap()
        .prior_authorization = None;
    assert_eq!(restore_journal(&snapshot, explicit_null).unwrap(), progress);
    let omitted = journal_value(&snapshot, &progress);
    assert!(
        omitted["coverage_binding"]
            .as_object()
            .unwrap()
            .get("prior_authorization")
            .is_none()
    );

    let mut round_zero = value;
    round_zero["coverage_binding"]["prior_authorization"]["round"] = json!(0);
    assert!(!validator().is_valid(&round_zero));
    assert!(matches!(
        restore_journal(&snapshot, round_zero),
        Err(JsonError::InvalidValue)
    ));
}

#[test]
fn shipped_examples_match_the_schema_and_the_domain_decoder() {
    let packet_subject = subject("https://gitlab.example.test");
    for (name, text) in [
        (
            "bootstrap",
            include_str!("fixtures/progress-v3-bootstrap.json"),
        ),
        ("settled", include_str!("fixtures/progress-v3-settled.json")),
        (
            "interrupted",
            include_str!("fixtures/progress-v3-interrupted.json"),
        ),
    ] {
        let value: Value = serde_json::from_str(text).unwrap();
        assert_schema_valid(&value);
        let restored = decode::<ProgressDocument>(text.as_bytes())
            .unwrap()
            .into_journal(&packet_subject, &reviewer());
        match name {
            "bootstrap" => assert_eq!(restored.unwrap(), PublicationProgress::default()),
            "settled" => {
                let progress = restored.unwrap();
                let binding = progress.coverage_binding.clone().unwrap();
                assert_eq!(progress.settled_runs.len(), 1);
                assert_eq!(progress.settled_runs[0].run_id, binding.run_id);
                assert_eq!(progress.settled_runs[0].outcome, SettledOutcome::Published);
                assert!(progress.dispatch_may_have_started);
                let rewritten = serde_json::to_value(ProgressDocument::journal(
                    &packet_subject,
                    &reviewer(),
                    &progress,
                ))
                .unwrap();
                assert_schema_valid(&rewritten);
                assert_eq!(rewritten["settled_runs"], value["settled_runs"]);
                assert_eq!(rewritten["coverage_binding"], value["coverage_binding"]);
            }
            _ => {
                // Recovery needs the baseline observation that this shape fixture omits.
                assert!(matches!(restored, Err(JsonError::InvalidValue)));
            }
        }
    }
}

#[test]
fn each_reader_accepts_only_its_own_versions_and_fields() {
    let (snapshot, mut progress) = retained_state();
    progress.unresolved = None;
    let legacy = serde_json::to_value(ProgressDocument::new(
        &snapshot.subject,
        &snapshot.reviewer,
        &progress,
    ))
    .unwrap();
    let journal = journal_value(&snapshot, &progress);
    let restore_legacy = |value: Value| {
        serde_json::from_value::<ProgressDocument>(value)
            .unwrap()
            .into_progress(&snapshot.subject, &snapshot.reviewer)
    };

    assert!(matches!(
        restore_legacy(journal.clone()),
        Err(JsonError::Version(3))
    ));
    assert!(matches!(
        restore_journal(&snapshot, legacy.clone()),
        Err(JsonError::Version(2))
    ));
    for field in [
        "coverage_binding",
        "settled_runs",
        "dispatch_may_have_started",
    ] {
        let mut mixed = legacy.clone();
        mixed[field] = journal[field].clone();
        assert!(
            matches!(restore_legacy(mixed), Err(JsonError::InvalidValue)),
            "{field}"
        );
        let mut incomplete = journal.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(!validator().is_valid(&incomplete), "{field}");
        assert!(
            matches!(
                restore_journal(&snapshot, incomplete),
                Err(JsonError::InvalidValue)
            ),
            "{field}"
        );
    }
    let mut unknown = journal.clone();
    unknown["coverage_owner"] = Value::Null;
    assert!(!validator().is_valid(&unknown));
    assert!(matches!(
        restore_journal(&snapshot, unknown),
        Err(JsonError::Syntax(_))
    ));
    let other = Snapshot {
        reviewer: ReviewerId::try_from("10".to_owned()).unwrap(),
        ..snapshot.clone()
    };
    assert!(matches!(
        restore_journal(&other, journal),
        Err(JsonError::ContextBinding)
    ));
}

#[test]
fn journal_relations_reject_inconsistent_bindings_and_settlements() {
    let (snapshot, retained) = retained_state();
    let current = binding(2, 'b');
    let bound = PublicationProgress {
        coverage_binding: Some(current.clone()),
        settled_runs: vec![settled(&binding(1, 'a'), SettledOutcome::Published, None)],
        dispatch_may_have_started: true,
        ..retained.clone()
    };
    let valid = journal_value(&snapshot, &bound);
    assert_schema_valid(&valid);
    restore_journal(&snapshot, valid.clone()).unwrap();

    let null_binding = PublicationProgress {
        unresolved: None,
        ..retained.clone()
    };
    let null_valid = journal_value(&snapshot, &null_binding);
    assert_schema_valid(&null_valid);
    restore_journal(&snapshot, null_valid.clone()).unwrap();

    let mut cases: Vec<(&str, Value, bool)> = Vec::new();
    let mut unbound_intent = null_valid.clone();
    unbound_intent["unresolved"] = valid["unresolved"].clone();
    cases.push(("null binding with intent", unbound_intent, true));
    let mut unbound_settlement = null_valid.clone();
    unbound_settlement["settled_runs"] = valid["settled_runs"].clone();
    cases.push(("null binding with settlement", unbound_settlement, true));
    let mut unbound_dispatch = null_valid.clone();
    unbound_dispatch["dispatch_may_have_started"] = json!(true);
    cases.push(("null binding with dispatch", unbound_dispatch, true));
    let mut abandoned = valid.clone();
    abandoned["settled_runs"][0]["outcome"] = json!("abandoned");
    cases.push(("abandoned without reason", abandoned, true));
    let mut blank = valid.clone();
    blank["settled_runs"][0]["reason"] = json!("  ");
    cases.push(("blank reason", blank, true));
    let mut round = valid.clone();
    round["coverage_binding"]["round"] = json!(0);
    cases.push(("zero round", round, true));
    let mut run_id = valid.clone();
    run_id["coverage_binding"]["run_id"] = json!("00000000-0000-4000-8000-00000000000A");
    cases.push(("uppercase run ID", run_id, false));
    let mut digest = valid.clone();
    digest["coverage_binding"]["review_assessment"] = json!(format!("sha256:{}", "a".repeat(63)));
    cases.push(("short digest", digest, true));
    let mut time = valid.clone();
    time["settled_runs"][0]["verified_at"] = json!("2026-10-13 12:10:00");
    cases.push(("timestamp", time, true));
    let mut duplicate = valid.clone();
    let entry = duplicate["settled_runs"][0].clone();
    duplicate["settled_runs"]
        .as_array_mut()
        .unwrap()
        .push(entry);
    cases.push(("duplicate settlement", duplicate, false));
    let current_settled = PublicationProgress {
        settled_runs: vec![SettledRun {
            round: 2,
            ..settled(&current, SettledOutcome::Published, None)
        }],
        unresolved: None,
        ..bound.clone()
    };
    cases.push((
        "settlement differs from its binding",
        journal_value(&snapshot, &current_settled),
        false,
    ));
    let settled_with_intent = PublicationProgress {
        settled_runs: vec![settled(&current, SettledOutcome::Published, None)],
        ..bound.clone()
    };
    cases.push((
        "settled binding with intent",
        journal_value(&snapshot, &settled_with_intent),
        false,
    ));

    for (name, value, schema_rejects) in cases {
        if schema_rejects {
            assert!(!validator().is_valid(&value), "{name}");
        }
        assert!(
            matches!(
                restore_journal(&snapshot, value),
                Err(JsonError::InvalidValue)
            ),
            "{name}"
        );
    }
}
