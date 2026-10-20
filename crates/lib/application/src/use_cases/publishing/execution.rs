use crate::observation as log;
use review_kit_domain::{ReviewDecision, ReviewerId};

use super::action_write::action_write;
use super::binding::{BoundInvocation, RunBinding};
use super::planning::{PlanError, PublishRequest, build_plan, nonempty};
use super::progress::{PublicationProgress, UnresolvedWrite, WritePhase};
use crate::conversation::WriteTargets;
use crate::use_cases::discovery::{DiscoveryError, discover_managed, validate_snapshot};
use crate::use_cases::publication_run::{ExecutionEvent, ExecutionEvidence, NoEvidence};
use crate::{
    ports::{
        CommittedSource, ManagedSession, PortError, PublicationHost, ReviewRenderer, WriteOutcome,
    },
    publication::*,
};

/// Outcome of communicating a review, separate from the agent's review decision.
/// `Completed` means the required communication is verified, including a review
/// whose decision requires changes. `NotStarted` is an execution status; failures
/// before a report exists are returned as `PlanError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStatus {
    Unchanged,
    Completed,
    Blocked,
    Partial,
    Superseded,
    Uncertain,
    Denied,
    NotStarted,
    Abandoned,
}

/// The condition that stopped execution or final summary recording.
#[derive(Debug)]
pub enum Stop {
    UnresolvedWrite,
    Evidence(PortError),
    Authority(Effect),
    RevisionChanged,
    ConcurrentChange,
    Read(PortError),
    Render(PlanError),
    Preparation(PlanError),
    Rejected(PortError),
    Unknown(PortError),
    ReadbackMismatch,
    Conflict,
}

/// Exact effect intent and the observation that corroborates its owned effect.
/// An identical pending summary can be verified without sending another write.
#[derive(Debug)]
pub struct VerifiedEffect {
    pub write: Write,
    pub readback: Snapshot,
}

/// Communication outcome with attempts, verified effects, and retained stop reasons.
/// The assessment decision is independent of status. Verified earlier effects
/// remain visible when a later write, readback, or final summary operation fails.
#[derive(Debug)]
pub struct PublicationResult {
    pub final_snapshot: Snapshot,
    pub status: PublicationStatus,
    pub decision: ReviewDecision,
    pub round: u64,
    pub planned: Vec<PlannedAction>,
    pub attempted: Vec<Write>,
    pub verified: Vec<VerifiedEffect>,
    /// Each listed creation uses the one extra send and can produce a duplicate.
    pub retried_creations: Vec<Write>,
    pub skipped: Vec<PlannedAction>,
    pub stop: Option<Stop>,
    /// Failure to record the final status does not hide the original stop.
    pub finalization_stop: Option<Stop>,
}

/// Publish and read back authorized effects, retaining progress for recovery.
///
/// The caller serializes publication for this subject and reviewer and carries
/// existing progress across calls. Preflight failures return `PlanError`; execution
/// stops remain in `PublicationResult`. Effects can be partial or uncertain.
/// Retained policy permits one extra eligible creation send. Durable callers use
/// `publish_recorded` to persist preparation, dispatch and settlement checkpoints.
#[review_kit_observation::operations]
#[helper]
pub fn publish(
    request: &PublishRequest,
    host: &mut dyn PublicationHost,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
    progress: &mut PublicationProgress,
) -> Result<PublicationResult, PlanError> {
    publish_recorded(request, host, source, renderer, progress, &mut NoEvidence)
}

