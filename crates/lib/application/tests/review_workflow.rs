#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{cell::RefCell, io, rc::Rc};

use review_kit_application::use_cases::assessment::AssessmentPolicies;
use review_kit_application::{
    ports::*,
    publication::*,
    summary::{PublishedAction, SummaryView},
    use_cases::publishing::{
        PlanError, PublicationProgress, PublicationResult, PublicationStatus, PublishRequest, Stop,
        plan, plan_from_snapshot, publish as publish_with_progress,
    },
};
use review_kit_domain::*;

type Log = Rc<RefCell<Vec<String>>>;

#[path = "support/operation_logs.rs"]
mod operation_logs;

#[path = "review_workflow/operation_logging.rs"]
mod operation_logging;

#[path = "review_workflow/bounded_recovery.rs"]
mod bounded_recovery;

#[path = "review_workflow/implicit_observation.rs"]
mod implicit_observation;

#[path = "review_workflow/policy_declarations.rs"]
mod policy_declarations;

#[path = "review_workflow/blocked_findings.rs"]
mod blocked_findings;

#[path = "review_workflow/conversation_freshness.rs"]
mod conversation_freshness;

fn publish(
    request: &PublishRequest,
    host: &mut Host,
    source: &mut Source,
    renderer: &mut Renderer,
) -> Result<PublicationResult, PlanError> {
    let mut progress = std::mem::take(&mut host.progress);
    let result = publish_with_progress(request, host, source, renderer, &mut progress);
    host.progress = progress;
    result
}

fn publish_recorded(
    request: &PublishRequest,
    host: &mut Host,
    source: &mut Source,
    renderer: &mut Renderer,
    evidence: &mut dyn review_kit_application::use_cases::publication_run::ExecutionEvidence,
) -> Result<PublicationResult, PlanError> {
    let mut progress = std::mem::take(&mut host.progress);
    let result = review_kit_application::use_cases::publishing::publish_recorded(
        request,
        host,
        source,
        renderer,
        &mut progress,
        evidence,
    );
    host.progress = progress;
    result
}

fn abandon_observed(
    request: &review_kit_application::use_cases::publishing::AbandonRequest,
    host: &mut Host,
    renderer: &mut Renderer,
) -> Result<PublicationResult, PlanError> {
    let mut progress = std::mem::take(&mut host.progress);
    let result = review_kit_application::use_cases::publishing::abandon(
        request,
        host,
        renderer,
        &mut progress,
    );
    host.progress = progress;
    result
}

fn abandon_recorded(
    request: &review_kit_application::use_cases::publishing::AbandonRequest,
    host: &mut Host,
    renderer: &mut Renderer,
    evidence: &mut dyn review_kit_application::use_cases::publication_run::ExecutionEvidence,
) -> Result<PublicationResult, PlanError> {
    let mut progress = std::mem::take(&mut host.progress);
    let result = review_kit_application::use_cases::publishing::abandon_recorded(
        request,
        host,
        renderer,
        &mut progress,
        evidence,
    );
    host.progress = progress;
    result
}

fn observe_pending_create(host: &mut Host) {
    let pending = host.progress.unresolved.as_ref().unwrap().clone();
    host.apply(&pending.reviewer, &pending.write);
}

fn failure(operation: &'static str) -> PortError {
    PortError::new(operation, io::Error::other("injected failure"))
}
fn reviewer(name: &str) -> ReviewerId {
    ReviewerId::try_from(name.to_owned()).unwrap()
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
fn provider_id(name: &str) -> ProviderId {
    ProviderId::new(name.to_owned()).unwrap()
}
fn subject() -> Subject {
    Subject::new("gitlab".into(), "host".into(), "group/repo".into(), 7).unwrap()
}
fn revision() -> Revision {
    Revision::new("head".into(), vec!["base".into(), "start".into()]).unwrap()
}
fn policy() -> PolicyId {
    PolicyId::try_from("correctness".to_owned()).unwrap()
}
fn finding(status: FindingStatus) -> Finding {
    Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: policy(),
        title: "Lost request".into(),
        body: "The failure path drops the request.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Regression case".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
            LineNumber::try_from(1).unwrap(),
        ),
        suggestion: None,
    })
    .unwrap()
}
fn request(label: &str, status: Option<FindingStatus>, blocked: bool) -> PublishRequest {
    let judgment = if blocked {
        PolicyJudgment::Blocked {
            limitation: "CI unavailable".into(),
        }
    } else {
        PolicyJudgment::Satisfied
    };
    PublishRequest {
        confirmed_validation: false,
        review_workflow: None,
        assessment: Assessment::new(AssessmentDraft {
            subject: subject(),
            reviewer: reviewer("agent"),
            revision: revision(),
            decision: if blocked {
                ReviewDecision::Blocked
            } else if status == Some(FindingStatus::Open) {
                ReviewDecision::ChangesRequired
            } else {
                ReviewDecision::Ready
            },
            depth: "Changed code and callers".into(),
            scope: "Request handling".into(),
            limitation: blocked.then(|| "CI unavailable".into()),
            selected_policies: vec![policy()],
            evaluations: vec![PolicyEvaluation::new(policy(), judgment).unwrap()],
            findings: status.map(finding).into_iter().collect(),
        })
        .unwrap(),
        policies: selected(),
        policy_declarations: vec![PublicPolicyDeclaration {
            policy: selected().policies()[0].clone(),
            status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
            reason: "The changed request handling requires this policy.".into(),
        }],
        transitions: match status {
            Some(to @ (FindingStatus::Fixed | FindingStatus::Withdrawn)) => vec![TransitionReply {
                finding: FindingId::try_from(1).unwrap(),
                from: FindingStatus::Open,
                to,
                body: "Regression case".into(),
            }],
            Some(FindingStatus::Open) if label == "recurrence" || label == "regressed" => {
                vec![TransitionReply {
                    finding: FindingId::try_from(1).unwrap(),
                    from: FindingStatus::Fixed,
                    to: FindingStatus::Open,
                    body: "Regression case".into(),
                }]
            }
            _ => vec![],
        },
        local_context: String::new(),
        effects: Effects::review(),
        answers: vec![],
        answer_targets: vec![],
        allow_colocated: false,
    }
}

#[derive(Clone, Copy)]
enum Fault {
    UnknownCommitted(Effect),
    UnknownAbsent(Effect),
    Rejected(Effect),
    UnknownAbsentFinding(FindingId),
}

