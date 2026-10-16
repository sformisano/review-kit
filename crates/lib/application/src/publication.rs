use std::collections::BTreeSet;

pub use crate::policy_declarations::{
    InvalidPolicyDeclarations, PublicPolicyDeclaration, validate_policy_declarations,
};

use review_kit_domain::{
    Assessment, Finding, FindingId, FindingStatus, PolicyId, ReviewerId, Revision, Subject,
};

/// Provider identifiers are opaque and are never interpreted as authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(value: String) -> Result<Self, InvalidIdentity> {
        if value.trim().is_empty() {
            return Err(InvalidIdentity);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// identity must not be blank
#[derive(Debug, Clone, Copy, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub struct InvalidIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    CreateSummary,
    UpdateSummary,
    Finding,
    Reply,
    Resolve,
    Reopen,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Effects(BTreeSet<Effect>);
impl Effects {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn review() -> Self {
        Self::new([
            Effect::CreateSummary,
            Effect::UpdateSummary,
            Effect::Finding,
            Effect::Reply,
            Effect::Resolve,
            Effect::Reopen,
        ])
    }
    pub fn new(effects: impl IntoIterator<Item = Effect>) -> Self {
        Self(effects.into_iter().collect())
    }
    pub fn contains(&self, effect: Effect) -> bool {
        self.0.contains(&effect)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn intersection(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundStatus {
    Pending,
    Partial,
    Completed,
    Blocked,
    Superseded,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Abandonment {
    pub reason: String,
    pub verified_actions: Vec<PlannedAction>,
}

/// Public failure categories contain no transport error text or private paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationLimitation {
    Authority(Effect),
    RevisionChanged,
    ConcurrentChange,
    ProviderUnavailable,
    Rejected,
    Uncertain,
    InvalidRendering,
    PreparationFailed,
    EvidenceUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    pub number: u64,
    pub assessment: Assessment,
    pub status: RoundStatus,
    pub abandonment: Option<Abandonment>,
    pub publication_limitation: Option<PublicationLimitation>,
    pub answers: Vec<Answer>,
    pub transitions: Vec<TransitionReply>,
    /// Exact input comments answered by this attempt, including their API authors.
    pub answer_targets: Vec<ReplyRecord>,
    /// The full intended effect set is retained even after effects are verified.
    pub actions: Vec<PlannedAction>,
    pub review_workflow: Option<String>,
    pub policy_set: Option<String>,
    pub policy_names: Vec<PolicyName>,
    pub policy_declarations: Vec<PublicPolicyDeclaration>,
    pub renderer: Option<crate::ports::RenderProfileIdentity>,
    /// None means this record lacks retained historical publication evidence.
    pub communicated_findings: Option<Vec<crate::summary::HistoricalFinding>>,
    pub verified_resolutions: Vec<ResolutionReceipt>,
    /// Whether acceptance validated this round's open or uncertain blocking findings
    /// only from recorded independent confirmations.
    pub confirmed_validation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionReceipt {
    pub subject: Subject,
    pub reviewer: ReviewerId,
    pub round: u64,
    pub thread: ProviderId,
    pub resolved: bool,
    pub provenance: ResolutionProvenance,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResolutionProvenance {
    #[default]
    Acknowledged,
    Observed,
}

impl Round {
    pub(crate) fn plans_resolution(&self, finding: FindingId, resolved: bool) -> bool {
        self.actions.iter().any(|action| match action {
            PlannedAction::SetResolved {
                finding: intended,
                resolved: state,
            } => *intended == finding && *state == resolved,
            _ => false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyName {
    pub id: PolicyId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRecord {
    /// Provider note identity used to update the overview summary.
    pub id: ProviderId,
    pub author: ReviewerId,
    pub body: String,
    pub current: Round,
    pub history: Vec<Round>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRecord {
    /// Provider discussion identity used for replies and resolution.
    pub id: ProviderId,
    /// Root note identity used for finding references in the public summary.
    pub root_note: ProviderId,
    pub author: ReviewerId,
    pub created_in: u64,
    pub revision: Revision,
    pub finding: Finding,
    pub body: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyKind {
    Transition(FindingStatus),
    Answer(ProviderId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionId {
    pub round: u64,
    pub finding: FindingId,
    pub kind: ReplyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyRecord {
    /// Provider note identity; distinct from the enclosing discussion identity.
    pub id: ProviderId,
    pub author: ReviewerId,
    pub thread: ProviderId,
    /// Ordinary discussion replies have no review action marker.
    pub action: Option<ActionId>,
    pub body: String,
}

/// A complete, paginated observation of a single subject, including resolved
/// discussions and API authors. Adapters reject malformed reserved records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub subject: Subject,
    /// Authenticated identity supplied by the reader, independent of marker text.
    pub reviewer: ReviewerId,
    pub revision: Revision,
    pub summaries: Vec<SummaryRecord>,
    pub findings: Vec<FindingRecord>,
    pub replies: Vec<ReplyRecord>,
    pub permissions: Effects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub finding: FindingId,
    pub in_reply_to: ProviderId,
    pub body: String,
}

/// The agent explains a status change or reclassification of an existing finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionReply {
    pub finding: FindingId,
    pub from: FindingStatus,
    pub to: FindingStatus,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Summary {
        expected: Option<Box<SummaryRecord>>,
        round: Box<Round>,
        history: Vec<Round>,
        body: String,
    },
    Finding {
        round: u64,
        revision: Revision,
        finding: Box<Finding>,
        body: String,
    },
    Reply {
        thread: ProviderId,
        action: ActionId,
        body: String,
    },
    SetResolved {
        thread: ProviderId,
        resolved: bool,
    },
}

impl Write {
    pub fn effect(&self) -> Effect {
        match self {
            Self::Summary { expected: None, .. } => Effect::CreateSummary,
            Self::Summary {
                expected: Some(_), ..
            } => Effect::UpdateSummary,
            Self::Finding { .. } => Effect::Finding,
            Self::Reply { .. } => Effect::Reply,
            Self::SetResolved { resolved: true, .. } => Effect::Resolve,
            Self::SetResolved {
                resolved: false, ..
            } => Effect::Reopen,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    Finding {
        finding: Box<Finding>,
        body: String,
    },
    Reply {
        finding: FindingId,
        kind: ReplyKind,
        body: String,
    },
    SetResolved {
        finding: FindingId,
        resolved: bool,
    },
}

impl PlannedAction {
    pub fn effect(&self) -> Effect {
        match self {
            Self::Finding { .. } => Effect::Finding,
            Self::Reply { .. } => Effect::Reply,
            Self::SetResolved { resolved: true, .. } => Effect::Resolve,
            Self::SetResolved {
                resolved: false, ..
            } => Effect::Reopen,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub round: Round,
    pub history: Vec<Round>,
    pub actions: Vec<PlannedAction>,
    pub unchanged: bool,
    pub summary_body: String,
    pub terminal_summary_preview: String,
    /// Final provider links are absent until these roots have verified IDs.
    pub unresolved_findings: Vec<FindingId>,
}
