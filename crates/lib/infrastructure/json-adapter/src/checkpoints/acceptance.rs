use review_kit_application::review_run::{ContentRef, RunId};
use serde::{Deserialize, Serialize};

use super::wire::{InvalidDocument, content_ref, run_id};

const MANIFEST: &str = "review-run.json";

/// Whether acceptance stored new bytes or found them already accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceStatus {
    Stored,
    Existing,
}

/// The stdout report of a successful acceptance.
///
/// It describes the manifest update; it is not itself an authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AcceptanceWire", into = "AcceptanceWire")]
pub struct AcceptanceResultDocument {
    pub run_id: RunId,
    pub status: AcceptanceStatus,
    pub reference: ContentRef,
}

impl AcceptanceResultDocument {
    /// The snapshot path relative to the run directory: `accepted/<hex>/<file>`.
    pub fn path(&self) -> String {
        format!("accepted/{}/review-assessment.json", self.reference.hex())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceWire {
    version: u64,
    run_id: String,
    status: AcceptanceStatus,
    reference: String,
    path: String,
    manifest: String,
}

impl TryFrom<AcceptanceWire> for AcceptanceResultDocument {
    type Error = InvalidDocument;

    fn try_from(wire: AcceptanceWire) -> Result<Self, Self::Error> {
        if wire.version != 1 {
            return Err(InvalidDocument::Version(wire.version));
        }
        if wire.manifest != MANIFEST {
            return Err(InvalidDocument::Manifest);
        }
        let document = Self {
            run_id: run_id(wire.run_id)?,
            status: wire.status,
            reference: content_ref(wire.reference)?,
        };
        if wire.path != document.path() {
            return Err(InvalidDocument::Path);
        }
        Ok(document)
    }
}

impl From<AcceptanceResultDocument> for AcceptanceWire {
    fn from(document: AcceptanceResultDocument) -> Self {
        Self {
            version: 1,
            run_id: document.run_id.as_str().into(),
            status: document.status,
            path: document.path(),
            reference: document.reference.as_str().into(),
            manifest: MANIFEST.into(),
        }
    }
}