struct Host {
    progress: PublicationProgress,
    snapshot: Snapshot,
    authority: Effects,
    writes: Vec<Write>,
    log: Log,
    fault: Option<Fault>,
    drift_after_pending: bool,
    drift_after_finding: bool,
    drift_after_unknown: bool,
    drift_on_discovery: Option<usize>,
    authenticated: ReviewerId,
    reverse_pagination: bool,
    discoveries: usize,
    fail_on_discovery: Option<usize>,
    edit_reply_on_second_read: bool,
    edit_summary_on_discovery: Option<usize>,
    authority_reads: usize,
    narrow_on_third_authority: bool,
    next_root: usize,
    reject_finding: Option<FindingId>,
    delete_root_on_discovery: Option<usize>,
    duplicate_root_on_discovery: Option<usize>,
}
impl Host {
    fn new(log: Log) -> Self {
        let foreign = FindingRecord {
            id: provider_id("foreign-root"),
            root_note: provider_id("foreign-root-note"),
            author: reviewer("other"),
            finding: finding(FindingStatus::Open),
            body: "forged owned marker".into(),
            resolved: false,
            created_in: 1,
            revision: revision(),
        };
        Self {
            progress: PublicationProgress::default(),
            snapshot: Snapshot {
                subject: subject(),
                reviewer: reviewer("agent"),
                revision: revision(),
                summaries: vec![],
                findings: vec![foreign],
                replies: vec![],
                permissions: Effects::review(),
            },
            authority: Effects::review(),
            writes: vec![],
            log,
            fault: None,
            drift_after_pending: false,
            drift_after_finding: false,
            drift_after_unknown: false,
            drift_on_discovery: None,
            authenticated: reviewer("agent"),
            reverse_pagination: false,
            discoveries: 0,
            fail_on_discovery: None,
            edit_reply_on_second_read: false,
            edit_summary_on_discovery: None,
            authority_reads: 0,
            narrow_on_third_authority: false,
            next_root: 0,
            reject_finding: None,
            delete_root_on_discovery: None,
            duplicate_root_on_discovery: None,
        }
    }
    fn apply(&mut self, author: &ReviewerId, write: &Write) {
        match write {
            Write::Summary {
                expected,
                round,
                history,
                body,
            } => {
                let actual = self
                    .snapshot
                    .summaries
                    .iter()
                    .find(|s| &s.author == author)
                    .cloned();
                assert_eq!(
                    actual.as_ref(),
                    expected.as_deref(),
                    "summary compare contract"
                );
                self.snapshot.summaries.retain(|s| &s.author != author);
                self.snapshot.summaries.push(SummaryRecord {
                    id: provider_id("summary"),
                    author: author.clone(),
                    body: body.clone(),
                    current: *round.clone(),
                    history: history.clone(),
                });
                if self.drift_after_pending && round.status == RoundStatus::Pending {
                    self.snapshot.revision =
                        Revision::new("head".into(), vec!["different-base".into(), "start".into()])
                            .unwrap();
                    self.drift_after_pending = false;
                }
            }
            Write::Finding {
                finding,
                body,
                round,
                revision,
            } => {
                self.next_root += 1;
                let suffix = if self.next_root == 1 {
                    String::new()
                } else {
                    format!("-{}", self.next_root)
                };
                self.snapshot.findings.push(FindingRecord {
                    id: provider_id(&format!("owned-root{suffix}")),
                    root_note: provider_id(&format!("owned-root-note{suffix}")),
                    author: author.clone(),
                    finding: *finding.clone(),
                    body: body.clone(),
                    resolved: false,
                    created_in: *round,
                    revision: revision.clone(),
                });
            }
            Write::Reply {
                thread,
                action,
                body,
            } => self.snapshot.replies.push(ReplyRecord {
                id: provider_id(&format!("reply-{}", self.snapshot.replies.len())),
                author: author.clone(),
                thread: thread.clone(),
                action: Some(action.clone()),
                body: body.clone(),
            }),
            Write::SetResolved { thread, resolved } => {
                let root = self
                    .snapshot
                    .findings
                    .iter_mut()
                    .find(|f| &f.id == thread)
                    .unwrap();
                assert_eq!(&root.author, author, "foreign threads cannot be mutated");
                root.resolved = *resolved;
            }
        }
    }
}
struct Session<'a>(&'a mut Host);
impl PublicationHost for Host {
    fn acquire(
        &mut self,
        wanted: &Subject,
        author: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError> {
        assert_eq!(wanted, &self.snapshot.subject);
        assert_eq!(author, &reviewer("agent"));
        self.log.borrow_mut().push("acquire".into());
        Ok(Box::new(Session(self)))
    }
}
impl ReviewReader for Session<'_> {
    fn discover(&mut self, wanted: &Subject) -> Result<Snapshot, PortError> {
        assert_eq!(wanted, &self.0.snapshot.subject);
        self.0.log.borrow_mut().push("discover".into());
        self.0.discoveries += 1;
        if self.0.duplicate_root_on_discovery == Some(self.0.discoveries)
            && let Some(mut duplicate) = self
                .0
                .snapshot
                .findings
                .iter()
                .find(|root| root.author == reviewer("agent"))
                .cloned()
        {
            duplicate.id = provider_id("duplicate-root");
            duplicate.root_note = provider_id("duplicate-note");
            self.0.snapshot.findings.push(duplicate);
        }
        if self.0.fail_on_discovery == Some(self.0.discoveries) {
            return Err(failure("discover"));
        }
        if self.0.delete_root_on_discovery == Some(self.0.discoveries) {
            self.0
                .snapshot
                .findings
                .retain(|root| root.author != reviewer("agent"));
        }
        if self.0.edit_summary_on_discovery == Some(self.0.discoveries) {
            self.0.snapshot.summaries[0].body = "Concurrent summary edit".into();
        }
        if self.0.drift_on_discovery == Some(self.0.discoveries) {
            self.0.snapshot.revision =
                Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap();
        }
        if self.0.edit_reply_on_second_read && self.0.discoveries == 2 {
            self.0
                .snapshot
                .replies
                .iter_mut()
                .find(|r| r.author == reviewer("other"))
                .unwrap()
                .body = "Changed question".into();
        }
        let mut observed = self.0.snapshot.clone();
        if self.0.reverse_pagination && self.0.discoveries.is_multiple_of(2) {
            observed.findings.reverse();
            observed.replies.reverse();
            observed.summaries.reverse();
        }
        Ok(observed)
    }
}
impl PublicationSession for Session<'_> {
    fn authenticated_reviewer(&self) -> &ReviewerId {
        &self.0.authenticated
    }
    fn authority(&mut self) -> Result<Effects, PortError> {
        self.0.log.borrow_mut().push("authority".into());
        self.0.authority_reads += 1;
        if self.0.narrow_on_third_authority && self.0.authority_reads == 3 {
            return Ok(Effects::new([Effect::CreateSummary, Effect::UpdateSummary]));
        }
        Ok(self.0.authority.clone())
    }
    fn write(&mut self, wanted: &Subject, author: &ReviewerId, write: &Write) -> WriteOutcome {
        assert_eq!(wanted, &self.0.snapshot.subject);
        self.0
            .log
            .borrow_mut()
            .push(format!("write:{:?}", write.effect()));
        self.0.writes.push(write.clone());
        if matches!(write, Write::Finding { finding, .. } if self.0.reject_finding == Some(finding.id()))
        {
            self.0.reject_finding = None;
            return WriteOutcome::Rejected(failure("write"));
        }
        let matches = self.0.fault.is_some_and(|fault| match fault {
            Fault::UnknownCommitted(effect)
            | Fault::UnknownAbsent(effect)
            | Fault::Rejected(effect) => effect == write.effect(),
            Fault::UnknownAbsentFinding(id) => {
                matches!(write, Write::Finding { finding, .. } if finding.id() == id)
            }
        });
        if matches {
            match self.0.fault.take().unwrap() {
                Fault::UnknownCommitted(_) => {
                    self.0.apply(author, write);
                    return WriteOutcome::Unknown(failure("response"));
                }
                Fault::UnknownAbsent(_) | Fault::UnknownAbsentFinding(_) => {
                    if self.0.drift_after_unknown {
                        self.0.snapshot.revision =
                            Revision::new("new-head".into(), vec!["base".into(), "start".into()])
                                .unwrap();
                    }
                    return WriteOutcome::Unknown(failure("response"));
                }
                Fault::Rejected(_) => return WriteOutcome::Rejected(failure("write")),
            }
        }
        self.0.apply(author, write);
        if self.0.drift_after_finding && matches!(write, Write::Finding { .. }) {
            self.0.snapshot.revision =
                Revision::new("head".into(), vec!["new-base".into(), "start".into()]).unwrap();
            self.0.drift_after_finding = false;
        }
        WriteOutcome::Acknowledged
    }
}

struct Source {
    log: Log,
    fail: bool,
    expected_revision: Revision,
}
impl CommittedSource for Source {
    fn lines(
        &mut self,
        wanted: &Subject,
        at: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        assert_eq!(wanted, &subject());
        assert_eq!(at, &self.expected_revision);
        assert_eq!(path.as_str(), "src/lib.rs");
        self.log.borrow_mut().push("source".into());
        if self.fail {
            Err(failure("source"))
        } else {
            Ok(vec!["committed line".into()])
        }
    }
    fn validate_position(
        &mut self,
        wanted: &Subject,
        at: &Revision,
        value: &Finding,
    ) -> Result<(), PortError> {
        assert_eq!(wanted, &subject());
        assert_eq!(at, &self.expected_revision);
        assert_eq!(value.location().line().get(), 1);
        self.log.borrow_mut().push("position".into());
        Ok(())
    }
}
struct Renderer {
    identity: Option<RenderProfileIdentity>,
    log: Log,
    fail: bool,
    fail_linked_terminal: bool,
    fail_after_resolution: bool,
    answer_context_uses_round: bool,
    include_transition_title: bool,
    summaries: Vec<SummaryView>,
}
impl ReviewRenderer for Renderer {
    fn identity(&self) -> Option<&RenderProfileIdentity> {
        self.identity.as_ref()
    }
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        Ok(format!("Answer: {body}"))
    }
    fn answer_context(&mut self, input: AnswerRenderInput<'_>) -> Result<String, PortError> {
        if self.answer_context_uses_round {
            Ok(format!(
                "Answer R{} @{}: {}",
                input.context.round,
                input.context.revision.head(),
                input.answer.body
            ))
        } else {
            self.answer(&input.answer.body)
        }
    }
    fn finding(&mut self, value: &Finding) -> Result<String, PortError> {
        self.log.borrow_mut().push("render:finding".into());
        if self.fail {
            Err(failure("render"))
        } else {
            Ok(format!("F{}: {}", value.id().get(), value.body()))
        }
    }
    fn transition(
        &mut self,
        value: &Finding,
        from: FindingStatus,
        explanation: &str,
    ) -> Result<String, PortError> {
        self.log.borrow_mut().push("render:transition".into());
        let body = format!(
            "F{}: {from:?} -> {:?}; {}",
            value.id().get(),
            value.status(),
            explanation
        );
        Ok(if self.include_transition_title {
            format!("{body}; {}", value.title())
        } else {
            body
        })
    }
    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError> {
        self.summaries.push(view.clone());
        let round = &view.round;
        let findings = &view.findings;
        self.log
            .borrow_mut()
            .push(format!("render:summary:{:?}", round.status));
        if self.fail {
            return Err(failure("render"));
        }
        if self.fail_linked_terminal
            && round.status == RoundStatus::Completed
            && !findings.is_empty()
        {
            return Err(failure("linked-summary"));
        }
        if self.fail_after_resolution
            && round.status == RoundStatus::Completed
            && view
                .findings
                .iter()
                .any(|row| row.actions.contains(&PublishedAction::Resolved))
        {
            return Err(failure("resolved-summary"));
        }
        let body = format!(
            "R{} {:?} {:?} roots:{}",
            round.number,
            round.status,
            round.assessment.decision(),
            findings.len()
        );
        Ok(self.identity.as_ref().map_or(body.clone(), |identity| {
            format!("{body} profile:{}", identity.fingerprint)
        }))
    }
}
fn fixtures() -> (Host, Source, Renderer) {
    let log = Rc::new(RefCell::new(vec![]));
    (
        Host::new(log.clone()),
        Source {
            log: log.clone(),
            fail: false,
            expected_revision: revision(),
        },
        Renderer {
            identity: None,
            log,
            fail: false,
            fail_linked_terminal: false,
            fail_after_resolution: false,
            answer_context_uses_round: false,
            include_transition_title: false,
            summaries: vec![],
        },
    )
}
fn effects(host: &Host) -> Vec<Effect> {
    host.writes.iter().map(Write::effect).collect()
}

fn has_plan_error(error: &PlanError, predicate: &impl Fn(&PlanError) -> bool) -> bool {
    predicate(error)
        || match error {
            PlanError::ReplyChecks(errors) | PlanError::ShapeChecks(errors) => {
                errors.iter().any(|error| has_plan_error(error, predicate))
            }
            _ => false,
        }
}

fn replace_findings(request: &mut PublishRequest, findings: Vec<Finding>) {
    let previous = &request.assessment;
    request.assessment = Assessment::new(AssessmentDraft {
        subject: previous.subject().clone(),
        reviewer: previous.reviewer().clone(),
        revision: previous.revision().clone(),
        decision: previous.decision(),
        depth: previous.depth().into(),
        scope: previous.scope().into(),
        limitation: previous.limitation().map(str::to_owned),
        selected_policies: previous.selected_policies().to_vec(),
        evaluations: previous.evaluations().to_vec(),
        findings,
    })
    .unwrap();
}

fn replace_revision(request: &mut PublishRequest, revision: Revision) {
    let previous = &request.assessment;
    request.assessment = Assessment::new(AssessmentDraft {
        subject: previous.subject().clone(),
        reviewer: previous.reviewer().clone(),
        revision,
        decision: previous.decision(),
        depth: previous.depth().into(),
        scope: previous.scope().into(),
        limitation: previous.limitation().map(str::to_owned),
        selected_policies: previous.selected_policies().to_vec(),
        evaluations: previous.evaluations().to_vec(),
        findings: previous.findings().to_vec(),
    })
    .unwrap();
}