#[review_kit_observation::operations]
#[helper]
/// Publish with evidence for observations, the plan, intended writes, and readbacks.
///
/// Recording intent precedes dispatch; recording failure prevents that write.
/// The caller must retain and restore progress for crash recovery and serialize
/// publication for this subject and reviewer. Each effect needs exact readback;
/// provider acknowledgment alone does not complete publication.
/// Preflight errors return `PlanError`. Execution reports preserve verified effects
/// and separate the original stop from any finalization failure.
pub fn publish_recorded(
    request: &PublishRequest,
    host: &mut dyn PublicationHost,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
    progress: &mut PublicationProgress,
    evidence: &mut dyn ExecutionEvidence,
) -> Result<PublicationResult, PlanError> {
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    let mut host = crate::ports::ManagedHost::new(host);
    let mut evidence = crate::use_cases::publication_run::ManagedEvidence::new(evidence);
    publish_recorded_managed(
        request,
        &mut host,
        &mut source,
        &mut renderer,
        progress,
        &mut evidence,
        None,
    )
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.publish", input = log::request(request), output = log::publication_result)]
pub(crate) fn publish_recorded_managed(
    request: &PublishRequest,
    host: &mut crate::ports::ManagedHost<'_>,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    progress: &mut PublicationProgress,
    evidence: &mut crate::use_cases::publication_run::ManagedEvidence<'_>,
    mut binding: Option<&mut BoundInvocation<'_>>,
) -> Result<PublicationResult, PlanError> {
    let admission = progress.admit(binding.as_deref().map(|bound| bound.request))?;
    if !matches!(admission, super::binding::Admission::ReleaseOwner(_)) {
        super::progress::check_unresolved_profile(progress, renderer)?;
    }
    let assessment = &request.assessment;

    let mut session = host
        .acquire(assessment.subject(), assessment.reviewer())
        .map_err(|e| PlanError::Discovery(DiscoveryError::Read(e)))?;
    if session.authenticated_reviewer() != assessment.reviewer() {
        return Err(PlanError::WrongReviewer);
    }
    let initial_authority = session
        .authority()
        .map_err(|e| PlanError::Discovery(DiscoveryError::Read(e)))?;
    let mut snapshot = discover_managed(
        &mut session.reader(),
        assessment.subject(),
        assessment.reviewer(),
    )
    .map_err(super::planning::discovery_error)?;
    let owner = super::binding::owner_settlement(
        &admission,
        binding.as_deref_mut(),
        &snapshot,
        assessment.reviewer(),
    )?;
    super::planning::check_pending_profile(&snapshot, assessment.reviewer(), renderer)?;
    evidence
        .record(ExecutionEvent::Observed(&snapshot))
        .map_err(PlanError::Evidence)?;
    let admission = progress.settle_owner(
        admission,
        owner,
        &snapshot.subject,
        assessment.reviewer(),
        evidence,
    )?;
    let settled = progress.reconcile_recorded(&snapshot, assessment.reviewer(), evidence)?;
    if !settled {
        if let Some(pending) = progress.unresolved.clone()
            && super::recovery::eligible(&pending, progress, request, &snapshot, &initial_authority)
        {
            // Repeated complete reads can observe a late original before any resend.
            for _ in 0..2 {
                snapshot = discover_managed(
                    &mut session.reader(),
                    assessment.subject(),
                    assessment.reviewer(),
                )
                .map_err(super::planning::discovery_error)?;
                if progress.reconcile_recorded(&snapshot, assessment.reviewer(), evidence)? {
                    drop(session);
                    return publish_recorded_managed(
                        request, host, source, renderer, progress, evidence, binding,
                    );
                }
                if !super::recovery::eligible(
                    &pending,
                    progress,
                    request,
                    &snapshot,
                    &initial_authority,
                ) {
                    break;
                }
            }
            if super::recovery::eligible(&pending, progress, request, &snapshot, &initial_authority)
            {
                let plan = build_plan(request, &snapshot, source, renderer)?;
                if let Some(scope) = binding.as_deref().and_then(|bound| bound.scope) {
                    let plan = super::history::with_progress(
                        plan, request, &snapshot, renderer, progress,
                    )?;
                    require_scope(scope, &plan, &snapshot, Some(&pending.write))?;
                }
                let mut report = super::recovery::pending_report(
                    &pending,
                    &snapshot,
                    assessment.decision(),
                    progress,
                );
                let Some(grant) = progress.authorization.as_ref() else {
                    report.stop = Some(Stop::UnresolvedWrite);
                    return Ok(report);
                };
                let mut execution = Execution {
                    session,
                    reviewer: assessment.reviewer(),
                    authority: request
                        .effects
                        .intersection(&initial_authority)
                        .intersection(&grant.effects),
                    snapshot,
                    revision: assessment.revision().clone(),
                    evidence,
                    progress,
                    install: None,
                };
                match execution.apply_retained(
                    pending.write.clone(),
                    false,
                    &mut report,
                    Some(pending),
                ) {
                    Ok(()) => {
                        drop(execution);
                        // A dispatched write puts the invocation after a possible
                        // dispatch, where the scope check does not apply.
                        if let Some(bound) = binding.as_deref_mut() {
                            bound.scope = None;
                        }
                        let resumed = publish_recorded_managed(
                            request, host, source, renderer, progress, evidence, binding,
                        );
                        let mut resumed = match resumed {
                            Ok(resumed) => resumed,
                            Err(error) => {
                                let stop = Stop::Preparation(error);
                                report.status = status_for(&stop, report.attempted.is_empty());
                                report.stop = Some(stop);
                                return Ok(report);
                            }
                        };
                        resumed.attempted.splice(0..0, report.attempted);
                        resumed.verified.splice(0..0, report.verified);
                        return Ok(resumed);
                    }
                    Err(stop) => {
                        report.status = if execution.progress.unresolved.is_some() {
                            PublicationStatus::Uncertain
                        } else {
                            status_for(&stop, report.attempted.is_empty())
                        };
                        report.stop = Some(stop);
                        return Ok(report);
                    }
                }
            }
        }
        // A prepared write that cannot resume never left; publication continues
        // from the current plan, whose own checks stop stale effects.
        if !progress.discard_unsent(&snapshot.subject, assessment.reviewer(), evidence)? {
            let round = progress
                .unresolved
                .as_ref()
                .map_or(0, |pending| match &pending.write {
                    Write::Summary { round, .. } => round.number,
                    Write::Finding { round, .. } => *round,
                    Write::Reply { action, .. } => action.round,
                    Write::SetResolved { .. } => snapshot
                        .summaries
                        .iter()
                        .find(|summary| &summary.author == assessment.reviewer())
                        .map_or(0, |summary| summary.current.number),
                });
            return Ok(PublicationResult {
                final_snapshot: snapshot,
                status: PublicationStatus::Uncertain,
                decision: assessment.decision(),
                round,
                planned: Vec::new(),
                attempted: Vec::new(),
                verified: Vec::new(),
                retried_creations: progress.retries_in_round(round),
                skipped: Vec::new(),
                stop: Some(Stop::UnresolvedWrite),
                finalization_stop: None,
            });
        }
    }
    let plan = build_plan(request, &snapshot, source, renderer)?;
    let plan = super::history::with_progress(plan, request, &snapshot, renderer, progress)?;
    if let Some(scope) = binding.as_deref().and_then(|bound| bound.scope) {
        require_scope(scope, &plan, &snapshot, None)?;
    }
    let install =
        super::binding::installation(&admission, binding.as_deref(), progress, plan.round.number)?;
    let authority = request.effects.intersection(&initial_authority);
    let unknown_ceiling = progress
        .authorization
        .as_ref()
        .is_none_or(|grant| grant.round != plan.round.number)
        && snapshot.summaries.iter().any(|summary| {
            &summary.author == assessment.reviewer()
                && summary.current.number == plan.round.number
                && matches!(
                    summary.current.status,
                    RoundStatus::Pending | RoundStatus::Partial
                )
        });
    let retained_authority = match &progress.authorization {
        Some(retained) if retained.round == plan.round.number => {
            authority.intersection(&retained.effects)
        }
        _ if unknown_ceiling => Effects::none(),
        _ => authority,
    };
    evidence
        .record(ExecutionEvent::Planned(&plan))
        .map_err(PlanError::Evidence)?;
    let mut report = PublicationResult {
        final_snapshot: snapshot.clone(),
        status: if plan.unchanged {
            PublicationStatus::Unchanged
        } else {
            PublicationStatus::Partial
        },
        decision: assessment.decision(),
        round: plan.round.number,
        planned: plan.actions.clone(),
        attempted: Vec::new(),
        verified: Vec::new(),
        retried_creations: progress.retries_in_round(plan.round.number),
        skipped: plan.actions.clone(),
        stop: None,
        finalization_stop: None,
    };
    if plan.unchanged {
        return Ok(report);
    }
    let mut execution = Execution {
        session,
        reviewer: assessment.reviewer(),
        authority: retained_authority,
        snapshot,
        revision: assessment.revision().clone(),
        evidence,
        progress,
        install,
    };
    let mut terminal = plan.round.clone();
    terminal.status = if assessment.decision() == ReviewDecision::Blocked {
        RoundStatus::Blocked
    } else {
        RoundStatus::Completed
    };
    let expected = execution
        .snapshot
        .summaries
        .iter()
        .find(|s| &s.author == execution.reviewer)
        .cloned()
        .map(Box::new);
    let mut start = Write::Summary {
        expected,
        round: Box::new(plan.round.clone()),
        history: plan.history.clone(),
        body: plan.summary_body.clone(),
    };
    if plan.actions.is_empty()
        && let Some(summary) = execution.snapshot.summaries.iter().find(|summary| {
            &summary.author == execution.reviewer
                && summary.current.number == plan.round.number
                && summary.body == plan.summary_body
                && matches!(
                    summary.current.status,
                    RoundStatus::Pending | RoundStatus::Partial
                )
        })
    {
        start = Write::Summary {
            expected: Some(Box::new(summary.clone())),
            round: Box::new(summary.current.clone()),
            history: summary.history.clone(),
            body: summary.body.clone(),
        };
    }
    if let Err(stop) = execution.apply(start, false, &mut report) {
        let no_writes = report.attempted.is_empty()
            || (report.verified.is_empty() && matches!(stop, Stop::Rejected(_)));
        report.status = status_for(&stop, no_writes);
        report.stop = Some(stop);
        if report.status == PublicationStatus::Superseded && !report.verified.is_empty() {
            terminal.status = RoundStatus::Superseded;
            execution.finalize(terminal, plan.history, renderer, &mut report);
        }
        return Ok(report);
    }
    if plan.round.status == RoundStatus::Blocked && plan.actions.is_empty() {
        report.status = PublicationStatus::Blocked;
        return Ok(report);
    }
    for (index, action) in plan.actions.iter().enumerate() {
        let verified_before = report.verified.len();
        let write = execution.action_write(action, plan.round.number);
        let outcome = write.and_then(|write| execution.apply(write, false, &mut report));
        if let Err(stop) = outcome {
            report.status = status_for(&stop, report.attempted.is_empty());
            report.stop = Some(stop);
            let remaining = index + usize::from(report.verified.len() > verified_before);
            report.skipped = plan.actions[remaining..].to_vec();
            terminal.status = if report.status == PublicationStatus::Superseded {
                RoundStatus::Superseded
            } else {
                RoundStatus::Partial
            };
            // Ambiguous state must remain pending until it can be reconciled.
            if report.status != PublicationStatus::Uncertain
                && report.status != PublicationStatus::NotStarted
                && report.status != PublicationStatus::Denied
                && !matches!(report.stop, Some(Stop::ConcurrentChange))
            {
                execution.finalize(terminal, plan.history, renderer, &mut report);
            }
            return Ok(report);
        }
        report.skipped = plan.actions[index + 1..].to_vec();
    }
    let completed_status = if terminal.status == RoundStatus::Blocked {
        PublicationStatus::Blocked
    } else {
        PublicationStatus::Completed
    };
    execution.finalize(terminal, plan.history, renderer, &mut report);
    if report.finalization_stop.is_none() && report.status != PublicationStatus::Superseded {
        report.status = completed_status;
    } else if let Some(stop) = &report.finalization_stop {
        report.status = status_for(stop, report.attempted.is_empty());
    }
    Ok(report)
}

/// Refuse an invocation that would write to an existing object outside `scope`.
#[review_kit_observation::operations]
#[helper]
fn require_scope(
    scope: &WriteTargets,
    plan: &Plan,
    snapshot: &Snapshot,
    pending: Option<&Write>,
) -> Result<(), PlanError> {
    if scope.covers(&WriteTargets::of(plan, &[snapshot], pending)) {
        Ok(())
    } else {
        Err(PlanError::UncomparedWrite)
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn status_for(stop: &Stop, no_writes: bool) -> PublicationStatus {
    if no_writes && matches!(stop, Stop::Evidence(_) | Stop::Read(_) | Stop::Rejected(_)) {
        return PublicationStatus::NotStarted;
    }
    match stop {
        Stop::UnresolvedWrite => PublicationStatus::Uncertain,
        Stop::RevisionChanged => PublicationStatus::Superseded,
        Stop::Unknown(_)
        | Stop::ReadbackMismatch
        | Stop::Conflict
        | Stop::Evidence(_)
        | Stop::Preparation(_) => PublicationStatus::Uncertain,
        Stop::Authority(_) if no_writes => PublicationStatus::Denied,
        Stop::Read(_) if !no_writes => PublicationStatus::Uncertain,
        _ => PublicationStatus::Partial,
    }
}

#[review_kit_observation::operations]
#[helper]
fn readback_failure(outcome: WriteOutcome, failure: Stop) -> Stop {
    // Failed observation cannot make a confirmed no-effect rejection uncertain.
    match outcome {
        WriteOutcome::Rejected(error) => Stop::Rejected(error),
        _ => failure,
    }
}

pub(super) struct Execution<'a, 'evidence> {
    pub(super) evidence: &'a mut crate::use_cases::publication_run::ManagedEvidence<'evidence>,
    pub(super) progress: &'a mut PublicationProgress,
    pub(super) session: ManagedSession<'a>,
    pub(super) reviewer: &'a ReviewerId,
    pub(super) authority: Effects,
    pub(super) snapshot: Snapshot,
    pub(super) revision: review_kit_domain::Revision,
    /// Binding installed with this invocation's first prepared write.
    pub(super) install: Option<RunBinding>,
}

#[review_kit_observation::operations]
impl Execution<'_, '_> {
    #[helper]
    fn summary_write(
        &self,
        mut round: Round,
        history: Vec<Round>,
        renderer: &mut crate::ports::ManagedRenderer<'_>,
    ) -> Result<Write, PlanError> {
        crate::summary::retain_communicated_findings(&mut round, &history, &self.snapshot)?;
        for receipt in self.progress.verified_resolutions.iter().filter(|receipt| {
            receipt.subject == self.snapshot.subject
                && &receipt.reviewer == self.reviewer
                && receipt.round == round.number
        }) {
            if !round.verified_resolutions.contains(receipt) {
                round.verified_resolutions.push(receipt.clone());
            }
        }
        crate::summary::retain_verified_resolutions(&mut round);
        let body = nonempty(
            renderer
                .summary(&crate::summary::summary_view(
                    &round,
                    &history,
                    &self.snapshot,
                )?)
                .map_err(PlanError::Render)?,
        )?;
        let expected = self
            .snapshot
            .summaries
            .iter()
            .find(|s| &s.author == self.reviewer)
            .cloned()
            .map(Box::new);
        Ok(Write::Summary {
            expected,
            round: Box::new(round),
            history,
            body,
        })
    }

    #[helper]
    fn action_write(&self, action: &PlannedAction, round: u64) -> Result<Write, Stop> {
        action_write(action, round, &self.revision, &self.snapshot, self.reviewer)
            .map_err(|_| Stop::Conflict)
    }

    #[operation(name = "application.publication.apply", input = log::write(&write).flag("allow_stale_summary", allow_stale_summary), output = |result| match result {
                Ok(()) => log::Completion::success(
                    log::Fields::new().label("effect_state", "readback_verified"),
                ),
                Err(error) => log::Completion::new(
                    match error {
                        Stop::UnresolvedWrite | Stop::Unknown(_) | Stop::ReadbackMismatch => {
                            log::Outcome::Uncertain
                        }
                        Stop::Rejected(_) | Stop::Authority(_) => log::Outcome::Rejected,
                        _ => log::Outcome::Failure,
                    },
                    log::Fields::new(),
                )
                .with_error(log::stop_error(error)),
            })]
    pub(super) fn apply(
        &mut self,
        write: Write,
        allow_stale_summary: bool,
        report: &mut PublicationResult,
    ) -> Result<(), Stop> {
        self.apply_retained(write, allow_stale_summary, report, None)
    }

    #[helper]
    pub(super) fn apply_retained(
        &mut self,
        write: Write,
        allow_stale_summary: bool,
        report: &mut PublicationResult,
        retained: Option<UnresolvedWrite>,
    ) -> Result<(), Stop> {
        if self.progress.unresolved.is_some() && retained.is_none() {
            return Err(Stop::UnresolvedWrite);
        }
        let fresh = self
            .session
            .reader()
            .discover(&self.snapshot.subject)
            .map_err(Stop::Read)?;
        validate_snapshot(&fresh, &self.snapshot.subject, self.reviewer)
            .map_err(|_| Stop::Conflict)?;
        let changed_revision = fresh.revision != self.revision;
        if !same_owned_state(&fresh, &self.snapshot, self.reviewer) {
            self.snapshot = fresh;
            report.final_snapshot = self.snapshot.clone();
            return Err(Stop::ConcurrentChange);
        }
        self.snapshot = fresh;
        report.final_snapshot = self.snapshot.clone();
        self.evidence
            .record(ExecutionEvent::Observed(&self.snapshot))
            .map_err(Stop::Evidence)?;
        if changed_revision && !(allow_stale_summary && matches!(write, Write::Summary { .. })) {
            return Err(Stop::RevisionChanged);
        }
        let current_authority = self.session.authority().map_err(Stop::Read)?;
        // A capability lost in this invocation cannot be regained by a later call.
        self.authority = self.authority.intersection(&current_authority);
        if !self.authority.contains(write.effect())
            || !self.snapshot.permissions.contains(write.effect())
        {
            return Err(Stop::Authority(write.effect()));
        }
        if matches!(write, Write::Summary { .. })
            && verified_write(&write, &self.snapshot, self.reviewer)
        {
            report.verified.push(VerifiedEffect {
                write,
                readback: self.snapshot.clone(),
            });
            return Ok(());
        }
        let previous_authorization = self.progress.authorization.clone();
        let mut established_authorization = false;
        if let Write::Summary { round, .. } = &write
            && round.status != RoundStatus::Abandoned
            && self
                .progress
                .authorization
                .as_ref()
                .is_none_or(|grant| grant.round != round.number)
        {
            self.progress.authorization = Some(super::progress::RoundAuthorization {
                round: round.number,
                effects: self.authority.clone(),
            });
            established_authorization = true;
        }
        let renderer_identity = match &write {
            Write::Summary { round, .. } => round.renderer.as_ref(),
            _ => self
                .snapshot
                .summaries
                .iter()
                .find(|summary| &summary.author == self.reviewer)
                .and_then(|summary| summary.current.renderer.as_ref()),
        };
        let installed = self.install.take().map(|mut binding| {
            binding.prior_authorization = previous_authorization.clone();
            let previous = (
                self.progress.coverage_binding.replace(binding),
                self.progress.dispatch_may_have_started,
            );
            self.progress.dispatch_may_have_started = false;
            previous
        });
        self.progress
            .before_write(&self.snapshot, self.reviewer, &write, renderer_identity);
        let retry = retained
            .as_ref()
            .is_some_and(|pending| pending.phase != WritePhase::Prepared);
        if let Some(pending) = retained.as_ref().filter(|_| retry) {
            self.progress.unresolved = Some(pending.clone());
        }
        self.checkpoint()?;
        if let Err(error) = self.evidence.record(ExecutionEvent::BeforeWrite(&write)) {
            return Err(Stop::Evidence(error));
        }
        let before = self.snapshot.clone();
        let before_dispatch_checkpoint = self.progress.clone();
        let dispatch_flag_before = self.progress.dispatch_may_have_started;
        let mut checkpoint_error = None;
        let mut dispatched = false;
        let progress = &mut *self.progress;
        let evidence = &mut *self.evidence;
        let outcome = self.session.write_with_checkpoint(
            &self.snapshot.subject,
            self.reviewer,
            &write,
            &mut || {
                let Some(pending) = progress.unresolved.as_mut() else {
                    checkpoint_error = Some(Stop::Conflict);
                    return Err(PortError::new(
                        "checkpoint dispatch",
                        std::io::Error::other("prepared intent unavailable"),
                    ));
                };
                if retry && !progress.retried_creations.contains(&write) {
                    progress.retried_creations.push(write.clone());
                }
                pending.phase = WritePhase::DispatchPossible;
                if progress.coverage_binding.is_some() {
                    progress.dispatch_may_have_started = true;
                }
                if let Err(error) = evidence.record(ExecutionEvent::Progress {
                    subject: &self.snapshot.subject,
                    reviewer: self.reviewer,
                    progress,
                }) {
                    checkpoint_error = Some(Stop::Evidence(error));
                    return Err(PortError::new(
                        "checkpoint dispatch",
                        std::io::Error::other("publication progress unavailable"),
                    ));
                }
                dispatched = true;
                Ok(())
            },
        );
        if let Some(stop) = checkpoint_error {
            *self.progress = before_dispatch_checkpoint;
            return Err(stop);
        }
        if dispatched {
            report.attempted.push(write.clone());
            if retry && !report.retried_creations.contains(&write) {
                report.retried_creations.push(write.clone());
            }
        }
        let mut rolled_back = None;
        if matches!(outcome, WriteOutcome::Rejected(_)) && !retry {
            self.progress.unresolved = None;
            if established_authorization {
                self.progress.authorization = previous_authorization;
            }
            rolled_back = self.reject_binding(installed, dispatch_flag_before, report);
            self.checkpoint()?;
        }
        let observed = match self.session.reader().readback(&self.snapshot.subject) {
            Ok(snapshot) => snapshot,
            Err(error) => return Err(readback_failure(outcome, Stop::Read(error))),
        };
        if validate_snapshot(&observed, &self.snapshot.subject, self.reviewer).is_err() {
            self.snapshot = observed;
            report.final_snapshot = self.snapshot.clone();
            self.evidence
                .record(ExecutionEvent::Readback {
                    write: &write,
                    snapshot: &self.snapshot,
                })
                .map_err(Stop::Evidence)?;
            return Err(Stop::Conflict);
        }
        let verified = verified_write(&write, &observed, self.reviewer);
        let revision_changed = observed.revision != self.revision;
        self.snapshot = observed;
        report.final_snapshot = self.snapshot.clone();
        let recorded = self.evidence.record(ExecutionEvent::Readback {
            write: &write,
            snapshot: &self.snapshot,
        });
        if !verified {
            if let Err(error) = recorded {
                return Err(readback_failure(outcome, Stop::Evidence(error)));
            }
            if !same_owned_state(&before, &self.snapshot, self.reviewer) {
                return Err(match outcome {
                    WriteOutcome::Rejected(_) => Stop::ConcurrentChange,
                    _ => Stop::ReadbackMismatch,
                });
            }
            if revision_changed && !allow_stale_summary {
                return Err(match outcome {
                    WriteOutcome::Unknown(error) => Stop::Unknown(error),
                    WriteOutcome::Acknowledged => Stop::ReadbackMismatch,
                    WriteOutcome::Rejected(_) => Stop::RevisionChanged,
                });
            }
            return Err(match outcome {
                WriteOutcome::Rejected(e) => Stop::Rejected(e),
                WriteOutcome::Unknown(e) => Stop::Unknown(e),
                WriteOutcome::Acknowledged => Stop::ReadbackMismatch,
            });
        }
        let isolated = only_expected_change(&before, &self.snapshot, &write, self.reviewer);
        let target_verified = self.progress.unresolved.as_ref().is_none_or(|pending| {
            super::progress::matches_retained_target(pending, &self.snapshot)
        });
        report.verified.push(VerifiedEffect {
            write: write.clone(),
            readback: self.snapshot.clone(),
        });
        if let Some((binding, dispatched)) = rolled_back {
            // A verified effect stays attributed to the binding that produced it.
            self.progress.coverage_binding = binding;
            self.progress.dispatch_may_have_started = dispatched;
        }
        if target_verified {
            self.progress.unresolved = None;
        }
        if let Write::SetResolved { thread, resolved } = &write
            && target_verified
            && let Some(summary) = before
                .summaries
                .iter()
                .find(|summary| &summary.author == self.reviewer)
        {
            let receipt = ResolutionReceipt {
                subject: self.snapshot.subject.clone(),
                reviewer: self.reviewer.clone(),
                round: summary.current.number,
                thread: thread.clone(),
                resolved: *resolved,
                provenance: if matches!(outcome, WriteOutcome::Acknowledged) {
                    ResolutionProvenance::Acknowledged
                } else {
                    ResolutionProvenance::Observed
                },
            };
            if !self.progress.verified_resolutions.contains(&receipt) {
                self.progress.verified_resolutions.push(receipt);
            }
        }
        self.checkpoint()?;
        recorded.map_err(Stop::Evidence)?;
        if !isolated {
            return Err(Stop::ConcurrentChange);
        }
        if revision_changed && !allow_stale_summary {
            return Err(Stop::RevisionChanged);
        }
        Ok(())
    }

    /// Undo binding state for a definitely rejected write and return what it replaced.
    ///
    /// A binding installed by this write is removed when no effect is verified in
    /// this invocation; otherwise the dispatch flag returns to its value before
    /// this write's dispatch checkpoint. A rejected first dispatch of a binding
    /// therefore leaves it without a possible provider effect, so the admission
    /// of a replacing run can release it.
    #[helper]
    fn reject_binding(
        &mut self,
        installed: Option<(Option<RunBinding>, bool)>,
        dispatch_flag_before: bool,
        report: &PublicationResult,
    ) -> Option<(Option<RunBinding>, bool)> {
        let current = (
            self.progress.coverage_binding.clone(),
            self.progress.dispatch_may_have_started,
        );
        match installed.filter(|_| report.verified.is_empty()) {
            Some((binding, dispatched)) => {
                self.progress.coverage_binding = binding;
                self.progress.dispatch_may_have_started = dispatched;
            }
            None => self.progress.dispatch_may_have_started = dispatch_flag_before,
        }
        let changed = current
            != (
                self.progress.coverage_binding.clone(),
                self.progress.dispatch_may_have_started,
            );
        changed.then_some(current)
    }

    #[helper]
    pub(super) fn checkpoint(&mut self) -> Result<(), Stop> {
        self.evidence
            .record(ExecutionEvent::Progress {
                subject: &self.snapshot.subject,
                reviewer: self.reviewer,
                progress: self.progress,
            })
            .map_err(Stop::Evidence)
    }

    #[helper]
    fn finalize(
        &mut self,
        mut terminal: Round,
        history: Vec<Round>,
        renderer: &mut crate::ports::ManagedRenderer<'_>,
        report: &mut PublicationResult,
    ) {
        if self.progress.unresolved.is_some() {
            report.finalization_stop = Some(Stop::UnresolvedWrite);
            return;
        }
        terminal.publication_limitation = report.stop.as_ref().map(publication_limitation);
        let fresh = match self.session.reader().discover(&self.snapshot.subject) {
            Ok(s) => s,
            Err(e) => {
                report.finalization_stop = Some(Stop::Read(e));
                return;
            }
        };
        if validate_snapshot(&fresh, &self.snapshot.subject, self.reviewer).is_err() {
            report.finalization_stop = Some(Stop::Conflict);
            return;
        }
        if !same_owned_state(&fresh, &self.snapshot, self.reviewer) {
            report.finalization_stop = Some(Stop::ConcurrentChange);
            self.snapshot = fresh;
            report.final_snapshot = self.snapshot.clone();
            return;
        }
        self.snapshot = fresh;
        report.final_snapshot = self.snapshot.clone();
        if let Err(error) = self
            .evidence
            .record(ExecutionEvent::Observed(&self.snapshot))
        {
            report.finalization_stop = Some(Stop::Evidence(error));
            return;
        }
        if self.snapshot.revision != self.revision {
            terminal.status = RoundStatus::Superseded;
            terminal.publication_limitation = Some(PublicationLimitation::RevisionChanged);
            report.status = PublicationStatus::Superseded;
            report.stop.get_or_insert(Stop::RevisionChanged);
        }
        let superseded = terminal.status == RoundStatus::Superseded;
        let write = match self.summary_write(terminal.clone(), history.clone(), renderer) {
            Ok(w) => w,
            Err(e) => {
                report.finalization_stop = Some(Stop::Render(e));
                return;
            }
        };
        match self.apply(write, superseded, report) {
            Err(Stop::RevisionChanged) if !superseded => {
                // The revision can change while the completed body is rendered
                // or written. Repair its status once without resuming findings.
                terminal.status = RoundStatus::Superseded;
                terminal.publication_limitation = Some(PublicationLimitation::RevisionChanged);
                report.status = PublicationStatus::Superseded;
                report.stop.get_or_insert(Stop::RevisionChanged);
                match self.summary_write(terminal, history, renderer) {
                    Ok(write) => {
                        if let Err(stop) = self.apply(write, true, report) {
                            report.finalization_stop = Some(stop);
                        }
                    }
                    Err(error) => report.finalization_stop = Some(Stop::Render(error)),
                }
            }
            Err(error) => report.finalization_stop = Some(error),
            Ok(()) => {}
        }
    }
}

