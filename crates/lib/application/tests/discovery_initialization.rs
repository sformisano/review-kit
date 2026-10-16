#![allow(clippy::unwrap_used)]

use review_kit_application::use_cases::assessment::AssessmentPolicies;

use std::io;

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::*,
    use_cases::{discovery_bundle::*, initialization::*},
};
use review_kit_domain::*;

#[path = "support/operation_logs.rs"]
mod operation_logs;

#[path = "discovery_initialization/implicit_observation.rs"]
mod implicit_observation;

fn subject() -> Subject {
    Subject::new("gitlab".into(), "host".into(), "group/repo".into(), 7).unwrap()
}
fn reviewer() -> ReviewerId {
    ReviewerId::try_from("agent".to_owned()).unwrap()
}
fn other() -> ReviewerId {
    ReviewerId::try_from("other".to_owned()).unwrap()
}
fn revision() -> Revision {
    Revision::new("head".into(), vec!["base".into(), "start".into()]).unwrap()
}
fn id(value: &str) -> ProviderId {
    ProviderId::new(value.into()).unwrap()
}
fn policy() -> PolicyId {
    PolicyId::try_from("correctness".to_owned()).unwrap()
}
fn path(value: &str) -> SourcePath {
    SourcePath::try_from(value.to_owned()).unwrap()
}
fn failure(operation: &'static str) -> PortError {
    PortError::new(operation, io::Error::other("injected failure"))
}

fn finding(number: u64, status: FindingStatus) -> Finding {
    Finding::new(FindingDraft {
        id: FindingId::try_from(number).unwrap(),
        policy_id: policy(),
        title: "Lost request".into(),
        body: "A failed request is dropped.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Regression".into(),
        validated: true,
        location: SourceLocation::new(path("src/lib.rs"), LineNumber::try_from(1).unwrap()),
        suggestion: None,
    })
    .unwrap()
}
fn assessment(findings: Vec<Finding>) -> Assessment {
    let decision = if findings
        .iter()
        .any(|finding| finding.status() == FindingStatus::Open)
    {
        ReviewDecision::ChangesRequired
    } else {
        ReviewDecision::Ready
    };
    Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer(),
        revision: revision(),
        decision,
        depth: "Focused review".into(),
        scope: "Changed code".into(),
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
fn root(number: u64, author: ReviewerId) -> FindingRecord {
    FindingRecord {
        id: id(&format!("thread-{number}")),
        root_note: id(&format!("note-{number}")),
        author,
        created_in: 1,
        revision: revision(),
        finding: finding(number, FindingStatus::Open),
        body: "Published root".into(),
        resolved: false,
    }
}
fn observed_changes() -> ObservedChanges {
    ObservedChanges {
        subject: subject(),
        revision: revision(),
        files: vec![
            ChangedFile::Modified(path("src/lib.rs")),
            ChangedFile::Renamed {
                previous: path("old.rs"),
                current: path("new.rs"),
            },
            ChangedFile::Deleted(path("gone.rs")),
        ],
        unified_diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
    }
}
struct Reader {
    snapshot: Snapshot,
    reads: usize,
    drift: bool,
    fail: bool,
}
impl ReviewReader for Reader {
    fn discover(&mut self, wanted: &Subject) -> Result<Snapshot, PortError> {
        assert_eq!(wanted, &subject());
        self.reads += 1;
        if self.fail {
            return Err(failure("discover"));
        }
        if self.drift && self.reads == 2 {
            self.snapshot.revision = Revision::new("new-head".into(), vec!["base".into()]).unwrap();
        }
        Ok(self.snapshot.clone())
    }
}
struct Changes {
    observed: ObservedChanges,
    calls: usize,
    fail: bool,
}
impl ReviewChanges for Changes {
    fn read_changes(
        &mut self,
        wanted: &Subject,
        refs: &Revision,
    ) -> Result<ObservedChanges, PortError> {
        assert_eq!(wanted, &subject());
        assert_eq!(refs, &revision());
        self.calls += 1;
        if self.fail {
            Err(failure("changes"))
        } else {
            Ok(self.observed.clone())
        }
    }
}
fn ports(state: Snapshot) -> (Reader, Changes) {
    (
        Reader {
            snapshot: state,
            reads: 0,
            drift: false,
            fail: false,
        },
        Changes {
            observed: observed_changes(),
            calls: 0,
            fail: false,
        },
    )
}
fn bundle(state: Snapshot) -> DiscoveryBundle {
    let (mut reader, mut changes) = ports(state);
    discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()).unwrap()
}

