use review_kit_application::{
    ports::{AnswerRenderInput, PublicRenderContext, TransitionRenderInput},
    publication::{PublicationLimitation, Round, RoundStatus},
    summary::{PublishedAction, SummaryView},
};
use review_kit_domain::{
    Disposition, Finding, FindingId, FindingStatus, PolicyJudgment, ReviewDecision, Revision,
    Severity, SourcePath, Subject, Suggestion,
};
use serde_json::{Value, json};
use url::Url;

use crate::RendererError;

/// The rendering-input shape that a profile manifest declares.
/// Current v2 profiles include the full public policy declaration history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputContract {
    V1,
    /// Summary policy results and retained public declarations.
    V2,
}

impl InputContract {
    const ALL: [Self; 2] = [Self::V1, Self::V2];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "review-kit-render-input/v1",
            Self::V2 => "review-kit-render-input/v2",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|contract| contract.as_str() == value)
    }
}

pub(crate) fn text(value: &str) -> String {
    let mut output = String::new();
    for c in value.chars() {
        match c {
            '\n' => output.push_str("&#10;"),
            '\r' => output.push_str("&#13;"),
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '|' => output.push_str("&#124;"),
            '@' => output.push_str("@\u{200b}"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '!' => {
                output.push('\\');
                output.push(c);
            }
            _ => output.push(c),
        }
    }
    output
}

pub(crate) fn status(status: FindingStatus) -> &'static str {
    match status {
        FindingStatus::Open => "Open",
        FindingStatus::Fixed => "Fixed",
        FindingStatus::Withdrawn => "Withdrawn",
        FindingStatus::Uncertain => "Uncertain",
    }
}

fn severity(value: Severity) -> &'static str {
    match value {
        Severity::P1 => "P1",
        Severity::P2 => "P2",
        Severity::P3 => "P3",
        Severity::P4 => "P4",
        Severity::P5 => "P5",
    }
}

fn disposition(value: Disposition) -> &'static str {
    match value {
        Disposition::Blocking => "Blocking",
        Disposition::Advisory => "Advisory",
    }
}

fn round_status(value: RoundStatus) -> &'static str {
    match value {
        RoundStatus::Pending => "Pending",
        RoundStatus::Partial => "Partial",
        RoundStatus::Completed => "Completed",
        RoundStatus::Blocked => "Blocked",
        RoundStatus::Superseded => "Superseded",
        RoundStatus::Abandoned => "Abandoned",
    }
}

fn revision(value: &Revision) -> Value {
    json!({
        "head": value.head(),
        "head_markdown": text(value.head()),
        "comparison": value.comparison(),
        "comparison_markdown": value.comparison().iter().map(|value| text(value)).collect::<Vec<_>>(),
    })
}

pub(crate) fn context(value: &PublicRenderContext) -> Value {
    json!({
        "round": value.round,
        "revision": revision(&value.revision),
        "publication_status": round_status(value.publication_status),
        "review_workflow": value.review_workflow,
        "review_workflow_markdown": value.review_workflow.as_deref().map(text),
        "policy_set": value.policy_set,
        "policy_set_markdown": text(&value.policy_set),
        "scope": value.scope,
        "scope_markdown": text(&value.scope),
        "depth": value.depth,
        "depth_markdown": text(&value.depth),
    })
}

fn finding_fields(finding: &Finding, policy_name: Option<&str>, contract: InputContract) -> Value {
    json!({
        "version": contract.as_str(),
        "id": finding.id().get(),
        "title": finding.title(),
        "title_markdown": text(finding.title()),
        "body": finding.body(),
        "evidence": finding.evidence(),
        "severity": severity(finding.severity()),
        "disposition": disposition(finding.disposition()),
        "status": status(finding.status()),
        "policy": {
            "id": finding.policy_id().as_str(),
            "name": policy_name,
            "name_markdown": policy_name.map(text),
        },
        "location": {
            "path": finding.location().path().as_str(),
            "path_markdown": text(finding.location().path().as_str()),
            "line": finding.location().line().get(),
        },
        "suggestion": null,
    })
}

