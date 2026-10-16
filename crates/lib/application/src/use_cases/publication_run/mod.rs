#![doc = include_str!("README.md")]

use crate::observation as log;

use std::time::SystemTime;

use review_kit_domain::Assessment;

use super::{
    discovery::discover_managed,
    publishing::{
        BindingRequest, BoundInvocation, PlanError, PublicationProgress, PublicationResult,
        PublishRequest, plan_from_snapshot_with_progress_managed, publish_recorded_managed,
    },
};
use crate::{
    ports::{CommittedSource, PortError, PublicationHost, ReviewReader, ReviewRenderer},
    publication::{Plan, Snapshot, Write},
};

/// Requested planning or publication path; empty effects force planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Plan,
    Publish,
}

/// Supplies host timestamps without embedding clock I/O in the use case.
pub trait Clock {
    fn now(&mut self) -> SystemTime;
}

/// A recorder persists evidence before permitting the corresponding provider write.
/// Exact HTTP payloads and raw provider responses belong to the provider adapter.
#[review_kit_observation::observed_port(wrapper = ManagedEvidence, ownership = "borrowed", visibility = "pub(crate)")]
pub trait ExecutionEvidence {
    #[operation(name = "application.evidence.record", input = log::Fields::new().label("stage", log::evidence_stage(&event)), output = |result| log::Completion::result(result, |_| log::Fields::new(), log::port_error))]
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError>;
}

/// Typed evidence emitted as planning or publication reaches each operation.
/// `BeforeWrite` supplies the exact intent that durable callers retain before dispatch.
#[derive(Debug)]
pub enum ExecutionEvent<'a> {
    Started {
        at: SystemTime,
        assessment: &'a Assessment,
    },
    Observed(&'a Snapshot),
    Planned(&'a Plan),
    BeforeWrite(&'a Write),
    /// A retained `Prepared` write that can no longer resume. It never reached
    /// the provider and is dropped before publication or abandonment continues.
    DiscardedUnsent(&'a Write),
    Readback {
        write: &'a Write,
        snapshot: &'a Snapshot,
    },
    Progress {
        subject: &'a review_kit_domain::Subject,
        reviewer: &'a review_kit_domain::ReviewerId,
        progress: &'a PublicationProgress,
    },
    Finished(&'a RunReport),
}

/// Successful planning output or an execution report with its own communication status.
#[derive(Debug)]
pub enum RunOutcome {
    Planned(Box<Plan>),
    Published(Box<PublicationResult>),
}

/// The actual run path, submitted assessment, timing, and outcome.
/// A final evidence-recording failure remains separate from that outcome.
#[derive(Debug)]
pub struct RunReport {
    pub mode: RunMode,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub final_assessment: Assessment,
    pub outcome: Result<RunOutcome, PlanError>,
    /// Failure to save the completed run leaves its actual outcome available.
    pub evidence_failure: Option<PortError>,
}

/// Host adapters and retained progress borrowed for one invocation.
/// The caller serializes publication and restores any unresolved intent before use.
pub struct RunPorts<'a> {
    pub reader: &'a mut dyn ReviewReader,
    pub host: &'a mut dyn PublicationHost,
    pub source: &'a mut dyn CommittedSource,
    pub renderer: &'a mut dyn ReviewRenderer,
    pub clock: &'a mut dyn Clock,
    pub evidence: &'a mut dyn ExecutionEvidence,
    pub progress: &'a mut PublicationProgress,
}

#[review_kit_observation::operations]
#[helper]
/// Plan or publish a review while recording timing, observations, and effects.
///
/// Explicit plan mode and empty effects use discovery without acquiring a
/// publication session. Evidence failures before a provider write prevent it.
/// The report retains the actual outcome even if recording the finish fails.
/// Read-only planning can still call source, renderer, clock, and evidence adapters.
/// Publication refuses progress already bound to a review run; use `run_bound`.
pub fn run(request: &PublishRequest, mode: RunMode, ports: RunPorts<'_>) -> RunReport {
    run_bound(request, None, mode, ports)
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.run", input = log::request(request).label(
            "requested_mode",
            match mode {
                RunMode::Plan => "plan",
                RunMode::Publish => "publish",
            },
        ).flag("bound", binding.is_some()), output = log::run)]
/// Plan or publish as `run` does, binding publication to a review run's accepted inputs.
///
/// Publication admits the binding before any write and installs it with the
/// first prepared write; see the [publication contract](crate::use_cases::publishing).
/// Plan mode never changes the binding or the dispatch flag. The clock also
/// timestamps a provider-verified settlement of another run's binding.
/// The caller records this run's settlement afterwards with
/// `PublicationProgress::settle_publication`.
pub fn run_bound(
    request: &PublishRequest,
    binding: Option<&BindingRequest>,
    mode: RunMode,
    ports: RunPorts<'_>,
) -> RunReport {
    let RunPorts {
        reader,
        host,
        source,
        renderer,
        clock,
        evidence,
        progress,
    } = ports;
    let mut reader = crate::ports::ManagedReader::new(reader);
    let mut host = crate::ports::ManagedHost::new(host);
    let mut source = crate::ports::ManagedSource::new(source);
    let mut renderer = crate::ports::ManagedRenderer::new(renderer);
    let mut evidence = ManagedEvidence::new(evidence);
    let mode = if request.effects.is_empty() {
        RunMode::Plan
    } else {
        mode
    };
    let started_at = clock.now();
    let outcome = evidence
        .record(ExecutionEvent::Started {
            at: started_at,
            assessment: &request.assessment,
        })
        .map_err(PlanError::Evidence)
        .and_then(|()| match mode {
            RunMode::Plan => {
                let snapshot = discover_managed(
                    &mut reader,
                    request.assessment.subject(),
                    request.assessment.reviewer(),
                )
                .map_err(PlanError::Discovery)?;
                evidence
                    .record(ExecutionEvent::Observed(&snapshot))
                    .map_err(PlanError::Evidence)?;
                let plan = plan_from_snapshot_with_progress_managed(
                    request,
                    &snapshot,
                    &mut source,
                    &mut renderer,
                    progress,
                )?;
                evidence
                    .record(ExecutionEvent::Planned(&plan))
                    .map_err(PlanError::Evidence)?;
                Ok(RunOutcome::Planned(Box::new(plan)))
            }
            RunMode::Publish => publish_recorded_managed(
                request,
                &mut host,
                &mut source,
                &mut renderer,
                progress,
                &mut evidence,
                binding
                    .map(|request| BoundInvocation {
                        request,
                        clock: &mut *clock,
                    })
                    .as_mut(),
            )
            .map(|result| RunOutcome::Published(Box::new(result))),
        });
    let mut report = RunReport {
        mode,
        started_at,
        finished_at: clock.now(),
        final_assessment: request.assessment.clone(),
        outcome,
        evidence_failure: None,
    };
    report.evidence_failure = evidence.record(ExecutionEvent::Finished(&report)).err();
    report
}

pub(crate) struct NoEvidence;

#[review_kit_observation::operations]
impl ExecutionEvidence for NoEvidence {
    #[helper]
    fn record(&mut self, _: ExecutionEvent<'_>) -> Result<(), PortError> {
        Ok(())
    }
}