#[test]
fn discovery_collects_exact_diff_and_changed_file_roles_without_writes() {
    let (mut reader, mut changes) = ports(snapshot());
    let result = discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()).unwrap();
    assert_eq!(result.changes(), &observed_changes());
    assert_eq!(reader.reads, 2);
    assert_eq!(changes.calls, 1);
    assert_eq!(result.next_finding_id().get(), 1);
    assert!(result.findings().is_empty());
}

#[test]
fn discovery_rejects_changed_or_mislabeled_revision_observations() {
    for wrong_diff in [true, false] {
        let (mut reader, mut changes) = ports(snapshot());
        if wrong_diff {
            changes.observed.revision = Revision::new("other-head".into(), vec![]).unwrap();
        } else {
            reader.drift = true;
        }
        assert!(matches!(
            discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
            Err(BundleError::ChangedRevision)
        ));
    }
    let (mut reader, mut changes) = ports(snapshot());
    changes.observed.subject =
        Subject::new("gitlab".into(), "host".into(), "group/other".into(), 7).unwrap();
    assert!(matches!(
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
        Err(BundleError::WrongSubject)
    ));
}

#[test]
fn discovery_preserves_read_failure_sources() {
    use std::error::Error;
    let (mut reader, mut changes) = ports(snapshot());
    reader.fail = true;
    let error = discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()).unwrap_err();
    assert!(matches!(error, BundleError::Discovery(_)));
    assert!(error.source().is_some());
    assert_eq!(changes.calls, 0);
    reader.fail = false;
    changes.fail = true;
    assert!(matches!(
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
        Err(BundleError::Changes(_))
    ));
}

#[test]
fn discovery_reconciles_verified_transitions_and_reserves_unpublished_and_orphan_ids() {
    let mut state = snapshot();
    state.findings = vec![root(1, reviewer()), root(99, other())];
    let transition = PlannedAction::Reply {
        finding: FindingId::try_from(1).unwrap(),
        kind: ReplyKind::Transition(FindingStatus::Fixed),
        body: "Fixed explanation".into(),
    };
    state.summaries.push(SummaryRecord {
        id: id("summary"),
        author: reviewer(),
        body: "Pending overview".into(),
        history: vec![],
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
            assessment: assessment(vec![
                finding(1, FindingStatus::Fixed),
                finding(10, FindingStatus::Open),
            ]),
            status: RoundStatus::Pending,
            answers: vec![],
            transitions: vec![TransitionReply {
                finding: FindingId::try_from(1).unwrap(),
                from: FindingStatus::Open,
                to: FindingStatus::Fixed,
                body: "Fixed explanation".into(),
            }],
            answer_targets: vec![],
            actions: vec![
                transition,
                PlannedAction::Finding {
                    finding: Box::new(finding(10, FindingStatus::Open)),
                    body: "New finding".into(),
                },
            ],
        },
    });
    state.replies.push(ReplyRecord {
        id: id("transition"),
        author: reviewer(),
        thread: id("thread-1"),
        body: "Fixed explanation".into(),
        action: Some(ActionId {
            round: 2,
            finding: FindingId::try_from(1).unwrap(),
            kind: ReplyKind::Transition(FindingStatus::Fixed),
        }),
    });
    state.replies.push(ReplyRecord {
        id: id("orphan"),
        author: reviewer(),
        thread: id("removed-thread"),
        body: "Reserved".into(),
        action: Some(ActionId {
            round: 1,
            finding: FindingId::try_from(20).unwrap(),
            kind: ReplyKind::Transition(FindingStatus::Open),
        }),
    });
    state.replies.push(ReplyRecord {
        id: id("question"),
        author: other(),
        thread: id("thread-1"),
        body: "How was it fixed?".into(),
        action: None,
    });
    state.replies.push(ReplyRecord {
        id: id("unrelated"),
        author: other(),
        thread: id("thread-99"),
        body: "Other review".into(),
        action: None,
    });
    let result = bundle(state.clone());
    assert_eq!(result.next_finding_id().get(), 21);
    assert_eq!(result.findings().len(), 1);
    assert_eq!(result.findings()[0].status(), FindingStatus::Fixed);
    assert_eq!(result.foreign_replies().len(), 1);
    assert_eq!(result.foreign_replies()[0].id, id("question"));
    let initialized = initialize(&result, policy_set(), &mut Writer::default()).unwrap();
    assert_eq!(
        initialized.draft.template.findings[0].previous.status(),
        FindingStatus::Fixed
    );
    assert!(
        initialized.draft.template.findings[0]
            .reassessment
            .is_none()
    );
    state.replies.retain(|reply| reply.id != id("transition"));
    assert_eq!(bundle(state).findings()[0].status(), FindingStatus::Open);
}