pub(crate) fn finding(
    finding: &Finding,
    policy_name: &str,
    public: &PublicRenderContext,
    contract: InputContract,
) -> Value {
    let mut value = finding_fields(finding, Some(policy_name), contract);
    value["context"] = context(public);
    value
}

pub(crate) fn suggestion(id: FindingId, value: &Suggestion, contract: InputContract) -> Value {
    let replacement = value.replacement().join("\n");
    let longest = replacement
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    let above = value.location().line().get() - value.start_line().get();
    let below = value.end_line().get() - value.location().line().get();
    let info = if above == 0 && below == 0 {
        "suggestion".to_owned()
    } else {
        format!("suggestion:-{above}+{below}")
    };
    let content = if value.replacement().is_empty() {
        String::new()
    } else {
        format!("{replacement}\n")
    };
    json!({
        "version": contract.as_str(),
        "id": id.get(),
        "path": value.location().path().as_str(),
        "path_markdown": text(value.location().path().as_str()),
        "anchor_line": value.location().line().get(),
        "start_line": value.start_line().get(),
        "end_line": value.end_line().get(),
        "original": value.original(),
        "replacement": value.replacement(),
        "replacement_text": replacement,
        "replacement_content": content,
        "fence": fence,
        "info": info,
        "lines_above": above,
        "lines_below": below,
        "revision": revision(value.revision()),
        "evidence": value.evidence(),
    })
}

pub(crate) fn transition(
    input: TransitionRenderInput<'_>,
    finding: Value,
    contract: InputContract,
) -> Value {
    json!({
        "version": contract.as_str(),
        "finding": finding,
        "previous_status": status(input.previous),
        "new_status": status(input.finding.finding.status()),
        "explanation": input.explanation,
        "context": context(input.finding.context),
    })
}

pub(crate) fn answer(input: AnswerRenderInput<'_>, contract: InputContract) -> Value {
    json!({
        "version": contract.as_str(),
        "id": input.answer.finding.get(),
        "title": input.current_finding.title(),
        "title_markdown": text(input.current_finding.title()),
        "body": input.answer.body,
        "in_reply_to": input.answer.in_reply_to.as_str(),
        "root_note": input.finding.root_note.as_str(),
        "thread": input.finding.id.as_str(),
        "question": {
            "id": input.target.id.as_str(),
            "thread": input.target.thread.as_str(),
            "author": input.target.author.as_str(),
            "body": input.target.body,
        },
        "finding": finding_fields(input.current_finding, Some(input.policy_name), contract),
        "context": context(input.context),
    })
}

/// Violation evidence stays in the assessment; only a blocked check exposes its limitation.
fn policy_result(value: &PolicyJudgment) -> Value {
    let (kind, limitation) = match value {
        PolicyJudgment::Satisfied => ("Satisfied", None),
        PolicyJudgment::Violated { .. } => ("Violated", None),
        PolicyJudgment::Blocked { limitation } => ("Blocked", Some(limitation.as_str())),
    };
    json!({
        "kind": kind,
        "limitation": limitation,
        "limitation_markdown": limitation.map(text),
    })
}

fn decision(value: ReviewDecision) -> &'static str {
    match value {
        ReviewDecision::Ready => "Ready",
        ReviewDecision::ChangesRequired => "Changes required",
        ReviewDecision::Blocked => "Blocked",
    }
}

fn limitation(value: &PublicationLimitation) -> &'static str {
    match value {
        PublicationLimitation::Authority(_) => "Required publication authority is unavailable",
        PublicationLimitation::RevisionChanged => "Reviewed revision changed",
        PublicationLimitation::ConcurrentChange => "Provider state changed concurrently",
        PublicationLimitation::ProviderUnavailable => "Provider is unavailable",
        PublicationLimitation::Rejected => "A required publication effect was rejected",
        PublicationLimitation::Uncertain => "A publication effect has an uncertain outcome",
        PublicationLimitation::InvalidRendering => "A required body could not be rendered",
        PublicationLimitation::PreparationFailed => {
            "Publication preparation failed after a verified effect"
        }
        PublicationLimitation::EvidenceUnavailable => "Publication evidence could not be retained",
    }
}

