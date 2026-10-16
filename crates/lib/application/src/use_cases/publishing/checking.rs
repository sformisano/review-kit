use crate::observation as log;
use std::collections::HashMap;

use review_kit_domain::{Finding, FindingStatus, ReviewDecision, Revision, SourcePath, Subject};

use super::{
    planning::{
        PlanError, PublishRequest, check_pending_profile, cumulative_policy_names, nonempty,
        plan_from_snapshot_managed, public_render_context, source_checks, summary_context,
    },
    reply_checks::check_replies,
    shape_checks::check_shapes,
};
use crate::{
    ports::{
        AnswerRenderInput, CommittedSource, FindingRenderInput, PortError, RenderProfileIdentity,
        ReviewRenderer, TransitionRenderInput,
    },
    publication::{Plan, RoundStatus, Snapshot},
    summary::{SummaryView, summary_view},
    use_cases::{assessment::check_findings, discovery::validate_snapshot},
};

#[review_kit_observation::operations]
#[operation(name = "application.publication.check_review", input = log::request(request), output = |result| log::Completion::result(result, log::plan, |errors| log::plan_errors(errors)))]
/// Check the complete request against retained observations and return a read-only plan.
///
/// Independent selection, shape, source, reply, and rendering failures are collected.
/// Snapshot conflicts stop checking because later checks require trusted context.
/// No review discovery, publication session, or provider writes occur. Source and
/// renderer adapters can perform I/O; success does not establish write authority.
pub fn check_review(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
) -> Result<Plan, Vec<PlanError>> {
    check_pending_profile(
        snapshot,
        request.assessment.reviewer(),
        &crate::ports::ManagedRenderer::new(renderer),
    )
    .map_err(|error| vec![error])?;
    validate_snapshot(
        snapshot,
        request.assessment.subject(),
        request.assessment.reviewer(),
    )
    .map_err(|error| {
        vec![super::planning::discovery_error(
            super::super::discovery::DiscoveryError::Conflict(error),
        )]
    })?;
    let mut source = CachedSource {
        inner: source,
        lines: HashMap::new(),
        positions: Vec::new(),
    };
    let mut renderer = CachedRenderer {
        inner: renderer,
        findings: Vec::new(),
        transitions: Vec::new(),
        answers: HashMap::new(),
        summaries: Vec::new(),
        contextual_findings: Vec::new(),
        contextual_transitions: Vec::new(),
        contextual_answers: Vec::new(),
    };
    let mut source = crate::ports::ManagedSource::new(&mut source);
    let mut renderer = crate::ports::ManagedRenderer::new(&mut renderer);
    let mut errors = check_shapes(request, snapshot);
    errors.extend(finding_input_errors(
        request,
        snapshot,
        &mut source,
        &mut renderer,
    ));
    let retained = snapshot
        .summaries
        .iter()
        .find(|summary| {
            &summary.author == request.assessment.reviewer()
                && summary.current.assessment.revision() == request.assessment.revision()
        })
        .map(|summary| &summary.current);
    let replies = match check_replies(request, snapshot, &mut renderer, retained) {
        Ok(replies) => Some(replies),
        Err(issues) => {
            errors.extend(issues);
            None
        }
    };
    match summary_context(request, snapshot) {
        Ok((mut round, history)) => {
            if let Some(replies) = replies {
                round.answer_targets = replies.targets;
            }
            if let Some(retained) = retained
                .filter(|round| matches!(round.status, RoundStatus::Pending | RoundStatus::Partial))
            {
                round.actions = retained.actions.clone();
            }
            check_summary(&round, &history, snapshot, &mut renderer, &mut errors);
            let initial_status = round.status;
            round.status = if request.assessment.decision() == ReviewDecision::Blocked {
                RoundStatus::Blocked
            } else {
                RoundStatus::Completed
            };
            if round.status != initial_status {
                check_summary(&round, &history, snapshot, &mut renderer, &mut errors);
            }
        }
        Err(error) => errors.push(error),
    }
    if errors.is_empty() {
        plan_from_snapshot_managed(request, snapshot, &mut source, &mut renderer)
            .map_err(|error| vec![error])
    } else {
        Err(errors)
    }
}

