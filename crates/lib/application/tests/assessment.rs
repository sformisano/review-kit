#![allow(clippy::unwrap_used)]

use std::io;

use review_kit_application::publication::PolicyName;
use review_kit_application::{
    ports::{CommittedSource, PortError, ReviewRenderer},
    publication::{
        ActionId, Effects, FindingRecord, PlannedAction, ProviderId, ReplyKind, ReplyRecord, Round,
        RoundStatus, Snapshot, SummaryRecord, TransitionReply,
    },
    summary::SummaryView,
    use_cases::{
        assessment::{
            AssessmentPolicies, CheckProblem, DraftIssue, FindingSlot, PolicySlot, check, complete,
            prepare,
        },
        checkpoints::reusable_confirmations,
    },
};
use review_kit_domain::{
    Assessment, AssessmentDraft, AssessmentError, Disposition, Finding, FindingDraft, FindingId,
    FindingStatus, LineNumber, PolicyEvaluation, PolicyEvaluationError, PolicyId, PolicyJudgment,
    ReviewDecision, ReviewerId, Revision, Severity, SourceLocation, SourcePath, Subject,
    Suggestion,
};

fn subject() -> Subject {
    Subject::new(
        "gitlab".into(),
        "reviews.example".into(),
        "group/project".into(),
        7,
    )
    .unwrap()
}

fn reviewer() -> ReviewerId {
    ReviewerId::try_from("reviewer-17".to_owned()).unwrap()
}
fn revision() -> Revision {
    Revision::new("head".into(), vec!["base".into(), "start".into()]).unwrap()
}
fn policy() -> PolicyId {
    PolicyId::try_from("safety".to_owned()).unwrap()
}
fn selected() -> AssessmentPolicies {
    AssessmentPolicies::new(
        "test-policies".into(),
        vec![PolicyName {
            id: policy(),
            name: "Correctness".into(),
        }],
    )
    .unwrap()
}
fn provider_id(value: &str) -> ProviderId {
    ProviderId::new(value.to_owned()).unwrap()
}

fn finding(number: u64, status: FindingStatus, suggestion: Option<Suggestion>) -> Finding {
    let location = suggestion
        .as_ref()
        .map(|value| value.location().clone())
        .unwrap_or_else(|| {
            SourceLocation::new(
                SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
                LineNumber::try_from(2).unwrap(),
            )
        });
    Finding::new(FindingDraft {
        id: FindingId::try_from(number).unwrap(),
        policy_id: policy(),
        title: "Missing guard".into(),
        body: "The request crosses the ownership boundary.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Reproduced with another account.".into(),
        validated: true,
        location,
        suggestion,
    })
    .unwrap()
}

fn assessment(findings: Vec<Finding>) -> Assessment {
    Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer(),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: vec![PolicyEvaluation::new(policy(), PolicyJudgment::Satisfied).unwrap()],
        findings,
    })
    .unwrap()
}

fn snapshot() -> Snapshot {
    Snapshot {
        subject: subject(),
        reviewer: reviewer(),
        revision: revision(),
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::review(),
    }
}

#[test]
fn missing_metadata_does_not_invent_an_agent_decision_or_limitation_requirement() {
    let mut template = prepare(snapshot(), reviewer(), selected()).unwrap();
    for slot in &mut template.policies {
        slot.judgment = Some(PolicyJudgment::Satisfied);
    }
    assert_eq!(
        complete(template, vec![]),
        Err(vec![
            DraftIssue::MissingDecision,
            DraftIssue::MissingDepth,
            DraftIssue::MissingScope
        ])
    );
}

#[derive(Debug, PartialEq, Eq)]
enum SourceCall {
    Lines {
        subject: Subject,
        revision: Revision,
        path: SourcePath,
    },
    Position {
        subject: Subject,
        revision: Revision,
        finding: FindingId,
    },
}

struct Source {
    lines: Vec<String>,
    fail_read: bool,
    fail_position: bool,
    calls: Vec<SourceCall>,
}

