use review_kit_application::review_run::RunId;
use review_kit_domain::Subject;
use serde::{Deserialize, Serialize};

use super::wire::{InvalidDocument, nonblank, run_id};

checkpoint_kind!(WorkspaceKind, ReviewWorkspace, "review_workspace");

/// The repository a worktree's review runs belong to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryBinding {
    pub provider: String,
    pub host: String,
    pub repository: String,
}

impl RepositoryBinding {
    /// The repository part of a review subject.
    pub fn of(subject: &Subject) -> Self {
        Self {
            provider: subject.provider().into(),
            host: subject.host().into(),
            repository: subject.repository().into(),
        }
    }

    /// Whether the subject belongs to this repository; the request number is not compared.
    pub fn binds(&self, subject: &Subject) -> bool {
        self == &Self::of(subject)
    }
}

/// `review-workspace.json`: the repository binding and the selected run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WorkspaceWire", into = "WorkspaceWire")]
pub struct ReviewWorkspaceDocument {
    pub repository: RepositoryBinding,
    pub selected_run_id: RunId,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceWire {
    version: u64,
    kind: WorkspaceKind,
    repository: RepositoryBinding,
    selected_run_id: String,
}

impl TryFrom<WorkspaceWire> for ReviewWorkspaceDocument {
    type Error = InvalidDocument;

    fn try_from(wire: WorkspaceWire) -> Result<Self, Self::Error> {
        if wire.version != 1 {
            return Err(InvalidDocument::Version(wire.version));
        }
        let repository = wire.repository;
        if ![
            &repository.provider,
            &repository.host,
            &repository.repository,
        ]
        .into_iter()
        .all(|value| nonblank(value))
        {
            return Err(InvalidDocument::Repository);
        }
        Ok(Self {
            repository,
            selected_run_id: run_id(wire.selected_run_id)?,
        })
    }
}

impl From<ReviewWorkspaceDocument> for WorkspaceWire {
    fn from(document: ReviewWorkspaceDocument) -> Self {
        Self {
            version: 1,
            kind: WorkspaceKind::ReviewWorkspace,
            repository: document.repository,
            selected_run_id: document.selected_run_id.as_str().into(),
        }
    }
}