fn copy_finding(
    value: &Finding,
    id: FindingId,
    policy_id: PolicyId,
    suggestion: Option<Suggestion>,
) -> Finding {
    Finding::new(FindingDraft {
        id,
        policy_id,
        title: value.title().into(),
        body: value.body().into(),
        severity: value.severity(),
        disposition: value.disposition(),
        status: value.status(),
        evidence: value.evidence().into(),
        validated: value.validated(),
        location: value.location().clone(),
        suggestion,
    })
    .unwrap()
}

#[test]
fn completed_transition_cosmetic_title_edits_reuse_verified_communication() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.include_transition_title = true;
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut req = request("fixed", Some(FindingStatus::Fixed), false);
    let completed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert!(
        host.snapshot
            .replies
            .iter()
            .any(|reply| reply.body.ends_with("; Lost request"))
    );
    let reworded = Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: policy(),
        title: "Request disappears".into(),
        body: "The failure path drops the request.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status: FindingStatus::Fixed,
        evidence: "Regression case".into(),
        validated: true,
        location: finding(FindingStatus::Fixed).location().clone(),
        suggestion: None,
    })
    .unwrap();
    let previous = &req.assessment;
    req.assessment = Assessment::new(AssessmentDraft {
        subject: previous.subject().clone(),
        reviewer: previous.reviewer().clone(),
        revision: previous.revision().clone(),
        decision: previous.decision(),
        depth: previous.depth().to_owned(),
        scope: previous.scope().to_owned(),
        limitation: previous.limitation().map(str::to_owned),
        selected_policies: previous.selected_policies().to_vec(),
        evaluations: previous.evaluations().to_vec(),
        findings: vec![reworded],
    })
    .unwrap();
    host.writes.clear();
    host.log.borrow_mut().clear();
    let repeated = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(repeated.status, PublicationStatus::Unchanged);
    assert_eq!(repeated.round, completed.round);
    assert!(host.writes.is_empty());
    assert!(
        !host
            .log
            .borrow()
            .iter()
            .any(|entry| entry == "render:transition")
    );
}

#[test]
fn initial_review_verifies_pending_before_finding_then_completes() {
    let (mut host, mut source, mut renderer) = fixtures();
    let result = publish(
        &request("initial", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::CreateSummary,
            Effect::Finding,
            Effect::UpdateSummary
        ]
    );
    assert_eq!(result.verified.len(), 3);
    assert!(
        matches!(&host.writes[0], Write::Summary { round, body, .. } if round.status == RoundStatus::Pending && body == "R1 Pending ChangesRequired roots:0")
    );
    assert!(
        matches!(&host.writes[1], Write::Finding { body, .. } if body == "F1: The failure path drops the request.")
    );
    assert!(
        matches!(&host.writes[2], Write::Summary { round, body, .. } if round.status == RoundStatus::Completed && body == "R1 Completed ChangesRequired roots:1")
    );
    let calls = host.log.borrow();
    let writes: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.starts_with("write:"))
        .map(|(index, _)| index)
        .collect();
    for index in writes {
        assert_eq!(calls[index - 1], "authority");
        assert_eq!(calls[index + 1], "discover");
    }
    assert_eq!(host.snapshot.findings[0].author, reviewer("other"));
}

#[test]
fn every_written_round_records_whether_acceptance_enforced_confirmations() {
    for enforced in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        let mut req = request("initial", Some(FindingStatus::Open), false);
        req.confirmed_validation = enforced;
        publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert!(!host.writes.is_empty());
        for write in &host.writes {
            if let Write::Summary { round, .. } = write {
                assert_eq!(round.confirmed_validation, enforced);
            }
        }
    }
}

#[test]
fn repeated_completed_review_writes_nothing_and_keeps_round() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("initial", Some(FindingStatus::Open), false);
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert_eq!(result.round, 1);
    assert!(host.writes.is_empty());
}

#[test]
fn blocked_repeat_has_no_writes_and_completion_reuses_round() {
    let (mut host, mut source, mut renderer) = fixtures();
    let blocked = request("attempt", Some(FindingStatus::Open), true);
    let result = publish(&blocked, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Blocked);
    assert_eq!(effects(&host), vec![Effect::CreateSummary]);
    host.writes.clear();
    assert_eq!(
        publish(&blocked, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Unchanged
    );
    assert!(host.writes.is_empty());
    let result = publish(
        &request("attempt", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, 1);
    assert!(host.snapshot.summaries[0].history.is_empty());
}

#[test]
fn fixed_finding_gets_reply_before_resolution_and_recurrence_gets_reply_before_reopen() {
    let (mut host, mut source, mut renderer) = fixtures();
    publish(
        &request("open", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    host.writes.clear();
    let fixed = publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(fixed.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
    assert!(
        matches!(&host.writes[1], Write::Reply { thread, body, .. } if thread.as_str() == "owned-root" && body == "F1: Open -> Fixed; Regression case")
    );
    assert!(
        host.snapshot
            .findings
            .iter()
            .find(|root| root.id == provider_id("owned-root"))
            .unwrap()
            .resolved
    );
    assert!(
        !host
            .snapshot
            .findings
            .iter()
            .find(|root| root.id == provider_id("foreign-root"))
            .unwrap()
            .resolved
    );
    host.writes.clear();
    let recurrent = publish(
        &request("recurrence", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(recurrent.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Reopen,
            Effect::UpdateSummary
        ]
    );
    assert!(
        matches!(&host.writes[1], Write::Reply { body, .. } if body == "F1: Fixed -> Open; Regression case")
    );
}

#[test]
fn partial_attempt_resumes_same_round_without_duplicate_root() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("recover", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::Rejected(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Partial
    );
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(result.round, 1);
    assert_eq!(
        host.snapshot
            .findings
            .iter()
            .filter(|f| f.author == reviewer("agent"))
            .count(),
        1
    );
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Finding,
            Effect::UpdateSummary
        ]
    );
}

#[test]
fn lost_response_with_committed_finding_is_verified_without_retry() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownCommitted(Effect::Finding));
    let result = publish(
        &request("lost", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::CreateSummary,
            Effect::Finding,
            Effect::UpdateSummary
        ]
    );
    assert_eq!(result.verified.len(), 3);
}

#[test]
fn unknown_absent_finding_stops_pending_and_later_discovery_recovers() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let req = request("uncertain", Some(FindingStatus::Open), false);
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(result.stop, Some(Stop::Unknown(_))));
    assert_eq!(effects(&host), vec![Effect::CreateSummary, Effect::Finding]);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    host.writes.clear();
    observe_pending_create(&mut host);
    let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(recovered.status, PublicationStatus::Completed);
    assert_eq!(recovered.round, 1);
}

#[test]
fn missing_summary_authority_prevents_every_write() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.authority = Effects::new([Effect::Finding]);
    let result = publish(
        &request("denied", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Denied);
    assert!(host.writes.is_empty());
    assert!(matches!(
        result.stop,
        Some(Stop::Authority(Effect::CreateSummary))
    ));
}

#[test]
fn requested_effects_cannot_expand_host_or_provider_authority() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.snapshot.permissions = Effects::new([Effect::CreateSummary, Effect::UpdateSummary]);
    let result = publish(
        &request("limited", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    assert_eq!(
        effects(&host),
        vec![Effect::CreateSummary, Effect::UpdateSummary]
    );
    assert!(matches!(
        result.stop,
        Some(Stop::Authority(Effect::Finding))
    ));
    assert_eq!(
        host.snapshot.summaries[0].current.publication_limitation,
        Some(PublicationLimitation::Authority(Effect::Finding))
    );
}

#[test]
fn comparison_reference_drift_stops_findings_and_marks_round_superseded() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.drift_after_pending = true;
    let result = publish(
        &request("drift", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Superseded);
    assert!(!effects(&host).contains(&Effect::Finding));
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Superseded
    );
}

#[test]
fn source_and_renderer_failures_are_reported_before_writes() {
    for source_failure in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        source.fail = source_failure;
        renderer.fail = !source_failure;
        let result = publish(
            &request("invalid", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer,
        );
        assert!(matches!(result, Err(PlanError::Checks(_))));
        assert!(host.writes.is_empty());
    }
}

#[test]
fn retained_progress_prevents_retries_across_invocations() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.progress.recovery_policy =
        review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative;
    let req = request("uncertain", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    host.writes.clear();
    for _ in 0..2 {
        let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(result.status, PublicationStatus::Uncertain);
        assert!(!effects(&host).contains(&Effect::Finding));
        host.writes.clear();
    }
}

#[test]
fn verified_finding_is_not_skipped_when_readback_reports_revision_drift() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.drift_after_finding = true;
    let result = publish(
        &request("drift", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Superseded);
    assert!(result.skipped.is_empty());
    assert!(
        result
            .verified
            .iter()
            .any(|effect| matches!(effect.write, Write::Finding { .. }))
    );
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Superseded
    );
}