#[review_kit_observation::operations]
#[operation(name = "application.publication.check_review_with_progress", input = log::request(request), output = |result| log::Completion::result(result, log::plan, |errors| log::plan_errors(errors)))]
/// Check the review with durable confirmed resolution receipts from earlier invocations.
pub fn check_review_with_progress(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut dyn CommittedSource,
    renderer: &mut dyn ReviewRenderer,
    progress: &super::PublicationProgress,
) -> Result<Plan, Vec<PlanError>> {
    super::progress::check_unresolved_profile(
        progress,
        &crate::ports::ManagedRenderer::new(renderer),
    )
    .map_err(|error| vec![error])?;
    let plan = check_review(request, snapshot, source, renderer)?;
    super::history::with_progress(
        plan,
        request,
        snapshot,
        &mut crate::ports::ManagedRenderer::new(renderer),
        progress,
    )
    .map_err(|error| vec![error])
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn finding_input_errors(
    request: &PublishRequest,
    snapshot: &Snapshot,
    source: &mut crate::ports::ManagedSource<'_>,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
) -> Vec<PlanError> {
    let mut errors = Vec::new();
    let (round, history) = match summary_context(request, snapshot) {
        Ok(context) => context,
        Err(error) => return vec![error],
    };
    let context = public_render_context(&round);
    let names = cumulative_policy_names(&round, &history);
    let needed = match source_checks(&request.assessment, snapshot) {
        Ok(findings) => findings,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if let Err(issues) = check_findings(
        &request.assessment,
        &needed,
        source,
        renderer,
        &context,
        &names,
    ) {
        errors.push(PlanError::Checks(issues));
    }
    for finding in request
        .assessment
        .findings()
        .iter()
        .filter(|f| !needed.iter().any(|needed| needed.id() == f.id()))
    {
        let rendered = names
            .iter()
            .find(|policy| &policy.id == finding.policy_id())
            .ok_or_else(|| PlanError::MissingPolicyName(finding.policy_id().clone()))
            .and_then(|policy| {
                renderer
                    .finding_context(FindingRenderInput {
                        finding,
                        policy_name: &policy.name,
                        context: &context,
                    })
                    .map_err(PlanError::Render)
            });
        if let Err(error) = rendered.and_then(nonempty) {
            errors.push(error);
        }
    }
    errors
}

#[review_kit_observation::operations]
#[helper]
fn check_summary(
    round: &crate::publication::Round,
    history: &[crate::publication::Round],
    snapshot: &Snapshot,
    renderer: &mut crate::ports::ManagedRenderer<'_>,
    errors: &mut Vec<PlanError>,
) {
    let result = summary_view(round, history, snapshot)
        .and_then(|view| renderer.summary(&view).map_err(PlanError::Render))
        .and_then(nonempty);
    if let Err(error) = result {
        errors.push(error);
    }
}

struct CachedSource<'a> {
    inner: &'a mut dyn CommittedSource,
    lines: HashMap<(Subject, Revision, SourcePath), Vec<String>>,
    positions: Vec<(Subject, Revision, Finding)>,
}

#[review_kit_observation::operations]
impl CommittedSource for CachedSource<'_> {
    #[helper]
    fn lines(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        path: &SourcePath,
    ) -> Result<Vec<String>, PortError> {
        let key = (subject.clone(), revision.clone(), path.clone());
        if let Some(lines) = self.lines.get(&key) {
            return Ok(lines.clone());
        }
        let lines = self.inner.lines(subject, revision, path)?;
        self.lines.insert(key, lines.clone());
        Ok(lines)
    }
    #[helper]
    fn validate_position(
        &mut self,
        subject: &Subject,
        revision: &Revision,
        finding: &Finding,
    ) -> Result<(), PortError> {
        let key = (subject.clone(), revision.clone(), finding.clone());
        if self.positions.contains(&key) {
            return Ok(());
        }
        self.inner.validate_position(subject, revision, finding)?;
        self.positions.push(key);
        Ok(())
    }
}

struct CachedRenderer<'a> {
    inner: &'a mut dyn ReviewRenderer,
    findings: Vec<(Finding, String)>,
    transitions: Vec<(Finding, FindingStatus, String, String)>,
    answers: HashMap<String, String>,
    summaries: Vec<(SummaryView, String)>,
    contextual_findings: Vec<(FindingRenderKey, String)>,
    contextual_transitions: Vec<(TransitionRenderKey, String)>,
    contextual_answers: Vec<(AnswerRenderKey, String)>,
}