#[review_kit_observation::operations]
#[helper]
fn publication_limitation(stop: &Stop) -> PublicationLimitation {
    match stop {
        Stop::Authority(effect) => PublicationLimitation::Authority(*effect),
        Stop::RevisionChanged => PublicationLimitation::RevisionChanged,
        Stop::ConcurrentChange | Stop::Conflict => PublicationLimitation::ConcurrentChange,
        Stop::Read(_) => PublicationLimitation::ProviderUnavailable,
        Stop::Rejected(_) => PublicationLimitation::Rejected,
        Stop::Unknown(_) | Stop::ReadbackMismatch | Stop::UnresolvedWrite => {
            PublicationLimitation::Uncertain
        }
        Stop::Render(_) => PublicationLimitation::InvalidRendering,
        Stop::Preparation(_) => PublicationLimitation::PreparationFailed,
        Stop::Evidence(_) => PublicationLimitation::EvidenceUnavailable,
    }
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn only_expected_change(
    before: &Snapshot,
    observed: &Snapshot,
    write: &Write,
    reviewer: &ReviewerId,
) -> bool {
    let mut expected = before.clone();
    match write {
        Write::Summary { .. } => {
            expected.summaries.retain(|r| &r.author != reviewer);
            expected.summaries.extend(
                observed
                    .summaries
                    .iter()
                    .filter(|r| &r.author == reviewer)
                    .cloned(),
            );
        }
        Write::Finding { finding, .. } => expected.findings.extend(
            observed
                .findings
                .iter()
                .filter(|r| &r.author == reviewer && r.finding.id() == finding.id())
                .cloned(),
        ),
        Write::Reply { action, .. } => expected.replies.extend(
            observed
                .replies
                .iter()
                .filter(|r| &r.author == reviewer && r.action.as_ref() == Some(action))
                .cloned(),
        ),
        Write::SetResolved { thread, resolved } => {
            if let Some(root) = expected
                .findings
                .iter_mut()
                .find(|r| &r.author == reviewer && &r.id == thread)
            {
                root.resolved = *resolved;
            }
        }
    }
    same_owned_state(&expected, observed, reviewer)
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn same_owned_state(left: &Snapshot, right: &Snapshot, reviewer: &ReviewerId) -> bool {
    use std::collections::BTreeMap;
    let summaries = |s: &Snapshot| {
        s.summaries
            .iter()
            .filter(|r| &r.author == reviewer)
            .map(|r| (r.id.clone(), r.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let findings = |s: &Snapshot| {
        s.findings
            .iter()
            .filter(|r| &r.author == reviewer)
            .map(|r| (r.id.clone(), r.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let replies = |s: &Snapshot| {
        s.replies
            .iter()
            .filter(|r| {
                s.findings
                    .iter()
                    .any(|root| &root.author == reviewer && root.id == r.thread)
            })
            .map(|r| (r.id.clone(), r.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    summaries(left) == summaries(right)
        && findings(left) == findings(right)
        && replies(left) == replies(right)
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.verify_effect", input = log::write(write), output = |matched| log::Completion::success(log::Fields::new().flag("matched", *matched)))]
pub(super) fn verified_write(write: &Write, snapshot: &Snapshot, reviewer: &ReviewerId) -> bool {
    match write {
        Write::Summary {
            expected,
            round,
            history,
            body,
        } => {
            let matches: Vec<_> = snapshot
                .summaries
                .iter()
                .filter(|s| &s.author == reviewer)
                .collect();
            matches!(matches.as_slice(), [s] if s.current == **round && &s.history == history && &s.body == body && expected.as_ref().is_none_or(|old| old.id == s.id))
        }
        Write::Finding {
            round,
            revision,
            finding,
            body,
        } => {
            let matches: Vec<_> = snapshot
                .findings
                .iter()
                .filter(|s| &s.author == reviewer && s.finding.id() == finding.id())
                .collect();
            matches!(matches.as_slice(), [s] if s.finding == **finding && &s.body == body && s.created_in == *round && &s.revision == revision)
        }
        Write::Reply {
            thread,
            action,
            body,
        } => {
            let matches: Vec<_> = snapshot
                .replies
                .iter()
                .filter(|s| &s.author == reviewer && s.action.as_ref() == Some(action))
                .collect();
            matches!(matches.as_slice(), [s] if &s.thread == thread && &s.body == body)
        }
        Write::SetResolved { thread, resolved } => snapshot
            .findings
            .iter()
            .any(|s| &s.author == reviewer && &s.id == thread && s.resolved == *resolved),
    }
}