#[test]
fn authenticated_actor_mismatch_rejects_assessment_before_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.authenticated = reviewer("other");
    assert!(matches!(
        publish(
            &request("wrong-actor", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer
        )
        .unwrap_err(),
        PlanError::WrongReviewer
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn read_only_plan_exposes_exact_bodies_without_provider_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    let planned = plan(
        &request("preview", Some(FindingStatus::Open), false),
        &mut Session(&mut host),
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert!(host.writes.is_empty());
    assert_eq!(planned.summary_body, "R1 Pending ChangesRequired roots:0");
    assert_eq!(
        planned.terminal_summary_preview,
        "R1 Completed ChangesRequired roots:0"
    );
    assert_eq!(
        planned.unresolved_findings,
        vec![FindingId::try_from(1).unwrap()]
    );
    assert!(
        matches!(&planned.actions[0], PlannedAction::Finding { body, .. } if body == "F1: The failure path drops the request.")
    );
}

#[test]
fn late_summary_render_failure_preserves_verified_root_and_pending_summary() {
    let (mut host, mut source, mut renderer) = fixtures();
    renderer.fail_linked_terminal = true;
    let result = publish(
        &request("late-render", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    assert!(matches!(result.finalization_stop, Some(Stop::Render(_))));
    assert_eq!(effects(&host), vec![Effect::CreateSummary, Effect::Finding]);
    assert_eq!(result.verified.len(), 2);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

fn seed_completed(host: &mut Host, source: &mut Source, renderer: &mut Renderer) {
    publish(
        &request("initial", Some(FindingStatus::Open), false),
        host,
        source,
        renderer,
    )
    .unwrap();
    host.writes.clear();
    host.discoveries = 0;
    host.authority_reads = 0;
}
fn foreign_question(host: &mut Host) {
    host.snapshot.replies.push(question_record());
}
fn question_record() -> ReplyRecord {
    ReplyRecord {
        id: provider_id("question"),
        author: reviewer("other"),
        thread: provider_id("owned-root"),
        action: None,
        body: "Why does this drop?".into(),
    }
}
fn answer_request(key: &str) -> PublishRequest {
    let mut req = request(key, Some(FindingStatus::Open), false);
    req.answer_targets.push(question_record());
    req.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: provider_id("question"),
        body: "The error branch skips retry.".into(),
    });
    req
}

#[test]
fn an_unperformed_fixed_transition_is_explained_before_a_new_attempt_resolves() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Reply));
    let failed = publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(failed.status, PublicationStatus::Partial);
    assert!(host.snapshot.replies.is_empty());
    host.writes.clear();
    let completed = publish(
        &request("fixed-again", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
    assert!(
        matches!(&host.writes[1], Write::Reply { body, .. } if body == "F1: Open -> Fixed; Regression case")
    );
}

#[test]
fn an_unperformed_fixed_transition_cannot_be_replaced_by_a_different_pending_assessment() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Reply));
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    host.writes.clear();
    let rejected = publish(
        &request("still-open", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    );
    assert!(matches!(rejected, Err(PlanError::ChangedPendingAssessment)));
    assert!(host.writes.is_empty());
    assert!(host.snapshot.replies.is_empty());
}

#[test]
fn a_verified_fixed_transition_is_not_repeated_when_a_new_attempt_repairs_resolution() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let failed = publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(failed.status, PublicationStatus::Partial);
    assert_eq!(host.snapshot.replies.len(), 1);
    host.writes.clear();
    let completed = publish(
        &request("repair-fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
    assert_eq!(host.snapshot.replies.len(), 1);
}

#[test]
fn pending_answers_cannot_be_omitted_or_reworded_on_recovery() {
    for omit in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        foreign_question(&mut host);
        let req = answer_request("answer");
        host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
        let failed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(failed.status, PublicationStatus::Uncertain);
        assert_eq!(
            host.snapshot.summaries[0].current.status,
            RoundStatus::Pending
        );
        let mut changed = req.clone();
        if omit {
            changed.answers.clear();
        } else {
            changed.answers[0].body = "A different explanation.".into();
        }
        host.writes.clear();
        let uncertain = publish(&changed, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(uncertain.status, PublicationStatus::Uncertain);
        assert!(matches!(uncertain.stop, Some(Stop::UnresolvedWrite)));
        observe_pending_create(&mut host);
        assert!(publish(&changed, &mut host, &mut source, &mut renderer).is_err());
        assert!(host.writes.is_empty());
        let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(recovered.status, PublicationStatus::Completed);
        assert_eq!(recovered.round, failed.round);
        assert_eq!(
            host.snapshot
                .replies
                .iter()
                .filter(|r| r.author == reviewer("agent"))
                .count(),
            1
        );
    }
}

#[test]
fn completed_assessments_allocate_new_rounds_for_explicit_reclassification() {
    for change_status in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        let mut changed = request("initial", Some(FindingStatus::Open), false);
        if change_status {
            changed = request("initial", Some(FindingStatus::Fixed), false);
        } else {
            let updated = Finding::new(FindingDraft {
                id: FindingId::try_from(1).unwrap(),
                policy_id: policy(),
                title: "Lost request".into(),
                body: "The failure path drops the request.".into(),
                severity: Severity::P1,
                disposition: Disposition::Blocking,
                status: FindingStatus::Open,
                evidence: "Regression case".into(),
                validated: true,
                location: finding(FindingStatus::Open).location().clone(),
                suggestion: None,
            })
            .unwrap();
            changed.assessment = Assessment::new(AssessmentDraft {
                subject: subject(),
                reviewer: reviewer("agent"),
                revision: revision(),
                decision: ReviewDecision::ChangesRequired,
                depth: "Changed code and callers".into(),
                scope: "Request handling".into(),
                limitation: None,
                selected_policies: vec![policy()],
                evaluations: vec![
                    PolicyEvaluation::new(policy(), PolicyJudgment::Satisfied).unwrap(),
                ],
                findings: vec![updated],
            })
            .unwrap();
        }
        if !change_status {
            changed.transitions.push(TransitionReply {
                finding: FindingId::try_from(1).unwrap(),
                from: FindingStatus::Open,
                to: FindingStatus::Open,
                body: "Higher severity follows from the impact evidence.".into(),
            });
        }
        let result = publish(&changed, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(result.status, PublicationStatus::Completed);
        assert_eq!(result.round, 2);
        assert!(effects(&host).contains(&Effect::Reply));
    }
}

#[test]
fn offline_planning_checks_clean_summary_without_provider_reads() {
    let (host, mut source, mut renderer) = fixtures();
    let req = request("clean", None, false);
    let planned = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert!(planned.actions.is_empty());
    assert_eq!(planned.summary_body, "R1 Pending Ready roots:0");
    assert!(
        !host
            .log
            .borrow()
            .iter()
            .any(|entry| entry == "acquire" || entry == "discover")
    );
    renderer.fail = true;
    assert!(matches!(
        plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer),
        Err(PlanError::Render(_))
    ));
}

#[test]
fn recovering_an_answer_after_cosmetic_rewording_does_not_invent_a_transition() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let mut req = answer_request("initial");
    let reworded = Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: policy(),
        title: "Request disappears".into(),
        body: "The error path loses the request.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Regression case".into(),
        validated: true,
        location: finding(FindingStatus::Open).location().clone(),
        suggestion: None,
    })
    .unwrap();
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: vec![PolicyEvaluation::new(policy(), PolicyJudgment::Satisfied).unwrap()],
        findings: vec![reworded],
    })
    .unwrap();
    host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
    let failed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(failed.status, PublicationStatus::Uncertain);
    assert_eq!(failed.round, 2);
    assert_eq!(
        host.snapshot.summaries[0].history[0].status,
        RoundStatus::Completed
    );
    host.writes.clear();
    observe_pending_create(&mut host);
    let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(recovered.status, PublicationStatus::Completed);
    assert_eq!(recovered.round, 2);
    assert_eq!(effects(&host), vec![Effect::UpdateSummary]);
    assert!(host.snapshot.replies.iter().any(|reply| {
        reply
            .action
            .as_ref()
            .is_some_and(|action| matches!(action.kind, ReplyKind::Answer(_)))
    }));
}

#[test]
fn changed_suggestions_on_carried_open_findings_require_committed_source_validation() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let location = finding(FindingStatus::Open).location().clone();
    let suggestion = Suggestion::new(
        location.clone(),
        revision(),
        LineNumber::try_from(1).unwrap(),
        vec!["not the committed line".into()],
        vec!["replacement".into()],
        "Fix the failing branch.".into(),
    )
    .unwrap();
    let updated = Finding::new(FindingDraft {
        id: FindingId::try_from(1).unwrap(),
        policy_id: policy(),
        title: "Lost request".into(),
        body: "The failure path drops the request.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Regression case".into(),
        validated: true,
        location,
        suggestion: Some(suggestion),
    })
    .unwrap();
    let mut req = request("new-suggestion", Some(FindingStatus::Open), false);
    req.transitions.push(TransitionReply {
        finding: FindingId::try_from(1).unwrap(),
        from: FindingStatus::Open,
        to: FindingStatus::Open,
        body: "The correction now replaces the affected branch.".into(),
    });
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: vec![PolicyEvaluation::new(policy(), PolicyJudgment::Satisfied).unwrap()],
        findings: vec![updated],
    })
    .unwrap();
    assert!(matches!(
        publish(&req, &mut host, &mut source, &mut renderer),
        Err(PlanError::Checks(_))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn unchanged_carried_open_findings_do_not_require_a_new_source_lookup() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    source.fail = true;
    host.log.borrow_mut().clear();
    let result = publish(
        &request("initial", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(host.writes.is_empty());
    assert!(
        !host
            .log
            .borrow()
            .iter()
            .any(|entry| entry == "source" || entry == "position")
    );
}

#[test]
fn pending_answers_reject_a_deleted_or_edited_question_before_recovery_writes() {
    for deleted in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        foreign_question(&mut host);
        let req = answer_request("answer");
        host.fault = Some(Fault::UnknownAbsent(Effect::Reply));
        let failed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(failed.status, PublicationStatus::Uncertain);
        if deleted {
            host.snapshot
                .replies
                .retain(|reply| reply.id != provider_id("question"));
        } else {
            host.snapshot
                .replies
                .iter_mut()
                .find(|reply| reply.id == provider_id("question"))
                .unwrap()
                .body = "A materially different question.".into();
        }
        host.writes.clear();
        let uncertain = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(uncertain.status, PublicationStatus::Uncertain);
        assert!(matches!(uncertain.stop, Some(Stop::UnresolvedWrite)));
        observe_pending_create(&mut host);
        assert_eq!(
            publish(&req, &mut host, &mut source, &mut renderer)
                .unwrap()
                .status,
            PublicationStatus::Uncertain
        );
        assert!(host.progress.unresolved.is_some());
        assert!(host.writes.is_empty());
        assert_eq!(
            host.snapshot.summaries[0].current.status,
            RoundStatus::Pending
        );
    }
}

#[test]
fn a_pending_round_with_a_missing_frozen_create_cannot_claim_completion() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("pending", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    host.snapshot.summaries[0].current.actions.clear();
    host.writes.clear();
    let uncertain = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(uncertain.status, PublicationStatus::Uncertain);
    assert!(matches!(uncertain.stop, Some(Stop::UnresolvedWrite)));
    assert!(plan(&req, &mut Session(&mut host), &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

#[test]
fn a_completed_round_with_a_deleted_transition_reply_is_a_conflict() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Completed
    );
    host.snapshot.replies.retain(|reply| {
        !matches!(
            reply.action.as_ref(),
            Some(ActionId {
                kind: ReplyKind::Transition(_),
                ..
            })
        )
    });
    host.writes.clear();
    assert!(publish(&req, &mut host, &mut source, &mut renderer).is_err());
    assert!(host.writes.is_empty());
}

#[test]
fn verified_root_note_references_remain_distinct_from_thread_write_targets() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let result = publish(
        &answer_request("answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    let verified_reply = result
        .verified
        .iter()
        .find(|effect| matches!(effect.write, Write::Reply { .. }))
        .unwrap();
    let root = verified_reply
        .readback
        .findings
        .iter()
        .find(|record| record.author == reviewer("agent"))
        .unwrap();
    assert_eq!(root.id, provider_id("owned-root"));
    assert_eq!(root.root_note, provider_id("owned-root-note"));
    assert_ne!(root.id, root.root_note);
    assert!(matches!(&verified_reply.write, Write::Reply { thread, .. } if thread == &root.id));
}

#[test]
fn foreign_answer_target_edited_after_plan_stops_before_writes() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    host.edit_reply_on_second_read = true;
    let result = publish(
        &answer_request("answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert!(matches!(result.stop, Some(Stop::ConcurrentChange)));
    assert!(host.writes.is_empty());
}

#[test]
fn duplicate_owned_summaries_and_roots_reject_before_writes() {
    for duplicate_summary in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        if duplicate_summary {
            let mut copy = host.snapshot.summaries[0].clone();
            copy.id = provider_id("duplicate-summary");
            host.snapshot.summaries.push(copy);
        } else {
            let mut copy = host
                .snapshot
                .findings
                .iter()
                .find(|f| f.author == reviewer("agent"))
                .unwrap()
                .clone();
            copy.id = provider_id("duplicate-root");
            host.snapshot.findings.push(copy);
        }
        assert!(
            publish(
                &request("initial", Some(FindingStatus::Open), false),
                &mut host,
                &mut source,
                &mut renderer
            )
            .is_err()
        );
        assert!(host.writes.is_empty());
    }
}

#[test]
fn changing_pagination_order_preserves_owned_state_comparison() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut root = host
        .snapshot
        .findings
        .iter()
        .find(|f| f.author == reviewer("agent"))
        .unwrap()
        .clone();
    let second = Finding::new(FindingDraft {
        id: FindingId::try_from(2).unwrap(),
        policy_id: policy(),
        title: "Second defect".into(),
        body: "Second path fails.".into(),
        severity: Severity::P3,
        disposition: Disposition::Advisory,
        status: FindingStatus::Open,
        evidence: "Second case".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
            LineNumber::try_from(1).unwrap(),
        ),
        suggestion: None,
    })
    .unwrap();
    root.id = provider_id("second-root");
    root.root_note = provider_id("second-root-note");
    root.finding = second.clone();
    host.snapshot.findings.push(root);
    let base = request("order", Some(FindingStatus::Fixed), false);
    let mut req = base.clone();
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: base.assessment.evaluations().to_vec(),
        findings: vec![finding(FindingStatus::Fixed), second],
    })
    .unwrap();
    host.reverse_pagination = true;
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
}

