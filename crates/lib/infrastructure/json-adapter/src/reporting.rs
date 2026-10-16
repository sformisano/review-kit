use std::time::{SystemTime, UNIX_EPOCH};

use review_kit_application::{
    publication::Plan,
    use_cases::{
        publication_run::{ExecutionEvent, RunMode, RunOutcome, RunReport},
        publishing::{PlanError, PublicationResult, PublicationStatus},
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    Diagnostic, SnapshotData, WriteData,
    domain::{AssessmentData, Decision},
    rounds::{PlannedData, RoundData},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanData {
    pub round: RoundData,
    pub history: Vec<RoundData>,
    pub actions: Vec<PlannedData>,
    pub unchanged: bool,
    pub summary_body: String,
    pub terminal_summary_preview: String,
    pub unresolved_findings: Vec<u64>,
}

impl From<&Plan> for PlanData {
    fn from(value: &Plan) -> Self {
        Self {
            round: (&value.round).into(),
            history: value.history.iter().map(Into::into).collect(),
            actions: value.actions.iter().map(Into::into).collect(),
            unchanged: value.unchanged,
            summary_body: value.summary_body.clone(),
            terminal_summary_preview: value.terminal_summary_preview.clone(),
            unresolved_findings: value
                .unresolved_findings
                .iter()
                .map(|id| id.get())
                .collect(),
        }
    }
}

document_kind!(PlanKind, Plan, "plan");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDocument {
    pub version: u64,
    pub kind: PlanKind,
    pub plan: PlanData,
}

impl From<&Plan> for PlanDocument {
    fn from(value: &Plan) -> Self {
        Self {
            version: 1,
            kind: PlanKind::Plan,
            plan: value.into(),
        }
    }
}

document_kind!(CheckKind, Check, "check");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDocument {
    pub version: u64,
    pub kind: CheckKind,
    pub valid: bool,
    pub plan: Option<PlanData>,
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckDocument {
    pub fn from_result(result: &Result<Plan, Vec<PlanError>>) -> Self {
        match result {
            Ok(plan) => Self {
                version: 1,
                kind: CheckKind::Check,
                valid: true,
                plan: Some(plan.into()),
                diagnostics: Vec::new(),
            },
            Err(errors) => Self::failed(
                errors
                    .iter()
                    .flat_map(Diagnostic::from_plan_error)
                    .collect(),
            ),
        }
    }
    pub fn failed(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            version: 1,
            kind: CheckKind::Check,
            valid: false,
            plan: None,
            diagnostics,
        }
    }
}

wire_enum!(
    PublicationState,
    PublicationStatus,
    [
        Unchanged, Completed, Blocked, Partial, Superseded, Uncertain, Denied, NotStarted,
        Abandoned
    ]
);
wire_enum!(RunModeData, RunMode, [Plan, Publish]);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedEffectData {
    pub write: WriteData,
    pub readback: SnapshotData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationData {
    pub final_snapshot: SnapshotData,
    pub status: PublicationState,
    pub decision: Decision,
    pub round: u64,
    pub planned: Vec<PlannedData>,
    pub attempted: Vec<WriteData>,
    pub verified: Vec<VerifiedEffectData>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retried_creations: Vec<WriteData>,
    pub skipped: Vec<PlannedData>,
    pub stop: Vec<Diagnostic>,
    pub finalization_stop: Vec<Diagnostic>,
}

impl From<&PublicationResult> for PublicationData {
    fn from(value: &PublicationResult) -> Self {
        Self {
            final_snapshot: (&value.final_snapshot).into(),
            status: value.status.into(),
            decision: value.decision.into(),
            round: value.round,
            planned: value.planned.iter().map(Into::into).collect(),
            attempted: value.attempted.iter().map(Into::into).collect(),
            verified: value
                .verified
                .iter()
                .map(|effect| VerifiedEffectData {
                    write: (&effect.write).into(),
                    readback: (&effect.readback).into(),
                })
                .collect(),
            skipped: value.skipped.iter().map(Into::into).collect(),
            retried_creations: value.retried_creations.iter().map(Into::into).collect(),
            stop: value
                .stop
                .as_ref()
                .map(Diagnostic::from_stop)
                .unwrap_or_default(),
            finalization_stop: value
                .finalization_stop
                .as_ref()
                .map(Diagnostic::from_stop)
                .unwrap_or_default(),
        }
    }
}

document_kind!(
    PublicationResultKind,
    PublicationResult,
    "publication_result"
);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationResultDocument {
    pub version: u64,
    pub kind: PublicationResultKind,
    pub result: PublicationData,
}

impl From<&PublicationResult> for PublicationResultDocument {
    fn from(value: &PublicationResult) -> Self {
        Self {
            version: 1,
            kind: PublicationResultKind::PublicationResult,
            result: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimestampData {
    pub seconds: u64,
    pub nanoseconds: u32,
    pub before_epoch: bool,
}

impl From<SystemTime> for TimestampData {
    fn from(value: SystemTime) -> Self {
        let (duration, before_epoch) = match value.duration_since(UNIX_EPOCH) {
            Ok(duration) => (duration, false),
            Err(error) => (error.duration(), true),
        };
        Self {
            seconds: duration.as_secs(),
            nanoseconds: duration.subsec_nanos(),
            before_epoch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunOutcomeData {
    Planned { plan: Box<PlanData> },
    Published { result: Box<PublicationData> },
    Failed { diagnostics: Vec<Diagnostic> },
}

document_kind!(RunKind, Run, "run");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDocument {
    pub version: u64,
    pub kind: RunKind,
    pub mode: RunModeData,
    pub started_at: TimestampData,
    pub finished_at: TimestampData,
    pub final_assessment: AssessmentData,
    pub outcome: RunOutcomeData,
    pub evidence_failure: Option<Diagnostic>,
    /// Host-selected rendering profile; absence means it was not recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
}

pub type RunReportDocument = RunDocument;

document_kind!(EvidenceKind, Evidence, "evidence");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDocument {
    pub version: u64,
    pub kind: EvidenceKind,
    pub subject: crate::domain::SubjectData,
    pub reviewer: String,
    pub events: Vec<EvidenceEventData>,
}

impl EvidenceDocument {
    /// Whether the events show that publication or abandonment began: a
    /// progress checkpoint, a write intent or a discarded intent, or a finished
    /// publication run. Planning records none of them.
    pub fn records_publication_attempt(&self) -> bool {
        self.events.iter().any(|event| match event {
            EvidenceEventData::Progress { .. }
            | EvidenceEventData::BeforeWrite { .. }
            | EvidenceEventData::DiscardedUnsent { .. } => true,
            EvidenceEventData::Finished { report } => matches!(report.mode, RunModeData::Publish),
            EvidenceEventData::Started { .. }
            | EvidenceEventData::Observed { .. }
            | EvidenceEventData::Planned { .. }
            | EvidenceEventData::Readback { .. } => false,
        })
    }
}

impl From<&RunReport> for RunDocument {
    fn from(value: &RunReport) -> Self {
        let outcome = match &value.outcome {
            Ok(RunOutcome::Planned(plan)) => RunOutcomeData::Planned {
                plan: Box::new(plan.as_ref().into()),
            },
            Ok(RunOutcome::Published(result)) => RunOutcomeData::Published {
                result: Box::new(result.as_ref().into()),
            },
            Err(error) => RunOutcomeData::Failed {
                diagnostics: Diagnostic::from_plan_error(error),
            },
        };
        Self {
            version: 1,
            kind: RunKind::Run,
            mode: value.mode.into(),
            started_at: value.started_at.into(),
            finished_at: value.finished_at.into(),
            final_assessment: (&value.final_assessment).into(),
            outcome,
            evidence_failure: value
                .evidence_failure
                .as_ref()
                .map(|error| Diagnostic::new("evidence_unavailable", &crate::error_chain(error))),
            renderer: None,
        }
    }
}

document_kind!(AbandonKind, Abandon, "abandon");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbandonDocument {
    pub version: u64,
    pub kind: AbandonKind,
    pub result: Option<PublicationData>,
    pub diagnostics: Vec<Diagnostic>,
}

impl AbandonDocument {
    pub fn from_result(result: &Result<PublicationResult, PlanError>) -> Self {
        match result {
            Ok(result) => Self {
                version: 1,
                kind: AbandonKind::Abandon,
                result: Some(result.into()),
                diagnostics: Vec::new(),
            },
            Err(error) => Self {
                version: 1,
                kind: AbandonKind::Abandon,
                result: None,
                diagnostics: Diagnostic::from_plan_error(error),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceEventData {
    Started {
        at: TimestampData,
        assessment: AssessmentData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer: Option<String>,
    },
    Observed {
        snapshot: SnapshotData,
    },
    Planned {
        plan: Box<PlanData>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer: Option<String>,
    },
    BeforeWrite {
        write: WriteData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer: Option<String>,
    },
    /// A prepared write that never reached the provider and could not resume.
    DiscardedUnsent {
        write: WriteData,
    },
    Readback {
        write: WriteData,
        snapshot: SnapshotData,
    },
    Progress {
        progress: Box<crate::ProgressDocument>,
    },
    Finished {
        report: Box<RunDocument>,
    },
}

impl From<ExecutionEvent<'_>> for EvidenceEventData {
    fn from(value: ExecutionEvent<'_>) -> Self {
        match value {
            ExecutionEvent::Started { at, assessment } => Self::Started {
                at: at.into(),
                assessment: assessment.into(),
                renderer: None,
            },
            ExecutionEvent::Observed(snapshot) => Self::Observed {
                snapshot: snapshot.into(),
            },
            ExecutionEvent::Planned(plan) => Self::Planned {
                plan: Box::new(plan.into()),
                renderer: None,
            },
            ExecutionEvent::BeforeWrite(write) => Self::BeforeWrite {
                write: write.into(),
                renderer: None,
            },
            ExecutionEvent::DiscardedUnsent(write) => Self::DiscardedUnsent {
                write: write.into(),
            },
            ExecutionEvent::Readback { write, snapshot } => Self::Readback {
                write: write.into(),
                snapshot: snapshot.into(),
            },
            ExecutionEvent::Progress {
                subject,
                reviewer,
                progress,
            } => Self::Progress {
                progress: Box::new(crate::ProgressDocument::new(subject, reviewer, progress)),
            },
            ExecutionEvent::Finished(report) => Self::Finished {
                report: Box::new(report.into()),
            },
        }
    }
}

impl EvidenceEventData {
    /// Associates host-known identity with rendered payloads in this event only.
    pub fn with_renderer(mut self, identity: &str) -> Self {
        match &mut self {
            Self::Started { renderer, .. }
            | Self::Planned { renderer, .. }
            | Self::BeforeWrite { renderer, .. } => *renderer = Some(identity.to_owned()),
            Self::Finished { report } => report.renderer = Some(identity.to_owned()),
            Self::Observed { .. }
            | Self::DiscardedUnsent { .. }
            | Self::Readback { .. }
            | Self::Progress { .. } => {}
        }
        self
    }
}