impl Source {
    fn new(lines: &[&str]) -> Self {
        Self {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            fail_read: false,
            fail_position: false,
            calls: vec![],
        }
    }
}

fn port_error(operation: &'static str) -> PortError {
    PortError::new(operation, io::Error::other("fixture failure"))
}

impl CommittedSource for Source {
    fn lines(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        self.calls.push(SourceCall::Lines {
            subject: subject.clone(),
            revision: revision.clone(),
            path: path.clone(),
        });
        if self.fail_read {
            Err(port_error("source"))
        } else {
            Ok(self.lines.clone())
        }
    }
    fn validate_position(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<(), PortError> {
        self.calls.push(SourceCall::Position {
            subject: subject.clone(),
            revision: revision.clone(),
            finding: finding.id(),
        });
        if self.fail_position {
            Err(port_error("position"))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct Renderer {
    findings: Vec<FindingId>,
    fail: bool,
    empty: bool,
}

impl ReviewRenderer for Renderer {
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError> {
        self.findings.push(finding.id());
        if self.fail {
            Err(port_error("render"))
        } else if self.empty {
            Ok(" \n".into())
        } else {
            Ok(format!("Finding {}", finding.id().get()))
        }
    }
    fn transition(&mut self, _: &Finding, _: FindingStatus, _: &str) -> Result<String, PortError> {
        Err(port_error("unexpected transition render"))
    }
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        Ok(body.to_owned())
    }
    fn summary(&mut self, _: &SummaryView) -> Result<String, PortError> {
        Err(port_error("unexpected summary render"))
    }
}

#[test]
fn preparation_carries_latest_owned_context_and_leaves_all_judgments_unassessed() {
    let original = finding(1, FindingStatus::Open, None);
    let latest = finding(1, FindingStatus::Fixed, None);
    let mut state = snapshot();
    state.findings.push(FindingRecord {
        created_in: 1,
        revision: revision(),
        id: provider_id("owned-root"),
        root_note: provider_id("owned-root-note"),
        author: reviewer(),
        finding: original,
        body: "original".into(),
        resolved: true,
    });
    state.findings.push(FindingRecord {
        created_in: 1,
        revision: revision(),
        id: provider_id("foreign-root"),
        root_note: provider_id("foreign-root-note"),
        author: ReviewerId::try_from("someone-else".to_owned()).unwrap(),
        finding: finding(2, FindingStatus::Open, None),
        body: "foreign".into(),
        resolved: false,
    });
    state.summaries.push(SummaryRecord {
        id: provider_id("summary"),
        author: reviewer(),
        body: "completed".into(),
        current: Round {
            confirmed_validation: true,
            review_workflow: None,
            policy_set: None,
            policy_names: vec![review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
            renderer: None,
            communicated_findings: None,
            verified_resolutions: Vec::new(),
            publication_limitation: None,
            abandonment: None,
            number: 2,
            answers: vec![],
            transitions: vec![TransitionReply {
                finding: latest.id(),
                from: FindingStatus::Open,
                to: FindingStatus::Fixed,
                body: "Regression confirms the fix.".into(),
            }],
            answer_targets: vec![],
            actions: vec![PlannedAction::Reply {
                finding: latest.id(),
                kind: ReplyKind::Transition(FindingStatus::Fixed),
                body: "Fixed explanation".into(),
            }],
            assessment: assessment(vec![latest.clone()]),
            status: RoundStatus::Completed,
        },
        history: vec![],
    });
    state.replies.push(ReplyRecord {
        id: provider_id("transition-note"),
        author: reviewer(),
        thread: provider_id("owned-root"),
        action: Some(ActionId {
            round: 2,
            finding: latest.id(),
            kind: ReplyKind::Transition(FindingStatus::Fixed),
        }),
        body: "Fixed explanation".into(),
    });

    let prepared = prepare(state.clone(), reviewer(), selected()).unwrap();

    assert_eq!(prepared.snapshot(), &state);
    assert_eq!(prepared.reviewer(), &reviewer());
    assert!(prepared.decision.is_none());
    assert!(prepared.depth.is_none());
    assert!(prepared.scope.is_none());
    assert_eq!(prepared.policies.len(), 1);
    assert_eq!(prepared.policies[0].policy_id, policy());
    assert!(prepared.policies[0].judgment.is_none());
    assert_eq!(prepared.findings.len(), 1);
    assert_eq!(prepared.findings[0].previous, latest);
    assert!(prepared.findings[0].reassessment.is_none());
}

#[test]
fn preparation_keeps_last_completed_judgment_when_current_transition_is_unpublished() {
    let original = finding(1, FindingStatus::Open, None);
    let desired = finding(1, FindingStatus::Fixed, None);
    for status in [
        RoundStatus::Pending,
        RoundStatus::Partial,
        RoundStatus::Blocked,
    ] {
        let mut state = snapshot();
        state.findings.push(FindingRecord {
            created_in: 1,
            revision: revision(),
            id: provider_id("owned-root"),
            root_note: provider_id("owned-root-note"),
            author: reviewer(),
            finding: original.clone(),
            body: "original".into(),
            resolved: false,
        });
        state.summaries.push(SummaryRecord {
            id: provider_id("summary"),
            author: reviewer(),
            body: "pending".into(),
            current: Round {
                confirmed_validation: true,
                review_workflow: None,
                policy_set: None,
                policy_names: vec![review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
                renderer: None,
                communicated_findings: None,
                verified_resolutions: Vec::new(),
                publication_limitation: None,
                abandonment: None,
                number: 2,
                answers: vec![],
                transitions: vec![],
                answer_targets: vec![],
                actions: vec![],
                assessment: assessment(vec![desired.clone()]),
                status,
            },
            history: vec![Round {
                confirmed_validation: true,
                review_workflow: None,
                policy_set: None,
                policy_names: vec![review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
                renderer: None,
                communicated_findings: None,
                verified_resolutions: Vec::new(),
                publication_limitation: None,
                abandonment: None,
                number: 1,
                answers: vec![],
                transitions: vec![],
                answer_targets: vec![],
                actions: vec![],
                assessment: assessment(vec![original.clone()]),
                status: RoundStatus::Completed,
            }],
        });
        let prepared = prepare(state.clone(), reviewer(), selected()).unwrap();
        assert_eq!(prepared.findings[0].previous, original);
        assert_eq!(prepared.snapshot(), &state);
        state.summaries[0].history.clear();
        let fallback = prepare(state, reviewer(), selected()).unwrap();
        assert_eq!(fallback.findings[0].previous, original);
    }
}

#[test]
fn unfinished_draft_reports_missing_judgments_reassessments_and_domain_errors_together() {
    let mut template = prepare(snapshot(), reviewer(), selected()).unwrap();
    let other = PolicyId::try_from("proof".to_owned()).unwrap();
    template.policies.push(PolicySlot {
        policy_id: other.clone(),
        judgment: Some(PolicyJudgment::Violated {
            evidence: " ".into(),
        }),
    });
    template.findings.push(FindingSlot {
        previous: finding(1, FindingStatus::Open, None),
        reassessment: None,
    });
    template.findings.push(FindingSlot {
        previous: finding(2, FindingStatus::Open, None),
        reassessment: Some(finding(3, FindingStatus::Fixed, None)),
    });
    let issues = complete(template, vec![finding(3, FindingStatus::Open, None)]).unwrap_err();
    assert!(issues.contains(&DraftIssue::MissingDecision));
    assert!(issues.contains(&DraftIssue::MissingDepth));
    assert!(issues.contains(&DraftIssue::MissingScope));
    assert!(issues.contains(&DraftIssue::MissingPolicyJudgment(policy())));
    assert!(issues.contains(&DraftIssue::InvalidPolicyJudgment {
        policy: other,
        error: PolicyEvaluationError::MissingViolationEvidence,
    }));
    assert!(issues.contains(&DraftIssue::MissingFindingReassessment(
        FindingId::try_from(1).unwrap()
    )));
    assert!(issues.contains(&DraftIssue::FindingIdMismatch {
        expected: FindingId::try_from(2).unwrap(),
        actual: FindingId::try_from(3).unwrap(),
    }));
    assert!(
        issues.contains(&DraftIssue::Domain(AssessmentError::DuplicateFinding(
            FindingId::try_from(3).unwrap()
        )))
    );
}

#[test]
fn completion_preserves_explicit_reassessment_and_new_findings_without_inheriting_judgments() {
    let mut template = prepare(snapshot(), reviewer(), selected()).unwrap();
    template.decision = Some(ReviewDecision::ChangesRequired);
    template.depth = Some("Changed code and callers".into());
    template.scope = Some("Request handling".into());
    template.policies[0].judgment = Some(PolicyJudgment::Satisfied);
    let reassessed = finding(1, FindingStatus::Fixed, None);
    template.findings.push(FindingSlot {
        previous: finding(1, FindingStatus::Open, None),
        reassessment: Some(reassessed.clone()),
    });
    let new = finding(2, FindingStatus::Open, None);
    let completed = complete(template, vec![new.clone()]).unwrap();
    assert_eq!(completed.subject(), &subject());
    assert_eq!(completed.reviewer(), &reviewer());
    assert_eq!(completed.revision(), &revision());
    assert_eq!(completed.findings(), &[reassessed, new]);
}

#[test]
fn deleting_a_carried_slot_does_not_make_the_draft_complete() {
    let mut state = snapshot();
    state.findings.push(FindingRecord {
        created_in: 1,
        revision: revision(),
        id: provider_id("owned-root"),
        root_note: provider_id("owned-root-note"),
        author: reviewer(),
        finding: finding(1, FindingStatus::Open, None),
        body: "original".into(),
        resolved: false,
    });
    let mut template = prepare(state, reviewer(), selected()).unwrap();
    template.policies[0].judgment = Some(PolicyJudgment::Satisfied);
    template.findings.clear();
    let issues = complete(template, vec![]).unwrap_err();
    assert!(issues.contains(&DraftIssue::MissingFindingReassessment(
        FindingId::try_from(1).unwrap()
    )));
}

#[test]
fn checking_passes_the_full_source_identity_and_skips_closed_findings() {
    let active = finding(1, FindingStatus::Open, None);
    let review = assessment(vec![active.clone(), finding(2, FindingStatus::Fixed, None)]);
    let mut source = Source::new(&["first", "second"]);
    let mut renderer = Renderer::default();

    let checked = check(&review, &selected(), &mut source, &mut renderer).unwrap();

    assert_eq!(
        source.calls,
        vec![
            SourceCall::Lines {
                subject: subject(),
                revision: revision(),
                path: active.location().path().clone()
            },
            SourceCall::Position {
                subject: subject(),
                revision: revision(),
                finding: active.id()
            },
        ]
    );
    assert_eq!(renderer.findings, vec![active.id()]);
    assert_eq!(
        checked.finding_bodies,
        vec![(active.id(), "Finding 1".into())]
    );
}

#[test]
fn checking_collects_independent_failures_for_every_active_finding() {
    let review = assessment(vec![
        finding(1, FindingStatus::Open, None),
        finding(2, FindingStatus::Open, None),
    ]);
    let mut source = Source::new(&[]);
    source.fail_read = true;
    source.fail_position = true;
    let mut renderer = Renderer {
        fail: true,
        ..Renderer::default()
    };

    let issues = check(&review, &selected(), &mut source, &mut renderer).unwrap_err();

    assert_eq!(issues.len(), 6);
    for number in [1, 2] {
        let id = FindingId::try_from(number).unwrap();
        let problems: Vec<_> = issues
            .iter()
            .filter(|issue| issue.finding == Some(id))
            .map(|issue| &issue.problem)
            .collect();
        assert_eq!(problems.len(), 3);
        assert!(
            problems
                .iter()
                .any(|problem| matches!(problem, CheckProblem::Source(_)))
        );
        assert!(
            problems
                .iter()
                .any(|problem| matches!(problem, CheckProblem::Position(_)))
        );
        assert!(
            problems
                .iter()
                .any(|problem| matches!(problem, CheckProblem::Render(_)))
        );
    }
    assert_eq!(source.calls.len(), 4);
    assert_eq!(renderer.findings.len(), 2);
}

#[test]
fn suggestions_compare_the_exact_committed_range_without_normalizing_whitespace() {
    let suggestion = Suggestion::new(
        SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
            LineNumber::try_from(3).unwrap(),
        ),
        revision(),
        LineNumber::try_from(2).unwrap(),
        vec!["  first".into(), "second\r".into()],
        vec![],
        "The duplicated branch is unnecessary.".into(),
    )
    .unwrap();
    let review = assessment(vec![finding(1, FindingStatus::Open, Some(suggestion))]);
    let mut renderer = Renderer::default();
    let mut exact = Source::new(&["prefix", "  first", "second\r", "suffix"]);

    assert!(check(&review, &selected(), &mut exact, &mut renderer).is_ok());

    let mut normalized = Source::new(&["prefix", "first", "second", "suffix"]);
    let issues = check(&review, &selected(), &mut normalized, &mut renderer).unwrap_err();
    assert_eq!(issues.len(), 1);
    assert!(matches!(
        issues[0].problem,
        CheckProblem::SuggestionMismatch
    ));

    let mut truncated = Source::new(&["prefix", "  first"]);
    let issues = check(&review, &selected(), &mut truncated, &mut renderer).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|issue| matches!(issue.problem, CheckProblem::SuggestionMismatch))
    );
    assert!(
        issues
            .iter()
            .any(|issue| matches!(issue.problem, CheckProblem::LineOutsideSource))
    );
}

#[test]
fn blank_rendered_body_is_a_diagnostic_even_when_source_and_position_are_valid() {
    let review = assessment(vec![finding(1, FindingStatus::Open, None)]);
    let mut source = Source::new(&["first", "second"]);
    let mut renderer = Renderer {
        empty: true,
        ..Renderer::default()
    };

    let issues = check(&review, &selected(), &mut source, &mut renderer).unwrap_err();

    assert_eq!(issues.len(), 1);
    assert!(matches!(issues[0].problem, CheckProblem::EmptyBody));
}

#[test]
fn completion_rejects_removing_a_declared_policy_slot() {
    let mut template = prepare(snapshot(), reviewer(), selected()).unwrap();
    template.decision = Some(ReviewDecision::Ready);
    template.depth = Some("Changed code".into());
    template.scope = Some("Request handling".into());
    template.policies.clear();
    let issues = complete(template, vec![]).unwrap_err();
    assert!(
        issues
            .iter()
            .any(|issue| matches!(issue, DraftIssue::Coverage(_)))
    );
}

#[test]
fn offline_check_rejects_an_assessment_that_omits_declared_coverage() {
    let review = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer(),
        revision: revision(),
        decision: ReviewDecision::Ready,
        depth: "Changed code".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![],
        evaluations: vec![],
        findings: vec![],
    })
    .unwrap();
    let mut source = Source::new(&[]);
    let mut renderer = Renderer::default();
    let issues = check(&review, &selected(), &mut source, &mut renderer).unwrap_err();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].finding, None);
    assert!(matches!(issues[0].problem, CheckProblem::Coverage(_)));
    assert!(source.calls.is_empty());
    assert!(renderer.findings.is_empty());
}

