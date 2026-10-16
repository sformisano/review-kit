use crate::observation::{file_error, port_error};
use review_kit_observation::{Completion, Fields};

use std::path::{Path, PathBuf};

use review_kit_application::ports::{PortError, RenderProfileIdentity};
use review_kit_application::use_cases::{
    publication_run::{ExecutionEvent, ExecutionEvidence},
    publishing::{PublicationProgress, UnresolvedWrite},
};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_json_adapter::{
    EvidenceDocument, EvidenceEventData, EvidenceKind, validate_version,
};

use crate::{FileError, FileProgressStore, RunWriter, WorkingFile, Workspace};

const LEGACY_EVENTS: &str = "evidence.json";

/// Where the events document lives and where progress checkpoints go.
enum Location<'run> {
    /// A workspace evidence file; progress uses the generic store or the
    /// workspace's own `progress.json`.
    Workspace {
        workspace: Workspace,
        events: PathBuf,
        progress: Option<FileProgressStore>,
    },
    /// A review run's `review-publication-events.json`, read and replaced through
    /// run storage; progress goes to the run-bound journal.
    Run {
        writer: RunWriter<'run>,
        journal: FileProgressStore,
    },
}

pub struct FileEvidence<'run> {
    location: Location<'run>,
    subject: Subject,
    reviewer: ReviewerId,
    document: EvidenceDocument,
    renderer: Option<String>,
    profile: Option<RenderProfileIdentity>,
}

#[review_kit_observation::operations]
impl FileEvidence<'static> {
    #[helper]
    pub fn new(
        workspace: &Workspace,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Self, FileError> {
        Self::in_workspace(workspace, None, subject, reviewer)
    }

    /// Keep evidence local and checkpoint generic progress into `store`.
    #[helper]
    pub fn with_progress_store(
        workspace: &Workspace,
        store: &FileProgressStore,
    ) -> Result<Self, FileError> {
        Self::in_workspace(workspace, Some(store), &store.subject, &store.reviewer)
    }

    #[helper]
    fn in_workspace(
        workspace: &Workspace,
        progress: Option<&FileProgressStore>,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Self, FileError> {
        let events = Path::new(LEGACY_EVENTS);
        workspace.path(events)?;
        let stored = if workspace.exists(events)? {
            Some(workspace.read_json(events)?)
        } else {
            None
        };
        Self::load(
            Location::Workspace {
                workspace: workspace.clone(),
                events: events.to_owned(),
                progress: progress.cloned(),
            },
            stored,
            subject,
            reviewer,
        )
    }
}

#[review_kit_observation::operations]
impl<'run> FileEvidence<'run> {
    /// Evidence in the run's `review-publication-events.json`, checkpointing
    /// progress into `journal`'s run-bound `publication-progress.json`.
    ///
    /// The events file is read and replaced only through `writer`, so a symbolic
    /// link or a non-file entry at that name is `FileError::RunStorage` and
    /// nothing is read or written through it. The file follows the same event
    /// contract and subject binding as `evidence.json`.
    #[helper]
    pub fn for_run(
        writer: RunWriter<'run>,
        journal: &FileProgressStore,
    ) -> Result<Self, FileError> {
        let stored = writer
            .read_optional_bytes(WorkingFile::PublicationEvents.into())?
            .map(|bytes| review_kit_json_adapter::decode::<EvidenceDocument>(&bytes))
            .transpose()?;
        Self::load(
            Location::Run {
                writer,
                journal: journal.clone(),
            },
            stored,
            &journal.subject,
            &journal.reviewer,
        )
    }

    #[operation(
        name = "filesystem.load_evidence",
        input = Fields::new(),
        output = |result| {
            Completion::result(
                result,
                |evidence| Fields::new().count("events", evidence.document.events.len()),
                file_error,
            )
        },
    )]
    fn load(
        location: Location<'run>,
        stored: Option<EvidenceDocument>,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Self, FileError> {
        let document = match stored {
            Some(document) => {
                validate_version(document.version)?;
                if document.subject.clone().into_subject()? != *subject
                    || document.reviewer != reviewer.as_str()
                {
                    return Err(FileError::EvidenceBinding);
                }
                document
            }
            None => EvidenceDocument {
                version: 1,
                kind: EvidenceKind::Evidence,
                subject: subject.into(),
                reviewer: reviewer.as_str().into(),
                events: Vec::new(),
            },
        };
        Ok(Self {
            location,
            subject: subject.clone(),
            reviewer: reviewer.clone(),
            document,
            renderer: None,
            profile: None,
        })
    }

    /// Records the selected profile on new rendered events, preserving prior identities.
    #[helper]
    pub fn with_renderer(mut self, identity: impl Into<String>) -> Self {
        self.renderer = Some(identity.into());
        self
    }

    /// Binds evidence and unresolved write intent to the same loaded profile.
    #[helper]
    pub fn with_profile(mut self, identity: &RenderProfileIdentity) -> Self {
        self.renderer = Some(
            serde_json::json!({
                "id": identity.id,
                "input_contract": identity.input_contract,
                "fingerprint": identity.fingerprint,
            })
            .to_string(),
        );
        self.profile = Some(identity.clone());
        self
    }

    /// The progress a new write intent extends.
    #[helper]
    fn load_checkpoint(&self) -> Result<PublicationProgress, FileError> {
        match &self.location {
            Location::Workspace {
                workspace,
                progress: Some(store),
                ..
            } => store.load(workspace),
            Location::Workspace {
                workspace,
                progress: None,
                ..
            } => workspace.load_progress(&self.subject, &self.reviewer),
            Location::Run { journal, .. } => journal.load_journal(),
        }
    }

    #[helper]
    fn save_checkpoint(&self, progress: &PublicationProgress) -> Result<(), FileError> {
        match &self.location {
            Location::Workspace {
                progress: Some(store),
                ..
            } => store.save(progress),
            Location::Workspace {
                workspace,
                progress: None,
                ..
            } => workspace.save_final_progress(&self.subject, &self.reviewer, progress),
            Location::Run { journal, .. } => journal.save_journal(progress),
        }
    }

    #[helper]
    fn save_document(&self) -> Result<(), FileError> {
        match &self.location {
            Location::Workspace {
                workspace, events, ..
            } => workspace.write_json(events, &self.document),
            Location::Run { writer, .. } => {
                Ok(writer.replace_json(WorkingFile::PublicationEvents, &self.document)?)
            }
        }
    }

    #[helper]
    fn shares_progress(&self) -> bool {
        !matches!(self.location, Location::Workspace { progress: None, .. })
    }
}