fn round(
    value: &Round,
    history: &[Round],
    contract: InputContract,
) -> Result<Value, RendererError> {
    let rows = value
        .communicated_findings
        .as_ref()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let name = value
                        .policy_names
                        .iter()
                        .find(|policy| &policy.id == row.finding.policy_id())
                        .map(|policy| policy.name.as_str())
                        .or_else(|| {
                            history
                                .iter()
                                .filter(|prior| prior.number < value.number)
                                .filter_map(|prior| {
                                    prior
                                        .policy_names
                                        .iter()
                                        .find(|policy| &policy.id == row.finding.policy_id())
                                        .map(|policy| (prior.number, policy.name.as_str()))
                                })
                                .max_by_key(|(number, _)| *number)
                                .map(|(_, name)| name)
                        });
                    finding_row(
                        value.assessment.subject(),
                        &row.finding,
                        row.root_note.as_str(),
                        row.thread.as_str(),
                        &row.actions,
                        name,
                        contract,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(json!({
        "number": value.number,
        "revision": revision(value.assessment.revision()),
        "revision_kind": if matches!(value.status, RoundStatus::Completed | RoundStatus::Blocked) { "Reviewed" } else { "Attempted" },
        "decision": decision(value.assessment.decision()),
        "status": round_status(value.status),
        "depth": value.assessment.depth(),
        "depth_markdown": text(value.assessment.depth()),
        "scope": value.assessment.scope(),
        "scope_markdown": text(value.assessment.scope()),
        "review_workflow": value.review_workflow,
        "review_workflow_markdown": value.review_workflow.as_deref().map(text),
        "policy_set": value.policy_set,
        "policy_set_markdown": value.policy_set.as_deref().map(text),
        "limitation": value.assessment.limitation(),
        "limitation_markdown": value.assessment.limitation().map(text),
        "publication_limitation": value.publication_limitation.as_ref().map(limitation),
        "abandonment": value.abandonment.as_ref().map(|value| value.reason.as_str()),
        "abandonment_markdown": value.abandonment.as_ref().map(|value| text(&value.reason)),
        "history_available": rows.is_some(),
        "findings": rows,
    }))
}

fn note_url(subject: &Subject, note: &str) -> Result<String, RendererError> {
    if subject.provider() != "gitlab"
        || note.parse::<u64>().ok().filter(|v| *v != 0).is_none()
        || !note.bytes().all(|v| v.is_ascii_digit())
        || SourcePath::try_from(subject.repository().to_owned()).is_err()
        || subject.repository().split('/').count() < 2
    {
        return Err(RendererError::Subject);
    }
    let mut url = Url::parse(subject.host()).map_err(RendererError::Url)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RendererError::Subject);
    }
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| RendererError::Subject)?;
        path.pop_if_empty();
        for segment in subject.repository().split('/') {
            path.push(segment);
        }
        path.extend(["-", "merge_requests", &subject.request().to_string()]);
    }
    url.set_fragment(Some(&format!("note_{note}")));
    Ok(url.to_string())
}

fn action(value: &PublishedAction) -> Value {
    match value {
        PublishedAction::Created => json!({
            "kind": "created",
            "label": "Created",
            "from": null,
            "to": null,
            "in_reply_to": null,
        }),
        PublishedAction::Transition { from, to } => json!({
            "kind": "transition",
            "label": if from == to {
                "Updated".to_owned()
            } else {
                format!("{} to {}", status(*from), status(*to))
            },
            "from": status(*from),
            "to": status(*to),
            "in_reply_to": null,
        }),
        PublishedAction::Answered { in_reply_to } => json!({
            "kind": "answered",
            "label": "Answered",
            "from": null,
            "to": null,
            "in_reply_to": in_reply_to.as_str(),
        }),
        PublishedAction::Resolved | PublishedAction::ObservedResolved => json!({
            "kind": "resolved",
            "label": if matches!(value, PublishedAction::ObservedResolved) {
                "Observed resolved"
            } else {
                "Resolved"
            },
            "from": null,
            "to": null,
            "in_reply_to": null,
        }),
        PublishedAction::Reopened | PublishedAction::ObservedReopened => json!({
            "kind": "reopened",
            "label": if matches!(value, PublishedAction::ObservedReopened) {
                "Observed reopened"
            } else {
                "Reopened"
            },
            "from": null,
            "to": null,
            "in_reply_to": null,
        }),
    }
}