#[test]
fn agent_declared_coverage_requires_each_judgment_during_completion() {
    let dependency = PolicyId::try_from("dependency".to_owned()).unwrap();
    let set = AssessmentPolicies::new(
        "optional-policies".into(),
        vec![
            PolicyName {
                id: policy(),
                name: "Safety".into(),
            },
            PolicyName {
                id: dependency.clone(),
                name: "Dependency".into(),
            },
        ],
    )
    .unwrap();
    let selected = set.clone();
    let mut template = prepare(snapshot(), reviewer(), selected).unwrap();
    assert_eq!(template.policies.len(), 2);
    template.decision = Some(ReviewDecision::Ready);
    template.depth = Some("Changed code".into());
    template.scope = Some("Request handling".into());
    for slot in &mut template.policies {
        slot.judgment = Some(PolicyJudgment::Satisfied);
    }
    assert!(complete(template.clone(), vec![]).is_ok());
    template
        .policies
        .retain(|slot| slot.policy_id != dependency);
    let issues = complete(template, vec![]).unwrap_err();
    assert!(issues.iter().any(|issue| matches!(issue,
        DraftIssue::Coverage(review_kit_application::use_cases::assessment::PolicyCoverageIssue::Missing(id))
            if *id == dependency
    )));
}

#[test]
fn declared_coverage_validates_names_and_unique_exact_ids() {
    use review_kit_application::use_cases::assessment::InvalidAssessmentPolicies;
    assert_eq!(
        AssessmentPolicies::new(" ".into(), vec![]),
        Err(InvalidAssessmentPolicies::BlankName),
    );
    assert_eq!(
        AssessmentPolicies::new(
            "set".into(),
            vec![PolicyName {
                id: policy(),
                name: "\t".into()
            }]
        ),
        Err(InvalidAssessmentPolicies::BlankPolicyName(policy())),
    );
    let named = PolicyName {
        id: policy(),
        name: "Safety".into(),
    };
    assert_eq!(
        AssessmentPolicies::new("set".into(), vec![named.clone(), named]),
        Err(InvalidAssessmentPolicies::DuplicatePolicy(policy())),
    );
    let exact = PolicyId::try_from(" Team/règle.v2 ".to_owned()).unwrap();
    let scope = AssessmentPolicies::new(
        " Named set ".into(),
        vec![PolicyName {
            id: exact.clone(),
            name: " Rule name ".into(),
        }],
    )
    .unwrap();
    assert_eq!(scope.name(), " Named set ");
    assert_eq!(scope.ids(), vec![exact]);
    assert_eq!(scope.policies()[0].name, " Rule name ");
}

