use std::error::Error;

use review_kit_application::publication::{PlannedAction, ReplyKind, Round, RoundStatus};
use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, Revision, Severity,
    SourceLocation, SourcePath, Subject, Suggestion,
};
use serde_json::json;

use super::*;
use crate::{MergeRequestIid, ProjectId};

type TestResult = Result<(), Box<dyn Error>>;

fn binding() -> Result<crate::Binding, Box<dyn Error>> {
    Ok(crate::Binding {
        subject: Subject::new(
            "gitlab".into(),
            "https://gitlab.example".into(),
            "nested/group/repo".into(),
            7,
        )?,
        target_project: ProjectId::new(11)?,
        source_project: ProjectId::new(12)?,
        iid: MergeRequestIid::new(7)?,
        mr_id: 99,
        reviewer: "42".to_owned().try_into()?,
    })
}

fn finding() -> Result<Finding, Box<dyn Error>> {
    Ok(Finding::new(FindingDraft {
        id: FindingId::try_from(3)?,
        policy_id: PolicyId::try_from("correctness".to_owned())?,
        title: "Request is dropped".into(),
        body: "The error branch drops the request.".into(),
        severity: Severity::P1,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "The request cannot reach the retry.".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned())?,
            LineNumber::try_from(2)?,
        ),
        suggestion: None,
    })?)
}

fn round(binding: &crate::Binding, number: u64) -> Result<Round, Box<dyn Error>> {
    let finding = finding()?;
    let policy = finding.policy_id().clone();
    let assessment = Assessment::new(AssessmentDraft {
        subject: binding.subject.clone(),
        reviewer: binding.reviewer.clone(),
        revision: Revision::new("a".repeat(40), vec!["b".repeat(40), "c".repeat(40)])?,
        decision: ReviewDecision::ChangesRequired,
        depth: "Review of changed control flow".into(),
        scope: "Changed source file".into(),
        limitation: None,
        selected_policies: vec![policy.clone()],
        evaluations: vec![PolicyEvaluation::new(
            policy,
            PolicyJudgment::Violated {
                evidence: "An error path loses the request.".into(),
            },
        )?],
        findings: vec![finding.clone()],
    })
    .map_err(|errors| format!("invalid fixture assessment: {errors:?}"))?;
    Ok(Round {
        confirmed_validation: false,
        review_workflow: None,
        policy_set: None,
        policy_names: vec![review_kit_application::publication::PolicyName {
            id: finding.policy_id().clone(),
            name: "Correctness".into(),
        }],
        policy_declarations: vec![
            review_kit_application::publication::PublicPolicyDeclaration {
                policy: review_kit_application::publication::PolicyName {
                    id: finding.policy_id().clone(),
                    name: "Correctness".into(),
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
        publication_limitation: None,
        answers: vec![],
        transitions: vec![],
        answer_targets: vec![],
        actions: vec![PlannedAction::Finding {
            finding: Box::new(finding),
            body: "Rendered finding\n".into(),
        }],
    })
}

fn note(id: u64, author: u64, body: &str) -> Result<Note, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "id": id, "body": body, "author": {"id": author}, "system": false,
        "noteable_id": 99, "noteable_type": "MergeRequest", "project_id": 11,
        "noteable_iid": null, "type": "DiffNote", "position": {"position_type": "text"},
        "resolvable": true, "resolved": false, "suggestions": []
    }))?)
}

fn discussion(root: Note) -> Discussion {
    Discussion {
        id: "abc123".into(),
        individual_note: false,
        notes: vec![root],
    }
}

#[test]
fn foreign_markers_never_establish_ownership_and_owned_malformed_records_fail() -> TestResult {
    let binding = binding()?;
    let copied = note(1, 9, "copied\n\n<!-- review-kit:v99 malformed -->")?;
    assert!(owned_marker(&copied, &binding.reviewer)?.is_none());
    let valid_summary = wire::encode_summary("Overview", &round(&binding, 1)?, &[])?;
    for body in [&valid_summary, "<!-- review-kit:state:v3 malformed -->"] {
        assert!(owned_marker(&note(4, 9, body)?, &binding.reviewer)?.is_none());
    }
    let own = note(2, 42, &copied.body)?;
    assert!(matches!(
        owned_marker(&own, &binding.reviewer),
        Err(GitLabError::Marker)
    ));
    let mut system = note(
        3,
        42,
        &wire::append_marker(
            "body",
            &Marker::Finding {
                finding: 3,
                round: 2,
            },
        )?,
    )?;
    system.system = true;
    assert!(matches!(
        owned_marker(&system, &binding.reviewer),
        Err(GitLabError::Marker)
    ));
    Ok(())
}