#[test]
fn missing_historical_root_is_conflict_instead_of_recreating_it() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot
        .findings
        .retain(|f| f.author != reviewer("agent"));
    assert!(
        publish(
            &request("initial", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer
        )
        .is_err()
    );
    assert!(host.writes.is_empty());
}

#[test]
fn same_answer_is_not_duplicated_across_review_rounds() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let first = publish(
        &answer_request("first-answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(first.status, PublicationStatus::Completed);
    assert!(host.writes.iter().any(|w| matches!(w, Write::Reply { body, .. } if body == "Answer: The error branch skips retry.")));
    host.writes.clear();
    let second = publish(
        &answer_request("later-round"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(second.status, PublicationStatus::Unchanged);
    assert_eq!(second.round, first.round);
    assert!(host.writes.is_empty());
    assert!(!effects(&host).contains(&Effect::Reply));
}

#[test]
fn authority_narrowed_midrun_is_not_regained_by_later_read() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.narrow_on_third_authority = true;
    let result = publish(
        &request("narrowed", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    assert!(matches!(
        result.stop,
        Some(Stop::Authority(Effect::Finding))
    ));
    assert_eq!(
        effects(&host),
        vec![Effect::CreateSummary, Effect::UpdateSummary]
    );
    assert!(host.authority_reads > 3);
}

#[test]
fn recovery_requires_exact_root_body_round_and_revision() {
    for mismatch in ["body", "round", "revision"] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        host.snapshot.summaries[0].current.status = RoundStatus::Pending;
        let root = host
            .snapshot
            .findings
            .iter_mut()
            .find(|f| f.author == reviewer("agent"))
            .unwrap();
        match mismatch {
            "body" => root.body = "Different rendered body".into(),
            "round" => root.created_in = 2,
            _ => {
                root.revision =
                    Revision::new("different-head".into(), vec!["base".into(), "start".into()])
                        .unwrap()
            }
        }
        assert!(
            publish(
                &request("initial", Some(FindingStatus::Open), false),
                &mut host,
                &mut source,
                &mut renderer
            )
            .is_err(),
            "accepted mismatched {mismatch}"
        );
        assert!(host.writes.is_empty());
    }
}

fn numbered_finding(number: u64) -> Finding {
    Finding::new(FindingDraft {
        id: FindingId::try_from(number).unwrap(),
        policy_id: policy(),
        title: format!("Defect {number}"),
        body: format!("Failure {number}"),
        severity: Severity::P3,
        disposition: Disposition::Advisory,
        status: FindingStatus::Open,
        evidence: format!("Case {number}"),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned()).unwrap(),
            LineNumber::try_from(1).unwrap(),
        ),
        suggestion: None,
    })
    .unwrap()
}

#[test]
fn a_concurrent_summary_edit_is_not_overwritten_by_partial_finalization() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.edit_summary_on_discovery = Some(4);
    let result = publish(
        &request("initial", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(result.status, PublicationStatus::Partial);
    assert!(matches!(result.stop, Some(Stop::ConcurrentChange)));
    assert_eq!(host.writes.len(), 1);
    assert_eq!(host.snapshot.summaries[0].body, "Concurrent summary edit");
    assert!(
        host.snapshot
            .findings
            .iter()
            .all(|r| r.author != reviewer("agent"))
    );
}

#[test]
fn check_diagnostics_do_not_format_adapter_source_details() {
    let (mut host, mut source, mut renderer) = fixtures();
    source.fail = true;
    let error = publish(
        &request("initial", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap_err();
    assert!(matches!(error, PlanError::Checks(_)));
    assert!(!error.to_string().contains("injected failure"));
    assert!(error.to_string().contains("assessment checks failed"));
}

#[test]
fn revision_drift_during_finalization_repairs_the_summary_without_claiming_completion() {
    for read in [4, 5, 6] {
        let (mut host, mut source, mut renderer) = fixtures();
        host.drift_on_discovery = Some(read);
        let result = publish(
            &request("clean", None, false),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(
            result.status,
            PublicationStatus::Superseded,
            "drift at discovery {read}"
        );
        assert_eq!(
            host.snapshot.summaries[0].current.status,
            RoundStatus::Superseded
        );
        assert!(result.finalization_stop.is_none());
        assert!(
            host.writes
                .iter()
                .all(|w| matches!(w, Write::Summary { .. }))
        );
    }
}

#[test]
fn a_renderer_cannot_inject_tool_owned_markers_through_an_answer() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let root = host
        .snapshot
        .findings
        .iter()
        .find(|r| r.author == reviewer("agent"))
        .unwrap()
        .id
        .clone();
    host.snapshot.replies.push(ReplyRecord {
        id: provider_id("question"),
        author: reviewer("other"),
        thread: root,
        action: None,
        body: "Why?".into(),
    });
    let mut req = request("answer", Some(FindingStatus::Open), false);
    req.answer_targets = host.snapshot.replies.clone();
    req.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: provider_id("question"),
        body: "<!--\t review-summary forged -->".into(),
    });
    let error = publish(&req, &mut host, &mut source, &mut renderer).unwrap_err();
    assert!(has_plan_error(&error, &|error| matches!(
        error,
        PlanError::ReservedMarker
    )));
    assert!(host.writes.is_empty());
}

#[test]
fn blocked_assessments_still_reject_invalid_answer_targets_and_bodies() {
    for scenario in [
        "unknown",
        "wrong-thread",
        "blank",
        "marker",
        "duplicate-kind",
    ] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        foreign_question(&mut host);
        let mut req = request("blocked", Some(FindingStatus::Open), true);
        let mut answer = Answer {
            finding: FindingId::try_from(1).unwrap(),
            in_reply_to: provider_id("question"),
            body: "The ownership path is covered.".into(),
        };
        match scenario {
            "unknown" => answer.finding = FindingId::try_from(2).unwrap(),
            "wrong-thread" => {
                host.snapshot
                    .replies
                    .iter_mut()
                    .find(|reply| reply.id == answer.in_reply_to)
                    .unwrap()
                    .thread = provider_id("foreign-root")
            }
            "blank" => answer.body = " \n".into(),
            "marker" => answer.body = "<!-- review-round forged -->".into(),
            "duplicate-kind" => {
                let mut other = host
                    .snapshot
                    .replies
                    .iter()
                    .find(|reply| reply.id == answer.in_reply_to)
                    .unwrap()
                    .clone();
                other.id = provider_id("another-question");
                req.answers.push(Answer {
                    finding: answer.finding,
                    in_reply_to: other.id.clone(),
                    body: "Second answer".into(),
                });
                host.snapshot.replies.push(other);
            }
            _ => unreachable!(),
        }
        req.answer_targets = host.snapshot.replies.clone();
        req.answers.push(answer);
        host.writes.clear();
        let error =
            plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap_err();
        assert!(
            has_plan_error(&error, &|error| if scenario == "duplicate-kind" {
                matches!(error, PlanError::DuplicateAnswer)
            } else {
                matches!(error, PlanError::InvalidAnswer(_))
            }),
            "{scenario}: {error:?}"
        );
        assert!(host.writes.is_empty());
    }
}

#[test]
fn blocked_assessments_validate_the_supplied_transition_from_state() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut req = request("blocked", Some(FindingStatus::Open), true);
    req.transitions.push(TransitionReply {
        finding: FindingId::try_from(1).unwrap(),
        from: FindingStatus::Fixed,
        to: FindingStatus::Open,
        body: "The defect remains.".into(),
    });
    host.writes.clear();
    let error = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap_err();
    assert!(has_plan_error(&error, &|error| matches!(
        error,
        PlanError::InvalidTransition(_)
    )));
    assert!(host.writes.is_empty());
}