#[test]
fn declared_coverage_order_has_one_canonical_representation() {
    let one = PolicyName {
        id: PolicyId::try_from("one".to_owned()).unwrap(),
        name: "One".into(),
    };
    let two = PolicyName {
        id: PolicyId::try_from("two".to_owned()).unwrap(),
        name: "Two".into(),
    };
    let forward = AssessmentPolicies::new("set".into(), vec![one.clone(), two.clone()]).unwrap();
    let reversed = AssessmentPolicies::new("set".into(), vec![two, one]).unwrap();
    assert_eq!(forward, reversed);
}

fn recorded_round(
    number: u64,
    confirmed_validation: bool,
    revision: Revision,
    findings: Vec<Finding>,
) -> Round {
    let decision = if findings.iter().all(Finding::validated) {
        ReviewDecision::ChangesRequired
    } else {
        ReviewDecision::Blocked
    };
    Round {
        number,
        assessment: Assessment::new(AssessmentDraft {
            subject: subject(),
            reviewer: reviewer(),
            revision,
            decision,
            depth: "Changed code and callers".into(),
            scope: "Request handling".into(),
            limitation: (decision == ReviewDecision::Blocked)
                .then(|| "The finding awaits independent confirmation.".into()),
            selected_policies: vec![policy()],
            evaluations: vec![
                PolicyEvaluation::new(
                    policy(),
                    PolicyJudgment::Violated {
                        evidence: "The guard is missing.".into(),
                    },
                )
                .unwrap(),
            ],
            findings,
        })
        .unwrap(),
        status: RoundStatus::Completed,
        abandonment: None,
        publication_limitation: None,
        answers: vec![],
        transitions: vec![],
        answer_targets: vec![],
        actions: vec![],
        review_workflow: None,
        policy_set: None,
        policy_names: vec![],
        policy_declarations: vec![],
        renderer: None,
        communicated_findings: None,
        verified_resolutions: vec![],
        confirmed_validation,
    }
}