#[test]
fn note_scope_corroborates_project_global_request_and_optional_iid() -> TestResult {
    let binding = binding()?;
    let valid = note(1, 42, "body")?;
    valid.check_scope(&binding)?;
    for field in ["project_id", "noteable_id", "noteable_iid", "id"] {
        let mut value = json!({
            "id": 1, "body": "body", "author": {"id": 42}, "system": false,
            "project_id": 11, "noteable_id": 99, "noteable_type": "MergeRequest", "noteable_iid": 7
        });
        value[field] = json!(0);
        let invalid: Note = serde_json::from_value(value)?;
        assert!(matches!(
            invalid.check_scope(&binding),
            Err(GitLabError::Identity)
        ));
    }
    let mut wrong_type = valid.clone();
    wrong_type.noteable_type = "Issue".into();
    assert!(wrong_type.check_scope(&binding).is_err());
    let mut wrong_author = valid;
    wrong_author.author.id = 0;
    assert!(wrong_author.check_scope(&binding).is_err());
    Ok(())
}

#[test]
fn incomplete_note_decoding_is_an_error() {
    for absent in [
        "body",
        "author",
        "system",
        "noteable_id",
        "noteable_type",
        "project_id",
    ] {
        let mut value = json!({
            "id": 1, "body": "body", "author": {"id": 42}, "system": false,
            "project_id": 11, "noteable_id": 99, "noteable_type": "MergeRequest"
        });
        if let Some(object) = value.as_object_mut() {
            object.remove(absent);
        }
        assert!(serde_json::from_value::<Note>(value).is_err());
    }
}

#[test]
fn roots_observed_on_both_endpoints_are_deduplicated_without_losing_positions() -> TestResult {
    let binding = binding()?;
    let root = note(1, 42, "body")?;
    let mut plain = root.clone();
    plain.position = None;
    plain.kind = None;
    let indexed = index_notes(vec![plain], &[discussion(root)], &binding)?;
    assert_eq!(indexed.len(), 1);
    let observed = indexed.get(&1).ok_or("missing root")?;
    assert_eq!(observed.kind.as_deref(), Some("DiffNote"));
    assert!(observed.position.is_some());
    Ok(())
}

#[test]
fn conflicting_note_observations_and_cross_thread_note_reuse_are_errors() -> TestResult {
    let binding = binding()?;
    let first = note(1, 42, "body")?;
    let changed = note(1, 9, "edited body")?;
    assert!(matches!(
        index_notes(vec![first.clone()], &[discussion(changed)], &binding),
        Err(GitLabError::Conflict)
    ));
    let mut other_thread = discussion(first.clone());
    other_thread.id = "second".into();
    assert!(matches!(
        index_notes(vec![], &[discussion(first), other_thread], &binding),
        Err(GitLabError::Protocol)
    ));
    Ok(())
}

#[test]
fn retained_state_cannot_change_provider_bound_subject_or_reviewer() -> TestResult {
    let binding = binding()?;
    let retained = round(&binding, 2)?;
    let summary = SummaryRecord {
        id: provider_id(1)?,
        author: binding.reviewer.clone(),
        body: "Summary".into(),
        current: retained,
        history: vec![],
    };
    let summaries = [summary.clone()];
    assert_eq!(retained_rounds(&summaries, &binding)?.len(), 1);
    let mut changed_binding = binding.clone();
    changed_binding.reviewer = "9".to_owned().try_into()?;
    assert!(matches!(
        retained_rounds(&summaries, &changed_binding),
        Err(GitLabError::Identity)
    ));
    changed_binding = binding.clone();
    changed_binding.subject = Subject::new(
        "gitlab".into(),
        "https://gitlab.example".into(),
        "other/project".into(),
        7,
    )?;
    assert!(matches!(
        retained_rounds(&summaries, &changed_binding),
        Err(GitLabError::Identity)
    ));
    let mut conflicting = summary.clone();
    conflicting.id = provider_id(2)?;
    conflicting.current.actions.clear();
    assert!(matches!(
        retained_rounds(&[summary, conflicting], &binding),
        Err(GitLabError::Conflict)
    ));
    Ok(())
}