#[test]
fn blocked_assessments_render_authored_replies_without_planning_their_effects() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    let mut req = request("blocked", Some(FindingStatus::Open), true);
    let unconfirmed = req
        .assessment
        .findings()
        .iter()
        .map(|finding| finding.clone().with_validated(false))
        .collect();
    replace_findings(&mut req, unconfirmed);
    req.answer_targets = host.snapshot.replies.clone();
    req.transitions.push(TransitionReply {
        finding: FindingId::try_from(1).unwrap(),
        from: FindingStatus::Open,
        to: FindingStatus::Open,
        body: "The defect remains while CI is unavailable.".into(),
    });
    req.answers.push(Answer {
        finding: FindingId::try_from(1).unwrap(),
        in_reply_to: provider_id("question"),
        body: "The unavailable integration run blocks the review.".into(),
    });
    host.log.borrow_mut().clear();
    let planned = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap();
    assert!(planned.actions.is_empty());
    assert!(
        host.log
            .borrow()
            .iter()
            .any(|call| call == "render:transition")
    );
    assert_eq!(planned.round.answer_targets.len(), 1);
}

#[test]
fn offline_checks_reject_a_different_trusted_reviewer_before_source_or_rendering() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.snapshot.reviewer = reviewer("different-authenticated-user");
    let req = request("first", None, false);
    let error = plan_from_snapshot(&req, &host.snapshot, &mut source, &mut renderer).unwrap_err();
    assert!(matches!(
        error,
        PlanError::Discovery(
            review_kit_application::use_cases::discovery::DiscoveryError::Conflict(
                review_kit_application::use_cases::discovery::DiscoveryConflict::WrongReviewer
            )
        )
    ));
    assert!(host.log.borrow().is_empty());
}

#[test]
fn offline_checks_collect_shape_source_and_summary_failures_in_one_pass() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut req = request("blocked", Some(FindingStatus::Open), true);
    let new = copy_finding(
        &finding(FindingStatus::Open),
        FindingId::try_from(2).unwrap(),
        policy(),
        None,
    );
    replace_findings(&mut req, vec![new]);
    req.answers.push(Answer {
        finding: FindingId::try_from(2).unwrap(),
        in_reply_to: provider_id("missing-question"),
        body: "Explanation".into(),
    });
    source.fail = true;
    renderer.fail = true;
    let errors = review_kit_application::use_cases::publishing::check_review(
        &req,
        &host.snapshot,
        &mut source,
        &mut renderer,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, PlanError::MissingCarriedFinding(_)))
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, PlanError::ColocatedFinding(_)))
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, PlanError::InvalidAnswer(_)))
    );
    assert!(errors.iter().any(|error| matches!(error, PlanError::Checks(issues) if issues.iter().any(|issue| matches!(issue.problem, review_kit_application::use_cases::assessment::CheckProblem::Source(_))))));
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, PlanError::Render(_)))
    );
    assert_eq!(
        renderer.summaries.last().unwrap().round.status,
        RoundStatus::Blocked
    );
}

#[test]
fn a_fresh_fixed_reassessment_omits_old_replies_and_returns_unchanged() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let mut fresh = request("fresh", Some(FindingStatus::Fixed), false);
    fresh.transitions.clear();
    source.fail = true;
    renderer.fail = true;
    host.writes.clear();
    let result = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert_eq!(result.round, 2);
    assert!(host.writes.is_empty());
}

#[test]
fn completed_answers_survive_later_question_edits_or_deletion() {
    for delete in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        foreign_question(&mut host);
        let completed = publish(
            &answer_request("answer"),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(completed.status, PublicationStatus::Completed);
        if delete {
            host.snapshot
                .replies
                .retain(|reply| reply.id != provider_id("question"));
        } else {
            host.snapshot
                .replies
                .iter_mut()
                .find(|reply| reply.id == provider_id("question"))
                .unwrap()
                .body = "Updated question".into();
        }
        host.writes.clear();
        let result = publish(
            &request("fresh", Some(FindingStatus::Open), false),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(result.status, PublicationStatus::Unchanged);
        assert_eq!(result.round, completed.round);
        assert!(host.writes.is_empty());
        assert!(host.snapshot.replies.iter().any(|reply| matches!(
            reply.action.as_ref().map(|action| &action.kind),
            Some(ReplyKind::Answer(_))
        )));
    }
}

#[test]
fn unfinished_resolution_recovers_on_a_new_revision_without_repeating_the_transition() {
    for resolving in [true, false] {
        for allowed in [true, false] {
            let (mut host, mut source, mut renderer) = fixtures();
            seed_completed(&mut host, &mut source, &mut renderer);
            if !resolving {
                publish(
                    &request("fixed", Some(FindingStatus::Fixed), false),
                    &mut host,
                    &mut source,
                    &mut renderer,
                )
                .unwrap();
            }
            let effect = if resolving {
                Effect::Resolve
            } else {
                Effect::Reopen
            };
            let status = if resolving {
                FindingStatus::Fixed
            } else {
                FindingStatus::Open
            };
            host.fault = Some(Fault::Rejected(effect));
            let failed = publish(
                &request(
                    if resolving { "fixed" } else { "recurrence" },
                    Some(status),
                    false,
                ),
                &mut host,
                &mut source,
                &mut renderer,
            )
            .unwrap();
            assert_eq!(failed.status, PublicationStatus::Partial);
            assert_eq!(
                host.snapshot
                    .findings
                    .iter()
                    .find(|root| root.author == reviewer("agent"))
                    .unwrap()
                    .resolved,
                !resolving
            );
            let replies = host.snapshot.replies.clone();
            host.snapshot.revision =
                Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap();
            let mut fresh = request("fresh", Some(status), false);
            fresh.transitions.clear();
            replace_revision(&mut fresh, host.snapshot.revision.clone());
            if !allowed {
                fresh.effects = Effects::new([Effect::UpdateSummary]);
            }
            host.writes.clear();
            let result = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
            assert_eq!(result.round, failed.round + 1);
            assert_eq!(host.snapshot.replies, replies);
            if allowed {
                assert_eq!(result.status, PublicationStatus::Completed);
                assert_eq!(
                    host.snapshot
                        .findings
                        .iter()
                        .find(|root| root.author == reviewer("agent"))
                        .unwrap()
                        .resolved,
                    resolving
                );
                assert_eq!(
                    effects(&host),
                    vec![Effect::UpdateSummary, effect, Effect::UpdateSummary]
                );
            } else {
                assert_eq!(result.status, PublicationStatus::Partial);
                assert!(matches!(result.stop, Some(Stop::Authority(denied)) if denied == effect));
                assert_eq!(
                    host.snapshot
                        .findings
                        .iter()
                        .find(|root| root.author == reviewer("agent"))
                        .unwrap()
                        .resolved,
                    !resolving
                );
                assert_eq!(
                    effects(&host),
                    vec![Effect::UpdateSummary, Effect::UpdateSummary]
                );
            }
        }
    }
}

#[test]
fn cross_revision_recovery_rejects_resolution_intent_that_conflicts_with_the_judgment() {
    for resolving in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        if !resolving {
            publish(
                &request("fixed", Some(FindingStatus::Fixed), false),
                &mut host,
                &mut source,
                &mut renderer,
            )
            .unwrap();
        }
        let effect = if resolving {
            Effect::Resolve
        } else {
            Effect::Reopen
        };
        let status = if resolving {
            FindingStatus::Fixed
        } else {
            FindingStatus::Open
        };
        host.fault = Some(Fault::Rejected(effect));
        let failed = publish(
            &request(
                if resolving { "fixed" } else { "recurrence" },
                Some(status),
                false,
            ),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(failed.status, PublicationStatus::Partial);
        let action = host.snapshot.summaries[0]
            .current
            .actions
            .iter_mut()
            .find(|action| matches!(action, PlannedAction::SetResolved { .. }))
            .unwrap();
        if let PlannedAction::SetResolved { resolved, .. } = action {
            *resolved = !resolving;
        }
        host.snapshot.revision =
            Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap();
        let mut fresh = request("fresh", Some(status), false);
        fresh.transitions.clear();
        replace_revision(&mut fresh, host.snapshot.revision.clone());
        host.writes.clear();
        let outcome = publish(&fresh, &mut host, &mut source, &mut renderer);
        assert!(matches!(outcome, Err(ref error) if has_plan_error(error,
            &|error| matches!(error, PlanError::InconsistentRound))));
        assert!(host.writes.is_empty());
        assert_eq!(
            host.snapshot
                .findings
                .iter()
                .find(|root| root.author == reviewer("agent"))
                .unwrap()
                .resolved,
            !resolving
        );
    }
}

#[test]
fn completed_resolution_preserves_a_later_manual_reopen() {
    for new_revision in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        publish(
            &request("fixed", Some(FindingStatus::Fixed), false),
            &mut host,
            &mut source,
            &mut renderer,
        )
        .unwrap();
        host.snapshot
            .findings
            .iter_mut()
            .find(|root| root.author == reviewer("agent"))
            .unwrap()
            .resolved = false;
        let mut fresh = request("fresh", Some(FindingStatus::Fixed), false);
        fresh.transitions.clear();
        if new_revision {
            host.snapshot.revision =
                Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap();
            replace_revision(&mut fresh, host.snapshot.revision.clone());
        }
        host.writes.clear();
        let result = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(
            result.status,
            if new_revision {
                PublicationStatus::Completed
            } else {
                PublicationStatus::Unchanged
            }
        );
        assert!(
            !host
                .snapshot
                .findings
                .iter()
                .find(|root| root.author == reviewer("agent"))
                .unwrap()
                .resolved
        );
        assert!(!effects(&host).contains(&Effect::Resolve));
        assert!(!effects(&host).contains(&Effect::Reply));
        if !new_revision {
            assert!(host.writes.is_empty());
        }
    }
}