fn finding_row(
    subject: &Subject,
    finding: &Finding,
    root_note: &str,
    thread: &str,
    actions: &[PublishedAction],
    name: Option<&str>,
    contract: InputContract,
) -> Result<Value, RendererError> {
    let mut row = finding_fields(finding, name, contract);
    row["url"] = Value::String(note_url(subject, root_note)?);
    row["root_note"] = Value::String(root_note.to_owned());
    row["thread"] = Value::String(thread.to_owned());
    row["actions"] = Value::Array(actions.iter().map(action).collect());
    if let Some(suggestion) = finding.suggestion() {
        row["suggestion"] = self::suggestion(finding.id(), suggestion, contract);
    }
    Ok(row)
}

pub(crate) fn summary(view: &SummaryView, contract: InputContract) -> Result<Value, RendererError> {
    let findings = view
        .findings
        .iter()
        .map(|row| {
            let name = view
                .policies
                .iter()
                .find(|policy| &policy.policy_id == row.finding.policy_id())
                .and_then(|policy| policy.name.as_deref());
            finding_row(
                view.round.assessment.subject(),
                &row.finding,
                row.root.root_note.as_str(),
                row.root.id.as_str(),
                &row.actions,
                name,
                contract,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut history = view.history.iter().collect::<Vec<_>>();
    history.sort_by_key(|v| std::cmp::Reverse(v.number));
    let policies = view
        .policies
        .iter()
        .map(|row| {
            let mut policy = json!({
                "id": row.policy_id.as_str(),
                "id_markdown": text(row.policy_id.as_str()),
                "name": row.name,
                "name_markdown": row.name.as_deref().map(text),
                "rounds": row.applicable_rounds,
                "comments": row.comments,
            });
            match contract {
                InputContract::V1 => {}
                InputContract::V2 => {
                    policy["declarations"] = Value::Array(row.declarations.iter().map(|retained| {
                        let declaration = &retained.declaration;
                        json!({
                            "round": retained.round,
                            "name": declaration.policy.name,
                            "name_markdown": text(&declaration.policy.name),
                            "status": match declaration.status {
                                review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable => "Applicable",
                                review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Excluded => "Excluded",
                                review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Unresolved => "Unresolved",
                            },
                            "reason": declaration.reason,
                            "reason_markdown": text(&declaration.reason),
                            "result": retained.result.as_ref().map(policy_result),
                        })
                    }).collect());
                    policy["result"] = view
                        .round
                        .assessment
                        .evaluations()
                        .iter()
                        .find(|evaluation| evaluation.policy_id() == &row.policy_id)
                        .map_or(Value::Null, |evaluation| {
                            policy_result(evaluation.judgment())
                        });
                }
            }
            policy
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "version": contract.as_str(),
        "round": round(&view.round, &view.history, contract)?,
        "history": history.into_iter().map(|item| round(item, &view.history, contract)).collect::<Result<Vec<_>,_>>()?,
        "findings": findings,
        "counts": {
            "total": view.counts.total,
            "open": view.counts.open,
            "fixed": view.counts.fixed,
            "withdrawn": view.counts.withdrawn,
            "uncertain": view.counts.uncertain,
        },
        "policies": policies,
        "unpublished": view.unpublished_findings.iter().map(|id| id.get()).collect::<Vec<_>>(),
    }))
}
