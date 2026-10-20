//! GitLab REST ports for authenticated review discovery and bounded publication.
//! Calls block; asynchronous hosts schedule the application use case on a worker.
#![doc = include_str!("../README.md")]
use review_kit_observation::Fields;

mod body;
mod config;
mod conversation;
mod discovery;
mod error;
mod http;
mod identity;
mod observation;
mod plan_validation;
mod publication;
mod source;
mod wire;

pub use config::{Credential, GitLabConfig, Limits};
pub use conversation::{BOT_LOOKUP_LIMIT, BotLookup};
pub use discovery::ManagedRoot;
pub use error::{FindingConflict, GitLabError};
pub use identity::{DiscussionId, MergeRequestIid, NoteId, ProjectId, UserId};
pub use plan_validation::validate_plan_bodies;
pub use publication::GitLabHost;
pub use source::RetainedSource;

use http::Api;
use identity::{Binding, MergeRequest};
use review_kit_application::publication::Effects;
use review_kit_domain::{ReviewerId, Subject};

/// A credential and canonical merge-request binding shared by read-only ports.
#[derive(Clone)]
pub struct GitLab {
    pub(crate) api: Api,
    pub(crate) binding: Binding,
}
#[review_kit_observation::operations]
impl GitLab {
    #[operation(
        name = "gitlab.connect",
        input = Fields::new().secret("merge_request", merge_request_url),
        output = |result| observation::result(result, |_| Fields::new().flag("bound", true)),
    )]
    pub fn connect(config: GitLabConfig, merge_request_url: &str) -> Result<Self, GitLabError> {
        let api = Api::new(config)?;
        let binding = identity::bind(&api, merge_request_url)?;
        Ok(Self { api, binding })
    }
    #[helper]
    pub fn subject(&self) -> &Subject {
        &self.binding.subject
    }
    #[helper]
    pub fn reviewer(&self) -> &ReviewerId {
        &self.binding.reviewer
    }
    #[helper]
    pub fn publication_host(&self, authority: Effects) -> GitLabHost {
        GitLabHost::new(self.clone(), authority)
    }
    #[helper]
    pub(crate) fn check_subject(&self, subject: &Subject) -> Result<(), GitLabError> {
        if subject != &self.binding.subject {
            Err(GitLabError::Identity)
        } else {
            Ok(())
        }
    }
    #[helper]
    pub(crate) fn mr_path(&self) -> String {
        format!(
            "projects/{}/merge_requests/{}",
            self.binding.target_project.get(),
            self.binding.iid.get()
        )
    }
    #[operation(
        name = "gitlab.refresh",
        input = Fields::new(),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    pub(crate) fn refresh(&self) -> Result<MergeRequest, GitLabError> {
        let mr = self.api.get(&self.mr_path(), &[])?;
        identity::corroborate(&self.api, &self.binding, &mr)?;
        Ok(mr)
    }
    #[helper]
    pub(crate) fn provider_permissions(&self, mr: &MergeRequest) -> Effects {
        // A readable object provides no complete token-capability probe. These
        // are supported effects; the server can deny any actual operation.
        if mr.discussion_locked != Some(true) {
            Effects::review()
        } else {
            Effects::none()
        }
    }
}