#[test]
fn a_new_judgment_does_not_replay_unfinished_resolution() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let failed = publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(failed.status, PublicationStatus::Partial);
    host.snapshot.revision =
        Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap();
    let mut fresh = request("recurrence", Some(FindingStatus::Open), false);
    replace_revision(&mut fresh, host.snapshot.revision.clone());
    host.writes.clear();
    let result = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(
        !host
            .snapshot
            .findings
            .iter()
            .find(|root| root.author == reviewer("agent"))
            .unwrap()
            .resolved
    );
    assert_eq!(
        effects(&host),
        vec![Effect::UpdateSummary, Effect::Reply, Effect::UpdateSummary]
    );
}

#[test]
fn a_deleted_completed_answer_remains_a_conflict() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    foreign_question(&mut host);
    publish(
        &answer_request("answer"),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    host.snapshot.replies.retain(|reply| reply.action.is_none());
    host.writes.clear();
    let outcome = publish(
        &request("fresh", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    );
    assert!(
        matches!(outcome, Err(ref error) if has_plan_error(error, &|error| matches!(error, PlanError::ConflictingReply(_))))
    );
    assert!(host.writes.is_empty());
}

#[test]
fn a_deleted_carried_root_in_a_superseded_round_stops_a_new_review() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.snapshot.summaries.clear();
    let mut req = request("followup", Some(FindingStatus::Open), false);
    replace_findings(
        &mut req,
        vec![finding(FindingStatus::Open), numbered_finding(2)],
    );
    req.allow_colocated = true;
    host.drift_after_pending = true;
    let superseded = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(superseded.status, PublicationStatus::Superseded);
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Superseded
    );
    assert!(!host.snapshot.summaries[0].current.actions.iter().any(|action| matches!(
        action, PlannedAction::Finding { finding, .. } if finding.id() == FindingId::try_from(1).unwrap()
    )));
    host.snapshot
        .findings
        .retain(|root| root.author != reviewer("agent"));
    let mut fresh = request("clean", None, false);
    replace_revision(&mut fresh, host.snapshot.revision.clone());
    host.writes.clear();
    let outcome = publish(&fresh, &mut host, &mut source, &mut renderer);
    assert!(
        matches!(outcome, Err(ref error) if has_plan_error(error, &|error| matches!(error,
        PlanError::MissingPublishedFinding(id) if *id == FindingId::try_from(1).unwrap())))
    );
    assert!(host.writes.is_empty());
}

#[test]
fn historical_closed_findings_do_not_require_their_policy_in_current_selection() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let mut fresh = request("new-scope", Some(FindingStatus::Fixed), false);
    fresh.transitions.clear();
    fresh.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::Ready,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![],
        evaluations: vec![],
        findings: vec![finding(FindingStatus::Fixed)],
    })
    .unwrap();
    fresh.policies = AssessmentPolicies::new("empty".into(), vec![]).unwrap();
    fresh.policy_declarations[0].status =
        review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Excluded;
    fresh.policy_declarations[0].reason = "The policy is excluded from this review scope.".into();
    source.fail = true;
    host.writes.clear();
    let result = publish(&fresh, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(
        effects(&host)
            .iter()
            .all(|effect| *effect == Effect::UpdateSummary)
    );
    let renamed = copy_finding(
        &finding(FindingStatus::Fixed),
        FindingId::try_from(1).unwrap(),
        PolicyId::try_from("unselected-other-policy".to_owned()).unwrap(),
        None,
    );
    replace_findings(&mut fresh, vec![renamed]);
    let errors = review_kit_application::use_cases::publishing::check_review(
        &fresh,
        &host.snapshot,
        &mut source,
        &mut renderer,
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, PlanError::UnselectedHistoricalPolicy(_)))
    );
}

#[test]
fn changing_only_suggestion_evidence_does_not_republish_or_read_source() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = request("initial", Some(FindingStatus::Open), false);
    let base = finding(FindingStatus::Open);
    let make_suggestion = |evidence: &str| {
        Suggestion::new(
            base.location().clone(),
            revision(),
            LineNumber::try_from(1).unwrap(),
            vec!["committed line".into()],
            vec!["replacement".into()],
            evidence.into(),
        )
        .unwrap()
    };
    replace_findings(
        &mut req,
        vec![copy_finding(
            &base,
            base.id(),
            policy(),
            Some(make_suggestion("Initial correction evidence")),
        )],
    );
    publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    replace_findings(
        &mut req,
        vec![copy_finding(
            &base,
            base.id(),
            policy(),
            Some(make_suggestion(
                "The same correction described more clearly",
            )),
        )],
    );
    source.fail = true;
    renderer.fail = true;
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(host.writes.is_empty());
}

#[test]
fn finding_effects_use_numeric_order_even_when_assessment_has_f10_before_f2() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = request("numeric", None, false);
    req.allow_colocated = true;
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: req.assessment.evaluations().to_vec(),
        findings: vec![numbered_finding(10), numbered_finding(2)],
    })
    .unwrap();
    let planned = plan(&req, &mut Session(&mut host), &mut source, &mut renderer).unwrap();
    let numbers: Vec<_> = planned
        .actions
        .iter()
        .filter_map(|action| match action {
            PlannedAction::Finding { finding, .. } => Some(finding.id().get()),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, vec![2, 10]);
    assert_eq!(
        renderer.summaries[0]
            .unpublished_findings
            .iter()
            .map(|id| id.get())
            .collect::<Vec<_>>(),
        vec![2, 10]
    );
    assert!(host.writes.is_empty());
}

#[test]
fn cumulative_policy_rows_count_each_owned_root_once_across_followups() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    publish(
        &request("fixed", Some(FindingStatus::Fixed), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let second = renderer.summaries.last().unwrap();
    assert_eq!(second.policies.len(), 1);
    assert_eq!(second.policies[0].policy_id, policy());
    assert_eq!(second.policies[0].applicable_rounds, vec![1, 2]);
    assert_eq!(second.policies[0].comments, 1);
    publish(
        &request("recurrence", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    let third = renderer.summaries.last().unwrap();
    assert_eq!(third.policies[0].applicable_rounds, vec![1, 2, 3]);
    assert_eq!(third.policies[0].comments, 1);
    assert_eq!(third.counts.total, 1);
}

#[test]
fn an_identical_pending_barrier_still_requires_summary_update_authority() {
    let (mut host, mut source, mut renderer) = fixtures();
    let req = request("pending", Some(FindingStatus::Open), false);
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    assert_eq!(
        publish(&req, &mut host, &mut source, &mut renderer)
            .unwrap()
            .status,
        PublicationStatus::Uncertain
    );
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
    host.writes.clear();
    observe_pending_create(&mut host);
    host.authority = Effects::new([
        Effect::Finding,
        Effect::Reply,
        Effect::Resolve,
        Effect::Reopen,
    ]);
    let denied = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(denied.status, PublicationStatus::Denied);
    assert!(matches!(
        denied.stop,
        Some(Stop::Authority(Effect::UpdateSummary))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn allocation_uses_owned_root_and_reply_rounds_beyond_summary() {
    for keep_summary in [true, false] {
        let (mut host, mut source, mut renderer) = fixtures();
        seed_completed(&mut host, &mut source, &mut renderer);
        if !keep_summary {
            host.snapshot.summaries.clear();
            host.snapshot
                .findings
                .iter_mut()
                .find(|f| f.author == reviewer("agent"))
                .unwrap()
                .created_in = 4;
        }
        host.snapshot.replies.push(ReplyRecord {
            id: provider_id("later-owned-reply"),
            author: reviewer("agent"),
            thread: provider_id("owned-root"),
            action: Some(ActionId {
                round: 8,
                finding: FindingId::try_from(1).unwrap(),
                kind: ReplyKind::Transition(FindingStatus::Open),
            }),
            body: "Earlier published communication".into(),
        });
        let planned = plan(
            &request("next", Some(FindingStatus::Fixed), false),
            &mut Session(&mut host),
            &mut source,
            &mut renderer,
        )
        .unwrap();
        assert_eq!(planned.round.number, 9);
        assert!(host.writes.is_empty());
    }
}

#[test]
fn surviving_owned_reply_reserves_its_finding_id_without_root_or_summary() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.snapshot.replies.push(ReplyRecord {
        id: provider_id("surviving-action"),
        author: reviewer("agent"),
        thread: provider_id("missing-root"),
        action: Some(ActionId {
            round: 7,
            finding: FindingId::try_from(10).unwrap(),
            kind: ReplyKind::Transition(FindingStatus::Fixed),
        }),
        body: "Verified fixed".into(),
    });
    let mut req = request("next", None, false);
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
        revision: revision(),
        decision: ReviewDecision::ChangesRequired,
        depth: "Changed code and callers".into(),
        scope: "Request handling".into(),
        limitation: None,
        selected_policies: vec![policy()],
        evaluations: req.assessment.evaluations().to_vec(),
        findings: vec![numbered_finding(2)],
    })
    .unwrap();
    assert!(
        matches!(plan(&req, &mut Session(&mut host), &mut source, &mut renderer), Err(PlanError::ReusedFindingId(id)) if id.get() == 2)
    );
    assert!(host.writes.is_empty());
}

#[test]
fn conservative_unknown_absent_create_is_sent_once() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.progress.recovery_policy =
        review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative;
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    let req = request("uncertain", Some(FindingStatus::Open), false);
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert_eq!(effects(&host), vec![Effect::CreateSummary, Effect::Finding]);
    assert!(host.progress.unresolved.is_some());
    host.writes.clear();
    let rerun = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(rerun.status, PublicationStatus::Uncertain);
    assert!(matches!(rerun.stop, Some(Stop::UnresolvedWrite)));
    assert!(host.writes.is_empty());
}

#[test]
fn conservative_absent_initial_summary_waits_for_observation() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.progress.recovery_policy =
        review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative;
    host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
    let req = request("initial", Some(FindingStatus::Open), false);
    let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(first.status, PublicationStatus::Uncertain);
    assert!(host.snapshot.summaries.is_empty());
    assert_eq!(effects(&host), vec![Effect::CreateSummary]);
    let retained = host.progress.clone();
    host.writes.clear();
    for _ in 0..2 {
        let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(result.status, PublicationStatus::Uncertain);
        assert!(matches!(result.stop, Some(Stop::UnresolvedWrite)));
        assert!(host.writes.is_empty());
        assert_eq!(host.progress, retained);
    }
    observe_pending_create(&mut host);
    let recovered = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(recovered.status, PublicationStatus::Completed);
    assert_eq!(effects(&host), vec![Effect::Finding, Effect::UpdateSummary]);
    assert!(host.progress.unresolved.is_none());
}

#[test]
fn conservative_unresolved_summary_stops_before_source_and_render_checks() {
    for failure in ["revision", "source", "render"] {
        let (mut host, mut source, mut renderer) = fixtures();
        host.progress.recovery_policy =
            review_kit_application::use_cases::publishing::RecoveryPolicy::Conservative;
        host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
        let req = request("initial", Some(FindingStatus::Open), false);
        let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        let retained = host.progress.clone();
        match failure {
            "revision" => {
                host.snapshot.revision =
                    Revision::new("new-head".into(), vec!["base".into(), "start".into()]).unwrap()
            }
            "source" => source.fail = true,
            _ => renderer.fail = true,
        }
        host.writes.clear();
        host.log.borrow_mut().clear();
        renderer.summaries.clear();
        let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        assert_eq!(result.status, PublicationStatus::Uncertain);
        assert!(matches!(result.stop, Some(Stop::UnresolvedWrite)));
        assert_eq!(result.round, first.round);
        assert_eq!(host.progress, retained);
        assert!(result.planned.is_empty());
        assert!(result.attempted.is_empty());
        assert!(result.verified.is_empty());
        assert!(result.skipped.is_empty());
        assert!(host.writes.is_empty());
        assert!(renderer.summaries.is_empty());
        assert!(
            !host
                .log
                .borrow()
                .iter()
                .any(|call| call == "source" || call == "position" || call.starts_with("render:"))
        );
    }
}

#[test]
fn matching_unknown_summary_update_settles_and_reruns_unchanged() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownCommitted(Effect::UpdateSummary));
    let req = request("initial", Some(FindingStatus::Open), false);
    let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(first.status, PublicationStatus::Completed);
    assert!(first.finalization_stop.is_none());
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Completed
    );
    assert!(host.progress.unresolved.is_none());
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(result.stop.is_none());
    assert!(host.writes.is_empty());
}

