use crate::observation as log;
use std::error::Error;

use review_kit_domain::{Finding, FindingStatus, Revision, SourcePath, Subject};

mod provider;
pub(crate) use provider::{ManagedHost, ManagedReader, ManagedSession};
pub use provider::{PublicationHost, PublicationSession, ReviewReader};

use crate::{
    publication::{Answer, FindingRecord, ReplyRecord, RoundStatus},
    summary::SummaryView,
};

/// {operation} failed
#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub struct PortError {
    operation: &'static str,
    #[source]
    source: Box<dyn Error + Send + Sync>,
}
#[review_kit_observation::operations]
impl PortError {
    #[helper]
    pub fn new(operation: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            operation,
            source: Box::new(source),
        }
    }
}

#[derive(Debug)]
pub enum WriteOutcome {
    Acknowledged,
    /// The effect is not submitted, or the provider rejects it without committing.
    Rejected(PortError),
    /// The request may have committed. The application does not blindly retry.
    Unknown(PortError),
}

#[review_kit_observation::observed_port(wrapper = ManagedSource, ownership = "borrowed", visibility = "pub(crate)")]
pub trait CommittedSource {
    /// Read a regular text blob at this exact immutable revision. No worktree fallback.
    #[operation(name = "application.source.lines", input = log::Fields::new()
            .object("subject", log::subject(subject))
            .secret("revision", revision.head())
            .secret("path", path.as_str()), output = |result| {
            log::Completion::result(
                result,
                |lines| log::Fields::new().count("lines", lines.len()),
                log::port_error,
            )
        })]
    fn lines(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError>;
    /// Verify that the anchor and the entire suggestion range are supported by
    /// the reviewed provider diff. This must not merely check file line bounds.
    #[operation(name = "application.source.validate_position", input = log::Fields::new()
            .object("subject", log::subject(subject))
            .secret("revision", revision.head())
            .secret("path", finding.location().path().as_str())
            .flag("suggestion", finding.suggestion().is_some()), output = |result| log::Completion::result(result, |_| log::Fields::new(), log::port_error))]
    fn validate_position(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<(), PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderProfileIdentity {
    pub id: String,
    pub input_contract: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRenderContext {
    pub round: u64,
    pub revision: Revision,
    pub publication_status: RoundStatus,
    pub review_workflow: Option<String>,
    pub policy_set: String,
    pub scope: String,
    pub depth: String,
}

#[derive(Debug, Clone, Copy)]
pub struct FindingRenderInput<'a> {
    pub finding: &'a Finding,
    pub policy_name: &'a str,
    pub context: &'a PublicRenderContext,
}

#[derive(Debug, Clone, Copy)]
pub struct TransitionRenderInput<'a> {
    pub finding: FindingRenderInput<'a>,
    pub previous: FindingStatus,
    pub explanation: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct AnswerRenderInput<'a> {
    pub answer: &'a Answer,
    pub finding: &'a FindingRecord,
    pub current_finding: &'a Finding,
    pub policy_name: &'a str,
    pub target: &'a ReplyRecord,
    pub context: &'a PublicRenderContext,
}

/// Rendering adapters validate their trusted profile and each projected input
/// against its schema. Markers and transport fields are attached by the provider
/// adapter.
#[review_kit_observation::observed_port(wrapper = ManagedRenderer, ownership = "borrowed", visibility = "pub(crate)")]
pub trait ReviewRenderer {
    #[helper]
    fn identity(&self) -> Option<&RenderProfileIdentity> {
        None
    }
    #[operation(name = "application.render.finding", input = log::Fields::new()
            .number("round", input.context.round)
            .flag("suggestion", input.finding.suggestion().is_some()), output = log::rendered)]
    fn finding_context(&mut self, input: FindingRenderInput<'_>) -> Result<String, PortError> {
        self.finding(input.finding)
    }
    #[operation(name = "application.render.transition", input = log::Fields::new()
            .number("round", input.finding.context.round)
            .secret("explanation", input.explanation), output = log::rendered)]
    fn transition_context(
        &mut self,
        input: TransitionRenderInput<'_>,
    ) -> Result<String, PortError> {
        self.transition(input.finding.finding, input.previous, input.explanation)
    }
    #[operation(name = "application.render.answer", input = log::Fields::new()
            .number("round", input.context.round)
            .secret("body", input.answer.body.as_str()), output = log::rendered)]
    fn answer_context(&mut self, input: AnswerRenderInput<'_>) -> Result<String, PortError> {
        self.answer(&input.answer.body)
    }
    #[operation(name = "application.render.finding", input = log::Fields::new().flag("suggestion", finding.suggestion().is_some()), output = log::rendered)]
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError>;
    #[operation(name = "application.render.transition", input = log::Fields::new().secret("explanation", explanation), output = log::rendered)]
    fn transition(
        &mut self,
        finding: &Finding,
        previous: FindingStatus,
        explanation: &str,
    ) -> Result<String, PortError>;
    #[operation(name = "application.render.answer", input = log::Fields::new().secret("body", body), output = log::rendered)]
    fn answer(&mut self, body: &str) -> Result<String, PortError>;
    /// Preflight validates the full summary shape even before links exist.
    /// Finding references use each root's note identity; its discussion identity
    /// identifies the target for replies and resolution.
    #[operation(name = "application.render.summary", input = log::Fields::new()
            .number("round", view.round.number)
            .count("findings", view.findings.len())
            .count("history", view.history.len()), output = log::rendered)]
    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError>;
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn contains_reserved_marker(body: &str) -> bool {
    body.split("<!--")
        .skip(1)
        .any(|comment| comment.trim_start().starts_with("review-"))
}