#[derive(PartialEq, Eq)]
struct FindingRenderKey {
    finding: Finding,
    policy_name: String,
    context: crate::ports::PublicRenderContext,
}

#[review_kit_observation::operations]
impl From<FindingRenderInput<'_>> for FindingRenderKey {
    #[helper]
    fn from(input: FindingRenderInput<'_>) -> Self {
        Self {
            finding: input.finding.clone(),
            policy_name: input.policy_name.to_owned(),
            context: input.context.clone(),
        }
    }
}

#[derive(PartialEq, Eq)]
struct TransitionRenderKey {
    finding: FindingRenderKey,
    previous: FindingStatus,
    explanation: String,
}

#[derive(PartialEq, Eq)]
struct AnswerRenderKey {
    answer: crate::publication::Answer,
    finding: crate::publication::FindingRecord,
    current_finding: Finding,
    target: crate::publication::ReplyRecord,
    policy_name: String,
    context: crate::ports::PublicRenderContext,
}

#[review_kit_observation::operations]
impl ReviewRenderer for CachedRenderer<'_> {
    #[helper]
    fn identity(&self) -> Option<&RenderProfileIdentity> {
        self.inner.identity()
    }
    #[helper]
    fn finding_context(&mut self, input: FindingRenderInput<'_>) -> Result<String, PortError> {
        let key = FindingRenderKey::from(input);
        if let Some((_, body)) = self
            .contextual_findings
            .iter()
            .find(|(cached, _)| cached == &key)
        {
            return Ok(body.clone());
        }
        let body = self.inner.finding_context(input)?;
        self.contextual_findings.push((key, body.clone()));
        Ok(body)
    }
    #[helper]
    fn transition_context(
        &mut self,
        input: TransitionRenderInput<'_>,
    ) -> Result<String, PortError> {
        let key = TransitionRenderKey {
            finding: input.finding.into(),
            previous: input.previous,
            explanation: input.explanation.to_owned(),
        };
        if let Some((_, body)) = self
            .contextual_transitions
            .iter()
            .find(|(cached, _)| cached == &key)
        {
            return Ok(body.clone());
        }
        let body = self.inner.transition_context(input)?;
        self.contextual_transitions.push((key, body.clone()));
        Ok(body)
    }
    #[helper]
    fn answer_context(&mut self, input: AnswerRenderInput<'_>) -> Result<String, PortError> {
        let key = AnswerRenderKey {
            answer: input.answer.clone(),
            finding: input.finding.clone(),
            current_finding: input.current_finding.clone(),
            target: input.target.clone(),
            policy_name: input.policy_name.to_owned(),
            context: input.context.clone(),
        };
        if let Some((_, body)) = self
            .contextual_answers
            .iter()
            .find(|(cached, _)| cached == &key)
        {
            return Ok(body.clone());
        }
        let body = self.inner.answer_context(input)?;
        self.contextual_answers.push((key, body.clone()));
        Ok(body)
    }
    #[helper]
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError> {
        if let Some((_, body)) = self.findings.iter().find(|(cached, _)| cached == finding) {
            return Ok(body.clone());
        }
        let body = self.inner.finding(finding)?;
        self.findings.push((finding.clone(), body.clone()));
        Ok(body)
    }
    #[helper]
    fn transition(
        &mut self,
        finding: &Finding,
        previous: FindingStatus,
        explanation: &str,
    ) -> Result<String, PortError> {
        if let Some((_, _, _, body)) = self.transitions.iter().find(|(cached, from, text, _)| {
            cached == finding && *from == previous && text == explanation
        }) {
            return Ok(body.clone());
        }
        let body = self.inner.transition(finding, previous, explanation)?;
        self.transitions.push((
            finding.clone(),
            previous,
            explanation.to_owned(),
            body.clone(),
        ));
        Ok(body)
    }
    #[helper]
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        if let Some(rendered) = self.answers.get(body) {
            return Ok(rendered.clone());
        }
        let rendered = self.inner.answer(body)?;
        self.answers.insert(body.to_owned(), rendered.clone());
        Ok(rendered)
    }
    #[helper]
    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError> {
        if let Some((_, body)) = self.summaries.iter().find(|(cached, _)| cached == view) {
            return Ok(body.clone());
        }
        let body = self.inner.summary(view)?;
        self.summaries.push((view.clone(), body.clone()));
        Ok(body)
    }
}