#[test]
fn matching_unknown_resolution_finalizes_with_observed_provenance() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    host.writes.clear();
    host.fault = Some(Fault::UnknownCommitted(Effect::Resolve));
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    let first = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(first.status, PublicationStatus::Completed);
    assert!(first.stop.is_none());
    assert_eq!(
        effects(&host),
        vec![
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
    assert!(host.progress.unresolved.is_none());
    assert_eq!(
        host.progress
            .verified_resolutions
            .last()
            .unwrap()
            .provenance,
        ResolutionProvenance::Observed
    );
    host.writes.clear();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Unchanged);
    assert!(result.stop.is_none());
    assert_eq!(result.round, first.round);
    assert!(host.writes.is_empty());
}

#[test]
fn progress_rejects_a_different_subject_or_reviewer() {
    for change_subject in [false, true] {
        let (mut host, mut source, mut renderer) = fixtures();
        host.fault = Some(Fault::UnknownAbsent(Effect::CreateSummary));
        let req = request("initial", Some(FindingStatus::Open), false);
        publish(&req, &mut host, &mut source, &mut renderer).unwrap();
        let pending = host.progress.unresolved.as_mut().unwrap();
        if change_subject {
            pending.subject =
                Subject::new("gitlab".into(), "host".into(), "group/repo".into(), 8).unwrap();
        } else {
            pending.reviewer = reviewer("other");
        }
        host.writes.clear();
        let result = publish(&req, &mut host, &mut source, &mut renderer);
        assert!(matches!(result, Err(PlanError::ProgressBinding)));
        assert!(host.writes.is_empty());
        assert!(host.progress.unresolved.is_some());
    }
}

#[test]
fn unknown_missing_create_with_revision_drift_keeps_its_progress() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.fault = Some(Fault::UnknownAbsent(Effect::Finding));
    host.drift_after_unknown = true;
    let req = request("initial", Some(FindingStatus::Open), false);
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Uncertain);
    assert!(matches!(result.stop, Some(Stop::Unknown(_))));
    assert_eq!(effects(&host), vec![Effect::CreateSummary, Effect::Finding]);
    assert!(matches!(
        host.progress.unresolved.as_ref().unwrap().write,
        Write::Finding { .. }
    ));
    assert_eq!(
        host.snapshot.summaries[0].current.status,
        RoundStatus::Pending
    );
}

#[test]
fn an_existing_transition_requires_the_agents_matching_explanation() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let mut req = request("fixed", Some(FindingStatus::Fixed), false);
    req.transitions.clear();
    assert!(
        matches!(publish(&req, &mut host, &mut source, &mut renderer), Err(PlanError::MissingTransition(id)) if id.get() == 1)
    );
    assert!(host.writes.is_empty());
    req.transitions.push(TransitionReply {
        finding: FindingId::try_from(1).unwrap(),
        from: FindingStatus::Withdrawn,
        to: FindingStatus::Fixed,
        body: "The guard is covered by a regression case.".into(),
    });
    assert!(
        matches!(publish(&req, &mut host, &mut source, &mut renderer), Err(PlanError::InvalidTransition(id)) if id.get() == 1)
    );
    assert!(host.writes.is_empty());
    req.transitions[0].from = FindingStatus::Open;
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(host.writes.iter().any(|write| matches!(write, Write::Reply { body, .. } if body == "F1: Open -> Fixed; The guard is covered by a regression case.")));
}

#[test]
fn publication_results_must_match_agent_declared_coverage() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = request("clean", None, false);
    req.assessment = Assessment::new(AssessmentDraft {
        subject: subject(),
        reviewer: reviewer("agent"),
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
    assert!(matches!(
        publish(&req, &mut host, &mut source, &mut renderer),
        Err(PlanError::Coverage(_))
    ));
    assert!(host.writes.is_empty());
}

#[test]
fn private_agent_context_never_enters_rounds_or_provider_payloads() {
    let (mut host, mut source, mut renderer) = fixtures();
    let mut req = request("private", Some(FindingStatus::Open), false);
    let private = "PRIVATE deliberation never publish this token";
    req.local_context = private.into();
    let result = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert!(!format!("{:?}", host.writes).contains(private));
    assert!(!format!("{:?}", host.snapshot.summaries).contains(private));
    assert!(!format!("{result:?}").contains(private));
}

#[test]
fn summary_creation_and_update_authorities_are_distinct() {
    let (mut host, mut source, mut renderer) = fixtures();
    host.authority = Effects::new([Effect::CreateSummary, Effect::Finding]);
    let req = request("create-only", Some(FindingStatus::Open), false);
    let partial = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    assert!(matches!(
        partial.finalization_stop,
        Some(Stop::Authority(Effect::UpdateSummary))
    ));
    assert_eq!(effects(&host), vec![Effect::CreateSummary, Effect::Finding]);
    host.writes.clear();
    host.authority = Effects::new([Effect::UpdateSummary]);
    let completed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(completed.status, PublicationStatus::Denied);
    assert!(host.writes.is_empty());
    assert!(
        !host
            .progress
            .authorization
            .as_ref()
            .unwrap()
            .effects
            .contains(Effect::UpdateSummary)
    );
}

#[test]
fn summary_projection_counts_roots_and_labels_only_observed_actions() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let initial = renderer.summaries.last().unwrap();
    assert_eq!(initial.counts.total, 1);
    assert_eq!(initial.counts.open, 1);
    assert_eq!(
        initial.findings[0].root.root_note,
        provider_id("owned-root-note")
    );
    assert_eq!(initial.findings[0].actions, vec![PublishedAction::Created]);
    assert!(initial.unpublished_findings.is_empty());
    assert!(
        renderer
            .summaries
            .iter()
            .any(
                |view| view.unpublished_findings == vec![FindingId::try_from(1).unwrap()]
                    && view.findings.is_empty()
            )
    );

    host.fault = Some(Fault::Rejected(Effect::Resolve));
    let req = request("fixed", Some(FindingStatus::Fixed), false);
    let partial = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(partial.status, PublicationStatus::Partial);
    let projection = renderer.summaries.last().unwrap();
    assert_eq!(projection.counts.total, 1);
    assert_eq!(projection.counts.fixed, 1);
    assert_eq!(projection.counts.open, 0);
    assert_eq!(
        projection.findings[0].finding.status(),
        FindingStatus::Fixed
    );
    assert_eq!(
        projection.findings[0].actions,
        vec![PublishedAction::Transition {
            from: FindingStatus::Open,
            to: FindingStatus::Fixed
        }]
    );
    assert!(!projection.findings[0].root.resolved);

    let completed = publish(&req, &mut host, &mut source, &mut renderer).unwrap();
    assert_eq!(completed.status, PublicationStatus::Completed);
    let projection = renderer.summaries.last().unwrap();
    assert_eq!(projection.counts.total, 1);
    assert!(
        projection.findings[0]
            .actions
            .contains(&PublishedAction::Resolved)
    );
    assert!(projection.findings[0].root.resolved);

    let recurrent = publish(
        &request("recurrence", Some(FindingStatus::Open), false),
        &mut host,
        &mut source,
        &mut renderer,
    )
    .unwrap();
    assert_eq!(recurrent.status, PublicationStatus::Completed);
    let projection = renderer.summaries.last().unwrap();
    assert_eq!(projection.counts.total, 1);
    assert_eq!(projection.counts.open, 1);
    assert_eq!(projection.counts.fixed, 0);
    assert!(
        projection.findings[0]
            .actions
            .contains(&PublishedAction::Reopened)
    );
    assert_eq!(
        projection.findings[0].root.root_note,
        provider_id("owned-root-note")
    );
    assert!(!projection.findings[0].root.resolved);
}

#[path = "review_workflow/recovery_regressions.rs"]
mod recovery_regressions;

#[path = "review_workflow/history_profiles.rs"]
mod history_profiles;

#[path = "review_workflow/answer_targets.rs"]
mod answer_targets;

#[path = "review_workflow/prepared_intents.rs"]
mod prepared_intents;

#[path = "review_workflow/coverage_binding.rs"]
mod coverage_binding;

#[path = "review_workflow/settlement.rs"]
mod settlement;

#[path = "review_workflow/effectless_owners.rs"]
mod effectless_owners;