#[test]
fn discovery_does_not_wrap_exhausted_finding_identity() {
    let mut state = snapshot();
    state.findings.push(root(u64::MAX, reviewer()));
    let (mut reader, mut changes) = ports(state);
    assert!(matches!(
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
        Err(BundleError::FindingIdExhausted)
    ));
}

#[test]
fn discovery_rejects_completed_communication_when_its_transition_reply_is_missing() {
    let mut state = snapshot();
    state.findings.push(root(1, reviewer()));
    state.summaries.push(SummaryRecord {
        id: id("summary"),
        author: reviewer(),
        body: "Completed overview".into(),
        history: vec![],
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
            assessment: assessment(vec![finding(1, FindingStatus::Fixed)]),
            status: RoundStatus::Completed,
            answers: vec![],
            transitions: vec![TransitionReply {
                finding: FindingId::try_from(1).unwrap(),
                from: FindingStatus::Open,
                to: FindingStatus::Fixed,
                body: "Fixed explanation".into(),
            }],
            answer_targets: vec![],
            actions: vec![PlannedAction::Reply {
                finding: FindingId::try_from(1).unwrap(),
                kind: ReplyKind::Transition(FindingStatus::Fixed),
                body: "Fixed explanation".into(),
            }],
        },
    });
    let (mut reader, mut changes) = ports(state);
    assert!(matches!(
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
        Err(BundleError::FindingState(_))
    ));
}

#[test]
fn discovery_rejects_a_pending_assessed_finding_without_a_root_or_retained_create() {
    let mut state = snapshot();
    state.summaries.push(SummaryRecord {
        id: id("summary"),
        author: reviewer(),
        body: "Pending overview".into(),
        history: vec![],
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
            number: 1,
            assessment: assessment(vec![finding(1, FindingStatus::Open)]),
            status: RoundStatus::Pending,
            answers: vec![],
            transitions: vec![],
            answer_targets: vec![],
            actions: vec![],
        },
    });
    let (mut reader, mut changes) = ports(state);
    assert!(matches!(
        discover_bundle(&mut reader, &mut changes, &subject(), &reviewer()),
        Err(BundleError::FindingState(_))
    ));
}

