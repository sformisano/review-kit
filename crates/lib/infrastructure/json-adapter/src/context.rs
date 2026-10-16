use crate::observation::json_error;
use review_kit_observation::{Completion, Fields};

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::{Effects, FindingRecord, Snapshot, SummaryRecord},
    use_cases::discovery_bundle::{
        ChangedFile, DiscoveryBundle, ObservedChanges, ReviewChanges, discover_bundle,
    },
};
use review_kit_domain::{ReviewerId, Revision, SourcePath, Subject};
use serde::{Deserialize, Serialize};

use crate::{
    JsonError,
    domain::{FindingData, RevisionData, SubjectData, valid},
    rounds::{EffectData, ReplyData, RoundData},
    validate_version,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryData {
    pub id: String,
    pub author: String,
    pub body: String,
    pub current: RoundData,
    pub history: Vec<RoundData>,
}

#[review_kit_observation::operations]
impl From<&SummaryRecord> for SummaryData {
    #[helper]
    fn from(value: &SummaryRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            author: value.author.as_str().into(),
            body: value.body.clone(),
            current: (&value.current).into(),
            history: value.history.iter().map(RoundData::from).collect(),
        }
    }
}

#[review_kit_observation::operations]
impl SummaryData {
    #[helper]
    pub fn into_summary(self) -> Result<SummaryRecord, JsonError> {
        use review_kit_application::publication::ProviderId;
        Ok(SummaryRecord {
            id: valid(ProviderId::new(self.id))?,
            author: valid(ReviewerId::try_from(self.author))?,
            body: self.body,
            current: self.current.into_round()?,
            history: self
                .history
                .into_iter()
                .map(RoundData::into_round)
                .collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingRecordData {
    pub id: String,
    pub root_note: String,
    pub author: String,
    pub created_in: u64,
    pub revision: RevisionData,
    pub finding: FindingData,
    pub body: String,
    pub resolved: bool,
}

#[review_kit_observation::operations]
impl From<&FindingRecord> for FindingRecordData {
    #[helper]
    fn from(value: &FindingRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            root_note: value.root_note.as_str().into(),
            author: value.author.as_str().into(),
            created_in: value.created_in,
            revision: (&value.revision).into(),
            finding: (&value.finding).into(),
            body: value.body.clone(),
            resolved: value.resolved,
        }
    }
}

#[review_kit_observation::operations]
impl FindingRecordData {
    #[helper]
    pub fn into_record(self) -> Result<FindingRecord, JsonError> {
        use review_kit_application::publication::ProviderId;
        Ok(FindingRecord {
            id: valid(ProviderId::new(self.id))?,
            root_note: valid(ProviderId::new(self.root_note))?,
            author: valid(ReviewerId::try_from(self.author))?,
            created_in: self.created_in,
            revision: self.revision.into_revision()?,
            finding: self.finding.into_finding()?,
            body: self.body,
            resolved: self.resolved,
        })
    }
}

#[review_kit_observation::operations]
#[helper]
pub fn effect_data(effects: &Effects) -> Vec<EffectData> {
    use review_kit_application::publication::Effect;
    [
        Effect::CreateSummary,
        Effect::UpdateSummary,
        Effect::Finding,
        Effect::Reply,
        Effect::Resolve,
        Effect::Reopen,
    ]
    .into_iter()
    .filter(|effect| effects.contains(*effect))
    .map(EffectData::from)
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotData {
    pub subject: SubjectData,
    pub reviewer: String,
    pub revision: RevisionData,
    pub summaries: Vec<SummaryData>,
    pub findings: Vec<FindingRecordData>,
    pub replies: Vec<ReplyData>,
    pub permissions: Vec<EffectData>,
}

#[review_kit_observation::operations]
impl From<&Snapshot> for SnapshotData {
    #[helper]
    fn from(value: &Snapshot) -> Self {
        Self {
            subject: (&value.subject).into(),
            reviewer: value.reviewer.as_str().into(),
            revision: (&value.revision).into(),
            summaries: value.summaries.iter().map(SummaryData::from).collect(),
            findings: value.findings.iter().map(FindingRecordData::from).collect(),
            replies: value.replies.iter().map(ReplyData::from).collect(),
            permissions: effect_data(&value.permissions),
        }
    }
}

#[review_kit_observation::operations]
impl SnapshotData {
    #[helper]
    pub fn into_snapshot(self) -> Result<Snapshot, JsonError> {
        Ok(Snapshot {
            subject: self.subject.into_subject()?,
            reviewer: valid(ReviewerId::try_from(self.reviewer))?,
            revision: self.revision.into_revision()?,
            summaries: self
                .summaries
                .into_iter()
                .map(SummaryData::into_summary)
                .collect::<Result<_, _>>()?,
            findings: self
                .findings
                .into_iter()
                .map(FindingRecordData::into_record)
                .collect::<Result<_, _>>()?,
            replies: self
                .replies
                .into_iter()
                .map(ReplyData::into_reply)
                .collect::<Result<_, _>>()?,
            permissions: Effects::new(self.permissions.into_iter().map(Into::into)),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChangedFileData {
    Added { path: String },
    Modified { path: String },
    Deleted { path: String },
    Renamed { previous: String, current: String },
}

#[review_kit_observation::operations]
impl From<&ChangedFile> for ChangedFileData {
    #[helper]
    fn from(value: &ChangedFile) -> Self {
        match value {
            ChangedFile::Added(path) => Self::Added {
                path: path.as_str().into(),
            },
            ChangedFile::Modified(path) => Self::Modified {
                path: path.as_str().into(),
            },
            ChangedFile::Deleted(path) => Self::Deleted {
                path: path.as_str().into(),
            },
            ChangedFile::Renamed { previous, current } => Self::Renamed {
                previous: previous.as_str().into(),
                current: current.as_str().into(),
            },
        }
    }
}

#[review_kit_observation::operations]
impl ChangedFileData {
    #[helper]
    pub fn into_changed_file(self) -> Result<ChangedFile, JsonError> {
        Ok(match self {
            Self::Added { path } => ChangedFile::Added(valid(SourcePath::try_from(path))?),
            Self::Modified { path } => ChangedFile::Modified(valid(SourcePath::try_from(path))?),
            Self::Deleted { path } => ChangedFile::Deleted(valid(SourcePath::try_from(path))?),
            Self::Renamed { previous, current } => ChangedFile::Renamed {
                previous: valid(SourcePath::try_from(previous))?,
                current: valid(SourcePath::try_from(current))?,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangesData {
    pub subject: SubjectData,
    pub revision: RevisionData,
    pub files: Vec<ChangedFileData>,
    pub unified_diff: String,
}

#[review_kit_observation::operations]
impl From<&ObservedChanges> for ChangesData {
    #[helper]
    fn from(value: &ObservedChanges) -> Self {
        Self {
            subject: (&value.subject).into(),
            revision: (&value.revision).into(),
            files: value.files.iter().map(ChangedFileData::from).collect(),
            unified_diff: value.unified_diff.clone(),
        }
    }
}

#[review_kit_observation::operations]
impl ChangesData {
    #[helper]
    pub fn into_changes(self) -> Result<ObservedChanges, JsonError> {
        Ok(ObservedChanges {
            subject: self.subject.into_subject()?,
            revision: self.revision.into_revision()?,
            files: self
                .files
                .into_iter()
                .map(ChangedFileData::into_changed_file)
                .collect::<Result<_, _>>()?,
            unified_diff: self.unified_diff,
        })
    }
}

document_kind!(ContextKind, Context, "context");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDocument {
    pub version: u64,
    pub kind: ContextKind,
    pub snapshot: SnapshotData,
    pub changes: ChangesData,
}

#[review_kit_observation::operations]
impl ContextDocument {
    #[helper]
    pub fn new(snapshot: &Snapshot, changes: &ObservedChanges) -> Self {
        Self {
            version: 1,
            kind: ContextKind::Context,
            snapshot: snapshot.into(),
            changes: changes.into(),
        }
    }
    #[helper]
    pub fn validate(&self) -> Result<(), JsonError> {
        self.bundle().map(|_| ())
    }
    #[operation(
        name = "json.context_bundle",
        input = Fields::new()
        .count("files", self.changes.files.len())
        .count("findings", self.snapshot.findings.len())
        .count("summaries", self.snapshot.summaries.len()),
        output = |result| Completion::result(result, |_| Fields::new(), json_error),
    )]
    pub fn bundle(&self) -> Result<DiscoveryBundle, JsonError> {
        validate_version(self.version)?;
        let snapshot = self.snapshot.clone().into_snapshot()?;
        let changes = self.changes.clone().into_changes()?;
        let subject = snapshot.subject.clone();
        let reviewer = snapshot.reviewer.clone();
        discover_bundle(
            &mut ContextReader(snapshot),
            &mut ContextChanges(changes),
            &subject,
            &reviewer,
        )
        .map_err(JsonError::Discovery)
    }
}

struct ContextReader(Snapshot);
#[review_kit_observation::operations]
impl ReviewReader for ContextReader {
    #[helper]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
        if subject != &self.0.subject {
            return Err(PortError::new(
                "read retained context",
                JsonError::ContextBinding,
            ));
        }
        Ok(self.0.clone())
    }
}
struct ContextChanges(ObservedChanges);
#[review_kit_observation::operations]
impl ReviewChanges for ContextChanges {
    #[helper]
    fn read_changes(
        &mut self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<ObservedChanges, PortError> {
        if subject != &self.0.subject || revision != &self.0.revision {
            return Err(PortError::new(
                "read retained changes",
                JsonError::ContextBinding,
            ));
        }
        Ok(self.0.clone())
    }
}