#[test]
fn a_minimal_finding_marker_recovers_before_summary_links_are_recorded() -> TestResult {
    let binding = binding()?;
    let round = round(&binding, 2)?;
    let rounds = BTreeMap::from([(2, &round)]);
    let body = wire::append_marker(
        "Rendered finding\n",
        &Marker::Finding {
            finding: 3,
            round: 2,
        },
    )?;
    let root = note(101, 42, &body)?;
    let thread = discussion(root.clone());
    let (body, marker) = owned_marker(&root, &binding.reviewer)?.ok_or("missing marker")?;
    let mapped = map_finding(&thread, &root, body, &marker, &rounds)?;
    assert_eq!(mapped.root_note.as_str(), "101");
    assert_eq!(mapped.id.as_str(), "abc123");
    assert_eq!(mapped.created_in, 2);
    assert_eq!(mapped.finding, finding()?);
    assert_eq!(mapped.body, "Rendered finding\n");
    assert_eq!(&mapped.revision, round.assessment.revision());
    Ok(())
}

#[test]
fn orphan_and_edited_findings_are_not_adopted() -> TestResult {
    let binding = binding()?;
    let round = round(&binding, 2)?;
    let rounds = BTreeMap::from([(2, &round)]);
    let root = note(101, 42, "irrelevant")?;
    let thread = discussion(root.clone());
    assert!(matches!(
        map_finding(
            &thread,
            &root,
            "Rendered finding\n".into(),
            &Marker::Finding {
                finding: 3,
                round: 99
            },
            &rounds
        ),
        Err(GitLabError::State)
    ));
    assert!(matches!(
        map_finding(
            &thread,
            &root,
            "Edited".into(),
            &Marker::Finding {
                finding: 3,
                round: 2
            },
            &rounds
        ),
        Err(GitLabError::Conflict)
    ));
    let mut absent_position = root;
    absent_position.position = None;
    assert!(matches!(
        map_finding(
            &thread,
            &absent_position,
            "Rendered finding\n".into(),
            &Marker::Finding {
                finding: 3,
                round: 2
            },
            &rounds
        ),
        Err(GitLabError::UnsupportedPosition)
    ));
    Ok(())
}

#[test]
fn resolved_thread_requires_every_resolvable_note_to_be_resolved() -> TestResult {
    let mut root = note(1, 42, "root")?;
    root.resolved = Some(true);
    let mut thread = discussion(root);
    assert!(thread.resolved()?);
    thread.notes.push(note(2, 9, "unresolved reply")?);
    assert!(!thread.resolved()?);
    thread.notes[1].resolved = None;
    assert!(matches!(thread.resolved(), Err(GitLabError::Protocol)));
    thread.notes[1].resolvable = Some(false);
    assert!(thread.resolved()?);
    Ok(())
}

#[test]
fn foreign_forged_reply_retains_exact_body_without_an_action() -> TestResult {
    let binding = binding()?;
    let round = round(&binding, 2)?;
    let rounds = BTreeMap::from([(2, &round)]);
    let root = note(1, 42, "root")?;
    let mapped_root = map_finding(
        &discussion(root.clone()),
        &root,
        "Rendered finding\n".into(),
        &Marker::Finding {
            finding: 3,
            round: 2,
        },
        &rounds,
    )?;
    let forged = note(2, 9, "question\r\n\n<!-- review-kit:v99 malformed -->")?;
    let mapped = map_reply(
        &forged,
        &mapped_root,
        &binding.reviewer,
        owned_marker(&forged, &binding.reviewer)?,
        &rounds,
    )?;
    assert_eq!(mapped.body, forged.body);
    assert!(mapped.action.is_none());
    assert_eq!(mapped.author.as_str(), "9");
    Ok(())
}