#[test]
fn discovery_requires_carried_roots_in_unfinished_current_and_historical_rounds() {
    for status in [RoundStatus::Partial, RoundStatus::Superseded] {
        for historical in [false, true] {
            let mut state = snapshot();
            state.findings.push(root(1, reviewer()));
            let round = Round {
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
                assessment: assessment(vec![
                    finding(1, FindingStatus::Open),
                    finding(10, FindingStatus::Open),
                ]),
                status,
                answers: vec![],
                transitions: vec![],
                answer_targets: vec![],
                actions: vec![PlannedAction::Finding {
                    finding: Box::new(finding(10, FindingStatus::Open)),
                    body: "Unpublished finding".into(),
                }],
            };
            state.summaries.push(SummaryRecord {
                id: id("summary"),
                author: reviewer(),
                body: "Unfinished overview".into(),
                history: vec![],
                current: round.clone(),
            });
            let discovered = bundle(state.clone());
            assert_eq!(discovered.findings().len(), 1);
            assert_eq!(
                discovered.next_finding_id(),
                FindingId::try_from(11).unwrap()
            );
            state.findings.clear();
            if historical {
                let summary = &mut state.summaries[0];
                summary.history.push(round);
                summary.current = Round {
                    review_workflow: None,
                    policy_set: None,
                    policy_names: vec![review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }],
            policy_declarations: vec![review_kit_application::publication::PublicPolicyDeclaration { policy: review_kit_application::publication::PolicyName { id: policy(), name: "Correctness".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed control flow requires this policy.".into() }],
                    renderer: None,
                    communicated_findings: None,
                    verified_resolutions: Vec::new(),
                    number: 3,
                    assessment: assessment(vec![]),
                    status: RoundStatus::Completed,
                    actions: vec![],
                    ..summary.current.clone()
                };
            }
            let (mut reader, mut changes) = ports(state);
            let outcome = discover_bundle(&mut reader, &mut changes, &subject(), &reviewer());
            assert!(matches!(outcome, Err(BundleError::FindingState(
                review_kit_application::use_cases::publishing::PlanError::MissingPublishedFinding(id)
            )) if id == FindingId::try_from(1).unwrap()));
        }
    }
}

#[test]
fn discovery_allows_blocked_new_findings_without_create_actions() {
    let mut state = snapshot();
    state.summaries.push(SummaryRecord {
        id: id("summary"),
        author: reviewer(),
        body: "Blocked overview".into(),
        history: vec![],
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
            number: 1,
            assessment: Assessment::new(AssessmentDraft {
                subject: subject(),
                reviewer: reviewer(),
                revision: revision(),
                decision: ReviewDecision::Blocked,
                depth: "Focused review".into(),
                scope: "Changed code".into(),
                limitation: Some("CI unavailable".into()),
                selected_policies: vec![policy()],
                evaluations: vec![
                    PolicyEvaluation::new(
                        policy(),
                        PolicyJudgment::Blocked {
                            limitation: "CI unavailable".into(),
                        },
                    )
                    .unwrap(),
                ],
                findings: vec![finding(10, FindingStatus::Uncertain)],
            })
            .unwrap(),
            status: RoundStatus::Blocked,
            answers: vec![],
            transitions: vec![],
            answer_targets: vec![],
            actions: vec![],
        },
    });
    let discovered = bundle(state);
    assert!(discovered.findings().is_empty());
    assert_eq!(
        discovered.next_finding_id(),
        FindingId::try_from(11).unwrap()
    );
}

fn policy_set() -> AssessmentPolicies {
    AssessmentPolicies::new(
        "rust".into(),
        vec![
            PolicyName {
                id: policy(),
                name: "Correctness".into(),
            },
            PolicyName {
                id: PolicyId::try_from("dependency".to_owned()).unwrap(),
                name: "Dependency".into(),
            },
            PolicyName {
                id: PolicyId::try_from("optional".to_owned()).unwrap(),
                name: "Optional".into(),
            },
        ],
    )
    .unwrap()
}
#[derive(Default)]
struct Writer {
    saved: Option<InitializedAssessment>,
    calls: usize,
    fail: bool,
}
impl AssessmentDraftWriter for Writer {
    fn create_new(&mut self, draft: &InitializedAssessment) -> Result<DraftCreation, PortError> {
        self.calls += 1;
        if self.fail {
            return Err(failure("create draft"));
        }
        if self.saved.is_some() {
            return Ok(DraftCreation::Existing);
        }
        self.saved = Some(draft.clone());
        Ok(DraftCreation::Created)
    }
}

#[test]
fn initialization_keeps_declared_names_and_requires_fresh_judgments() {
    let mut state = snapshot();
    state.findings.push(root(1, reviewer()));
    let discovered = bundle(state);
    let mut writer = Writer::default();
    let result = initialize(&discovered, policy_set(), &mut writer).unwrap();
    assert_eq!(result.outcome, DraftCreation::Created);
    assert_eq!(result.draft.policies().name(), "rust");
    assert_eq!(result.draft.policies().policies()[0].name, "Correctness");
    assert!(
        result
            .draft
            .template
            .policies
            .iter()
            .all(|slot| slot.judgment.is_none())
    );
    assert_eq!(result.draft.template.findings.len(), 1);
    assert!(result.draft.template.findings[0].reassessment.is_none());
    assert!(result.draft.template.decision.is_none());
    assert!(result.draft.template.depth.is_none());
    assert!(result.draft.template.scope.is_none());
    assert_eq!(writer.saved, Some(result.draft));
}

#[test]
fn initialization_never_overwrites_an_existing_draft_even_if_coverage_differs() {
    let discovered = bundle(snapshot());
    let mut writer = Writer::default();
    let first = initialize(&discovered, policy_set(), &mut writer).unwrap();
    let other_coverage = AssessmentPolicies::new("other-policy-set".into(), vec![]).unwrap();
    let second = initialize(&discovered, other_coverage, &mut writer).unwrap();
    assert_ne!(first.draft.policies(), second.draft.policies());
    assert_eq!(second.outcome, DraftCreation::Existing);
    assert_eq!(writer.saved, Some(first.draft));
    assert_eq!(writer.calls, 2);
}

#[test]
fn initialization_reports_writer_failure_without_claiming_creation() {
    let discovered = bundle(snapshot());
    let mut writer = Writer {
        fail: true,
        ..Writer::default()
    };
    assert!(matches!(
        initialize(&discovered, policy_set(), &mut writer),
        Err(InitializationError::Write(_))
    ));
    assert!(writer.saved.is_none());
}
