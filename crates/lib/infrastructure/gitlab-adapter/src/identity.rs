use percent_encoding::percent_decode_str;
use review_kit_domain::{ReviewerId, Revision, SourcePath, Subject};
use serde::Deserialize;
use url::Url;

use crate::{GitLabError, config::clean_url, http::Api};

macro_rules! numeric_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name(u64);
        #[review_kit_observation::operations]
        impl $name {
            #[helper]
            pub fn new(value: u64) -> Result<Self, GitLabError> {
                if value == 0 {
                    Err(GitLabError::Identity)
                } else {
                    Ok(Self(value))
                }
            }
            #[helper]
            pub fn get(self) -> u64 {
                self.0
            }
        }
    };
}
numeric_identity!(ProjectId);
numeric_identity!(MergeRequestIid);
numeric_identity!(UserId);
numeric_identity!(NoteId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscussionId(String);
#[review_kit_observation::operations]
impl DiscussionId {
    #[helper]
    pub fn new(value: String) -> Result<Self, GitLabError> {
        if value.is_empty() || value.chars().any(char::is_control) {
            Err(GitLabError::Identity)
        } else {
            Ok(Self(value))
        }
    }
    #[helper]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
pub(crate) struct Binding {
    pub(crate) subject: Subject,
    pub(crate) target_project: ProjectId,
    pub(crate) source_project: ProjectId,
    pub(crate) iid: MergeRequestIid,
    pub(crate) mr_id: u64,
    pub(crate) reviewer: ReviewerId,
}

#[derive(Deserialize)]
pub(crate) struct MergeRequest {
    pub(crate) id: u64,
    pub(crate) iid: u64,
    pub(crate) project_id: u64,
    pub(crate) target_project_id: u64,
    pub(crate) source_project_id: Option<u64>,
    pub(crate) web_url: String,
    pub(crate) diff_refs: Option<DiffRefs>,
    #[serde(deserialize_with = "required_nullable_bool")]
    pub(crate) discussion_locked: Option<bool>,
    // Only conversation capture requires these; discovery tolerates their absence.
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    pub(crate) description: Option<Option<String>>,
}
#[review_kit_observation::operations]
#[helper]
fn required_nullable_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    Option::<bool>::deserialize(deserializer)
}
/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
#[review_kit_observation::operations]
#[helper]
fn present_nullable_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
pub(crate) struct DiffRefs {
    pub(crate) base_sha: String,
    pub(crate) start_sha: String,
    pub(crate) head_sha: String,
}
#[review_kit_observation::operations]
impl MergeRequest {
    #[helper]
    pub(crate) fn revision(&self) -> Result<Revision, GitLabError> {
        let refs = self.diff_refs.as_ref().ok_or(GitLabError::Protocol)?;
        for sha in [&refs.head_sha, &refs.base_sha, &refs.start_sha] {
            validate_sha(sha)?;
        }
        Revision::new(
            refs.head_sha.clone(),
            vec![refs.base_sha.clone(), refs.start_sha.clone()],
        )
        .map_err(|_| GitLabError::Protocol)
    }
    /// Title and description; a `null` description is empty text.
    #[helper]
    pub(crate) fn text(&self) -> Result<(String, String), GitLabError> {
        let title = self.title.clone().ok_or(GitLabError::Protocol)?;
        let description = self
            .description
            .clone()
            .ok_or(GitLabError::Protocol)?
            .unwrap_or_default();
        Ok((title, description))
    }
}
#[review_kit_observation::operations]
#[helper]
pub(crate) fn validate_sha(sha: &str) -> Result<(), GitLabError> {
    if !matches!(sha.len(), 40 | 64)
        || !sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(GitLabError::Identity);
    }
    Ok(())
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn parse_subject(server: &Url, value: &str) -> Result<Subject, GitLabError> {
    let url = Url::parse(value).map_err(|_| GitLabError::Identity)?;
    clean_url(&url).map_err(|_| GitLabError::Identity)?;
    if url.origin() != server.origin() {
        return Err(GitLabError::Identity);
    }
    let tail = url
        .path()
        .strip_prefix(server.path())
        .ok_or(GitLabError::Identity)?;
    let (repository, number) = tail
        .rsplit_once("/-/merge_requests/")
        .or_else(|| tail.rsplit_once("/merge_requests/"))
        .ok_or(GitLabError::Identity)?;
    if number.contains('/') || number.starts_with('0') {
        return Err(GitLabError::Identity);
    }
    let repository = percent_decode_str(repository)
        .decode_utf8()
        .map_err(|_| GitLabError::Identity)?
        .into_owned();
    SourcePath::try_from(repository.clone()).map_err(|_| GitLabError::Identity)?;
    if repository.split('/').count() < 2 {
        return Err(GitLabError::Identity);
    }
    Subject::new(
        "gitlab".into(),
        server.as_str().trim_end_matches('/').into(),
        repository,
        number.parse().map_err(|_| GitLabError::Identity)?,
    )
    .map_err(|_| GitLabError::Identity)
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn bind(api: &Api, url: &str) -> Result<Binding, GitLabError> {
    let subject = parse_subject(&api.server, url)?;
    #[derive(Deserialize)]
    struct User {
        id: u64,
    }
    let actor: User = api.get("user", &[])?;
    let actor = UserId::new(actor.id)?;
    #[derive(Deserialize)]
    struct Project {
        id: u64,
        path_with_namespace: String,
        web_url: String,
    }
    let project: Project = api.get(
        &format!("projects/{}", crate::http::segment(subject.repository())),
        &[],
    )?;
    let target_project = ProjectId::new(project.id)?;
    if project.path_with_namespace != subject.repository() {
        return Err(GitLabError::Identity);
    }
    let projected = parse_subject(
        &api.server,
        &format!("{}/-/merge_requests/{}", project.web_url, subject.request()),
    )?;
    if projected != subject {
        return Err(GitLabError::Identity);
    }
    let iid = MergeRequestIid::new(subject.request())?;
    let mr: MergeRequest = api.get(
        &format!(
            "projects/{}/merge_requests/{}",
            target_project.get(),
            iid.get()
        ),
        &[],
    )?;
    let binding = Binding {
        subject,
        target_project,
        source_project: ProjectId::new(
            mr.source_project_id.ok_or(GitLabError::UnsupportedSource)?,
        )?,
        iid,
        mr_id: mr.id,
        reviewer: ReviewerId::try_from(actor.get().to_string())
            .map_err(|_| GitLabError::Identity)?,
    };
    corroborate(api, &binding, &mr)?;
    mr.revision()?;
    Ok(binding)
}
#[review_kit_observation::operations]
#[helper]
pub(crate) fn corroborate(
    api: &Api,
    binding: &Binding,
    mr: &MergeRequest,
) -> Result<(), GitLabError> {
    if mr.id == 0
        || mr.id != binding.mr_id
        || mr.iid != binding.iid.get()
        || mr.project_id != binding.target_project.get()
        || mr.target_project_id != binding.target_project.get()
        || mr.source_project_id != Some(binding.source_project.get())
        || parse_subject(&api.server, &mr.web_url)? != binding.subject
    {
        return Err(GitLabError::Identity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_url_preserves_nested_names_and_configured_deployment_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = Url::parse("https://gitlab.example/gitlab/")?;
        let subject = parse_subject(
            &server,
            "https://gitlab.example/gitlab/group/sub/project/-/merge_requests/7",
        )?;
        assert_eq!(subject.host(), "https://gitlab.example/gitlab");
        assert_eq!(subject.repository(), "group/sub/project");
        assert_eq!(subject.request(), 7);
        for url in [
            "https://other.example/gitlab/group/project/-/merge_requests/7",
            "https://gitlab.example/group/project/-/merge_requests/7",
            "https://gitlab.example/gitlab/group/project/-/merge_requests/0",
            "https://secret@gitlab.example/gitlab/group/project/-/merge_requests/7",
            "https://gitlab.example/gitlab/group/project/-/merge_requests/7?token=secret",
        ] {
            assert!(parse_subject(&server, url).is_err());
        }
        Ok(())
    }
}