#[test]
fn transition_and_answer_markers_require_retained_intent_and_thread_identity() -> TestResult {
    let binding = binding()?;
    let first = round(&binding, 2)?;
    let mut next = round(&binding, 3)?;
    next.actions = vec![
        PlannedAction::Reply {
            finding: FindingId::try_from(3)?,
            kind: ReplyKind::Transition(FindingStatus::Fixed),
            body: "Fixed\n".into(),
        },
        PlannedAction::Reply {
            finding: FindingId::try_from(3)?,
            kind: ReplyKind::Answer(provider_id(66)?),
            body: "Answer".into(),
        },
    ];
    let rounds = BTreeMap::from([(2, &first), (3, &next)]);
    let root = note(1, 42, "root")?;
    let mapped_root = map_finding(
        &discussion(root.clone()),
        &root,
        "Rendered finding\n".into(),
        &Marker::Finding {
            finding: 3,
            round: 2,
        },
        &rounds,
    )?;
    for (id, marker, body, kind) in [
        (
            2,
            Marker::Transition {
                finding: 3,
                round: 3,
            },
            "Fixed\n",
            ReplyKind::Transition(FindingStatus::Fixed),
        ),
        (
            3,
            Marker::Answer {
                finding: 3,
                round: 3,
                note: "66".into(),
            },
            "Answer",
            ReplyKind::Answer(provider_id(66)?),
        ),
    ] {
        let marked_body = wire::append_marker(body, &marker)?;
        let reply = note(id, 42, &marked_body)?;
        let mapped = map_reply(
            &reply,
            &mapped_root,
            &binding.reviewer,
            owned_marker(&reply, &binding.reviewer)?,
            &rounds,
        )?;
        assert_eq!(mapped.body, body);
        assert_eq!(
            mapped.action.as_ref().map(|action| &action.kind),
            Some(&kind)
        );
        let edited = Some(("Changed body".into(), marker));
        assert!(matches!(
            map_reply(&reply, &mapped_root, &binding.reviewer, edited, &rounds),
            Err(GitLabError::Conflict)
        ));
    }
    let reply = note(4, 42, "body")?;
    assert!(matches!(
        map_reply(
            &reply,
            &mapped_root,
            &binding.reviewer,
            Some((
                "body".into(),
                Marker::Transition {
                    finding: 4,
                    round: 3
                }
            )),
            &rounds
        ),
        Err(GitLabError::Conflict)
    ));
    Ok(())
}

#[test]
fn suggestion_readback_requires_exact_range_and_content_but_accepts_external_application()
-> TestResult {
    let initial = finding()?;
    let revision = Revision::new("a".repeat(40), vec!["b".repeat(40), "c".repeat(40)])?;
    let suggestion = Suggestion::new(
        initial.location().clone(),
        revision,
        LineNumber::try_from(1)?,
        vec!["old".into(), "line".into()],
        vec!["replacement".into()],
        "The replacement preserves retries.".into(),
    )?;
    let suggested = Finding::new(FindingDraft {
        id: initial.id(),
        policy_id: initial.policy_id().clone(),
        title: initial.title().into(),
        body: initial.body().into(),
        severity: initial.severity(),
        disposition: initial.disposition(),
        status: initial.status(),
        evidence: initial.evidence().into(),
        validated: true,
        location: initial.location().clone(),
        suggestion: Some(suggestion),
    })?;
    let mut root = note(1, 42, "root")?;
    root.suggestions = Some(serde_json::from_value(json!([{
        "from_line": 1, "to_line": 2, "from_content": "old\nline\n", "to_content": "replacement\n", "applied": true
    }]))?);
    let contents = ("old\nline\n".into(), "replacement\n".into());
    root.verify_suggestions(&suggested, Some(&contents), Some((1, 2)))?;
    if let Some(values) = &mut root.suggestions {
        values[0].to_line = 3;
    }
    assert!(matches!(
        root.verify_suggestions(&suggested, Some(&contents), Some((1, 2))),
        Err(GitLabError::Conflict)
    ));
    root.suggestions = None;
    assert!(matches!(
        root.verify_suggestions(&suggested, Some(&contents), Some((1, 2))),
        Err(GitLabError::Protocol)
    ));
    Ok(())
}
