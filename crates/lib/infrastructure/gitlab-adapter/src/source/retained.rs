use crate::observation;
use review_kit_application::ports::{CommittedSource, PortError};
use review_kit_domain::{Finding, Revision, SourcePath, Subject};
use review_kit_json_adapter::domain::{RevisionData, SubjectData};
use review_kit_observation::Fields;
use serde::{Deserialize, Serialize};

use super::{diff::DiffVersion, diff::regular_mode, diff::validate_revision, position, text_lines};
use crate::{GitLab, GitLabError};

/// Committed head-side text and complete diff evidence for offline source checks.
/// Decoding validates local structure; it does not establish current provider state.
#[derive(Debug)]
pub struct RetainedSource {
    subject: Subject,
    revision: Revision,
    data: SourceData,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceData {
    version: u64,
    subject: SubjectData,
    revision: RevisionData,
    diff: DiffVersion,
    blobs: Vec<BlobRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobRecord {
    path: String,
    source: Blob,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Blob {
    Text { text: String },
    Unsupported,
    Deleted,
}

#[review_kit_observation::operations]
impl RetainedSource {
    #[helper]
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    #[helper]
    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    #[operation(
        name = "gitlab.retained_source.encode",
        input = Fields::new(),
        output = |result| observation::result(result, |bytes| Fields::new().count("bytes", bytes.len())),
    )]
    pub fn encode(&self) -> Result<Vec<u8>, GitLabError> {
        serde_json::to_vec_pretty(&self.data).map_err(|_| GitLabError::State)
    }

    #[operation(
        name = "gitlab.retained_source.decode",
        input = Fields::new()
            .count("bytes", bytes.len())
            .secret("source", "retained content"),
        output = |result| {
            observation::result(result, |source| {
                Fields::new().count("files", source.data.blobs.len())
            })
        },
    )]
    pub fn decode(bytes: &[u8]) -> Result<Self, GitLabError> {
        let data = serde_json::from_slice(bytes).map_err(|_| GitLabError::State)?;
        Self::validated(data)
    }

    #[helper]
    fn validated(data: SourceData) -> Result<Self, GitLabError> {
        if data.version != 1 {
            return Err(GitLabError::State);
        }
        let subject = data
            .subject
            .clone()
            .into_subject()
            .map_err(|_| GitLabError::State)?;
        if subject.provider() != "gitlab" {
            return Err(GitLabError::State);
        }
        SourcePath::try_from(subject.repository().to_owned()).map_err(|_| GitLabError::State)?;
        let revision = data
            .revision
            .clone()
            .into_revision()
            .map_err(|_| GitLabError::State)?;
        validate_revision(&revision)?;
        if data.diff.version.id == 0 || data.diff.version.merge_request_id == 0 {
            return Err(GitLabError::State);
        }
        data.diff.validate(&data.diff.version, &revision)?;
        data.diff.unified_diff()?;
        if data.blobs.len() != data.diff.diffs.len() {
            return Err(GitLabError::State);
        }
        for (blob, file) in data.blobs.iter().zip(&data.diff.diffs) {
            if blob.path != file.new_path {
                return Err(GitLabError::State);
            }
            match &blob.source {
                Blob::Deleted if file.deleted_file => {}
                Blob::Unsupported if !file.deleted_file => {}
                Blob::Text { text } if !file.deleted_file && regular_mode(&file.b_mode) => {
                    text_lines(text.as_bytes().to_vec())?;
                }
                _ => return Err(GitLabError::State),
            }
        }
        Ok(Self {
            subject,
            revision,
            data,
        })
    }

    #[helper]
    fn bind(&self, subject: &Subject, revision: &Revision) -> Result<(), GitLabError> {
        if subject != &self.subject {
            return Err(GitLabError::Identity);
        }
        if revision != &self.revision {
            return Err(GitLabError::RevisionChanged);
        }
        Ok(())
    }

    #[helper]
    fn retained_lines(&self, path: &SourcePath) -> Result<Vec<String>, GitLabError> {
        let blob = self
            .data
            .blobs
            .iter()
            .find(|blob| blob.path == path.as_str())
            .ok_or(GitLabError::UnsupportedSource)?;
        match &blob.source {
            Blob::Text { text } => text_lines(text.as_bytes().to_vec()),
            Blob::Unsupported | Blob::Deleted => Err(GitLabError::UnsupportedSource),
        }
    }
}

#[review_kit_observation::operations]
impl GitLab {
    /// Capture complete diff evidence and changed committed head-side blobs.
    /// Unsupported blobs and deleted files remain explicit in the retained data.
    #[operation(
        name = "gitlab.capture_source",
        input = Fields::new(),
        output = |result| {
            observation::result(result, |source| {
                Fields::new().count("files", source.data.blobs.len())
            })
        },
    )]
    pub fn capture_source(
        &self,
        subject: &Subject,
        revision: &Revision,
    ) -> Result<RetainedSource, GitLabError> {
        self.check_subject(subject)?;
        self.require_revision(revision)?;
        let diff = self.diff_version(revision)?;
        let mut bytes = serde_json::to_vec(&diff)
            .map_err(|_| GitLabError::State)?
            .len();
        let mut blobs = Vec::with_capacity(diff.diffs.len());
        for file in &diff.diffs {
            let source = if file.deleted_file {
                Blob::Deleted
            } else if !regular_mode(&file.b_mode) {
                Blob::Unsupported
            } else {
                let path = SourcePath::try_from(file.new_path.clone())
                    .map_err(|_| GitLabError::UnsupportedSource)?;
                match self.committed_bytes(revision, &path) {
                    Ok(contents) => {
                        bytes = bytes
                            .checked_add(contents.len())
                            .ok_or(GitLabError::Limit)?;
                        if bytes > self.api.limits.total_bytes {
                            return Err(GitLabError::Limit);
                        }
                        if text_lines(contents.clone()).is_ok() {
                            Blob::Text {
                                text: String::from_utf8(contents)
                                    .map_err(|_| GitLabError::UnsupportedSource)?,
                            }
                        } else {
                            Blob::Unsupported
                        }
                    }
                    Err(GitLabError::UnsupportedSource) => Blob::Unsupported,
                    Err(error) => return Err(error),
                }
            };
            blobs.push(BlobRecord {
                path: file.new_path.clone(),
                source,
            });
        }
        if bytes > self.api.limits.total_bytes {
            return Err(GitLabError::Limit);
        }
        self.require_revision(revision)?;
        RetainedSource::validated(SourceData {
            version: 1,
            subject: subject.into(),
            revision: revision.into(),
            diff,
            blobs,
        })
    }
}

#[review_kit_observation::operations]
impl CommittedSource for RetainedSource {
    #[operation(
        name = "gitlab.retained_source.lines",
        input = Fields::new().secret("path", path.as_str()),
        output = |result| {
            observation::port_result(result, |lines| Fields::new().count("lines", lines.len()))
        },
    )]
    fn lines(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        self.bind(subject, revision)
            .and_then(|()| self.retained_lines(path))
            .map_err(|error| error.port("read retained GitLab source"))
    }

    #[operation(
        name = "gitlab.retained_source.validate_position",
        input = Fields::new(),
        output = |result| observation::port_result(result, |_| Fields::new()),
    )]
    fn validate_position(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<(), PortError> {
        let validate = || {
            self.bind(subject, revision)?;
            let file = self.data.diff.file(finding.location().path())?;
            let lines = self.retained_lines(finding.location().path())?;
            position::build(file, revision, finding, Some(&lines))?;
            Ok(())
        };
        validate()
            .map_err(|error: GitLabError| error.port("validate retained GitLab diff position"))
    }
}