fn reusable_after(rounds: Vec<Round>, author: ReviewerId, captured: &Finding) -> bool {
    let mut rounds = rounds;
    let current = rounds.pop().unwrap();
    let mut state = snapshot();
    state.summaries.push(SummaryRecord {
        id: provider_id("summary"),
        author,
        body: "summary".into(),
        current,
        history: rounds,
    });
    reusable_confirmations(&state, &reviewer(), std::slice::from_ref(captured))
        .iter()
        .any(|finding| finding == captured)
}

#[test]
fn only_a_round_accepted_with_confirmations_at_this_revision_records_a_reusable_confirmation() {
    let captured = finding(1, FindingStatus::Open, None);
    let at = |number, confirmed, findings| recorded_round(number, confirmed, revision(), findings);
    let older = Revision::new("older".into(), vec!["base".into(), "start".into()]).unwrap();
    assert!(reusable_after(
        vec![at(1, true, vec![captured.clone()])],
        reviewer(),
        &captured
    ));
    assert!(
        !reusable_after(
            vec![at(1, false, vec![captured.clone()])],
            reviewer(),
            &captured
        ),
        "validation accepted without confirmation enforcement records no confirmation"
    );
    assert!(
        reusable_after(
            vec![
                at(1, false, vec![captured.clone()]),
                at(2, true, vec![captured.clone()])
            ],
            reviewer(),
            &captured
        ),
        "a later enforced round confirms the same version"
    );
    assert!(
        !reusable_after(
            vec![recorded_round(1, true, older, vec![captured.clone()])],
            reviewer(),
            &captured
        ),
        "a confirmation at another revision is not reusable"
    );
    let reworded = Finding::new(FindingDraft {
        id: captured.id(),
        policy_id: policy(),
        title: captured.title().into(),
        body: captured.body().into(),
        severity: captured.severity(),
        disposition: captured.disposition(),
        status: captured.status(),
        evidence: "Reproduced with a foreign administrator.".into(),
        validated: true,
        location: captured.location().clone(),
        suggestion: None,
    })
    .unwrap();
    assert!(
        !reusable_after(vec![at(1, true, vec![reworded])], reviewer(), &captured),
        "the recorded confirmation covered different content"
    );
    assert!(
        !reusable_after(
            vec![at(1, true, vec![captured.clone().with_validated(false)])],
            reviewer(),
            &captured
        ),
        "the enforced round left the finding unvalidated"
    );
    let unvalidated = captured.clone().with_validated(false);
    assert!(!reusable_after(
        vec![at(1, true, vec![captured.clone()])],
        reviewer(),
        &unvalidated
    ));
    let fixed = finding(1, FindingStatus::Fixed, None);
    assert!(
        !reusable_after(vec![at(1, true, vec![fixed.clone()])], reviewer(), &fixed),
        "only open or uncertain blocking findings need a confirmation"
    );
    assert!(!reusable_after(
        vec![at(1, true, vec![captured.clone()])],
        ReviewerId::try_from("someone-else".to_owned()).unwrap(),
        &captured
    ));
    let rebased = Revision::new("head".into(), vec!["other-base".into(), "start".into()]).unwrap();
    assert!(
        !reusable_after(
            vec![recorded_round(1, true, rebased, vec![captured.clone()])],
            reviewer(),
            &captured
        ),
        "the same head against another comparison is another revision"
    );
    for status in [RoundStatus::Pending, RoundStatus::Abandoned] {
        let mut round = at(1, true, vec![captured.clone()]);
        round.status = status;
        assert!(
            reusable_after(vec![round], reviewer(), &captured),
            "the mark records enforced acceptance, not publication: {status:?}"
        );
    }
}