#[review_kit_observation::operations]
impl ExecutionEvidence for FileEvidence<'_> {
    #[operation(
        name = "filesystem.record_evidence",
        input = Fields::new()
        .label("event", event_kind(&event))
        .flag("shared_progress", self.shares_progress()),
        output = |result| Completion::result(result, |_| Fields::new(), port_error),
    )]
    fn record(&mut self, event: ExecutionEvent<'_>) -> Result<(), PortError> {
        let pending = match &event {
            ExecutionEvent::BeforeWrite(write) => {
                let progress = self
                    .load_checkpoint()
                    .map_err(|error| PortError::new("load publication intent", error));
                progress.map(|mut progress| {
                    if progress
                        .unresolved
                        .as_ref()
                        .is_none_or(|pending| pending.write != **write)
                    {
                        progress.unresolved = Some(UnresolvedWrite {
                            subject: self.subject.clone(),
                            reviewer: self.reviewer.clone(),
                            write: (*write).clone(),
                            renderer: self.profile.clone(),
                            phase: Default::default(),
                            baseline: None,
                        });
                    }
                    Some(progress)
                })
            }
            ExecutionEvent::Progress {
                subject,
                reviewer,
                progress,
            } => {
                if *subject != &self.subject || *reviewer != &self.reviewer {
                    return Err(PortError::new(
                        "retain publication progress",
                        FileError::EvidenceBinding,
                    ));
                }
                Ok(Some((*progress).clone()))
            }
            _ => Ok(None),
        };
        let mut event = EvidenceEventData::from(event);
        if let Some(renderer) = &self.renderer {
            event = event.with_renderer(renderer);
        }
        self.document.events.push(event);
        if let Err(error) = self.save_document() {
            self.document.events.pop();
            return Err(PortError::new("save execution evidence", error));
        }
        if let Some(progress) = pending? {
            self.save_checkpoint(&progress)
                .map_err(|error| PortError::new("retain publication intent", error))?;
        }
        Ok(())
    }
}

#[review_kit_observation::operations]
#[helper]
fn event_kind(event: &ExecutionEvent<'_>) -> &'static str {
    match event {
        ExecutionEvent::Started { .. } => "started",
        ExecutionEvent::Observed(_) => "observed",
        ExecutionEvent::Planned(_) => "planned",
        ExecutionEvent::BeforeWrite(_) => "before_write",
        ExecutionEvent::DiscardedUnsent(_) => "discarded_unsent",
        ExecutionEvent::Readback { .. } => "readback",
        ExecutionEvent::Progress { .. } => "progress",
        ExecutionEvent::Finished(_) => "finished",
    }
}
