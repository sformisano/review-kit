use review_kit_application::{
    ports::{PortError, ReviewRenderer},
    publication::{Effects, PolicyName, TransitionReply},
    summary::SummaryView,
    use_cases::{assessment::AssessmentPolicies, publishing::PublishRequest},
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, Revision, Severity,
    SourceLocation, SourcePath, Suggestion,
};
use review_kit_gitlab_adapter::GitLab;

pub fn revision() -> Revision {
    Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap()
}

fn policy() -> PolicyId {
    PolicyId::try_from("correctness".to_owned()).unwrap()
}

type SuggestionFixture<'a> = (u64, Vec<&'a str>, Vec<&'a str>);

pub fn finding(
    id: u64,
    path: &str,
    line: u64,
    status: FindingStatus,
    range: Option<SuggestionFixture<'_>>,
) -> Finding {
    let location = SourceLocation::new(
        SourcePath::try_from(path.to_owned()).unwrap(),
        LineNumber::try_from(line).unwrap(),
    );
    let suggestion = range.map(|(start, original, replacement)| {
        Suggestion::new(
            location.clone(),
            revision(),
            LineNumber::try_from(start).unwrap(),
            original.into_iter().map(str::to_owned).collect(),
            replacement.into_iter().map(str::to_owned).collect(),
            "Regression proves correction".into(),
        )
        .unwrap()
    });
    Finding::new(FindingDraft {
        id: FindingId::try_from(id).unwrap(),
        policy_id: policy(),
        title: format!("Request {id} is lost"),
        body: "The failure path discards the request.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Controlled regression".into(),
        validated: true,
        location,
        suggestion,
    })
    .unwrap()
}

pub fn request(client: &GitLab, findings: Vec<Finding>) -> PublishRequest {
    let policy = policy();
    let set = AssessmentPolicies::new(
        "fixture-policies".into(),
        vec![PolicyName {
            id: policy.clone(),
            name: "Correctness".into(),
        }],
    )
    .unwrap();
    PublishRequest {
        confirmed_validation: false,
        review_workflow: None,
        assessment: Assessment::new(AssessmentDraft {
            subject: client.subject().clone(),
            reviewer: client.reviewer().clone(),
            revision: revision(),
            decision: if findings
                .iter()
                .any(|finding| finding.status() == FindingStatus::Open)
            {
                ReviewDecision::ChangesRequired
            } else {
                ReviewDecision::Ready
            },
            depth: "Changed code and callers".into(),
            scope: "Request handling".into(),
            limitation: None,
            selected_policies: vec![policy.clone()],
            evaluations: vec![PolicyEvaluation::new(policy, PolicyJudgment::Satisfied).unwrap()],
            findings,
        })
        .unwrap(),
        policies: set.clone(),
        policy_declarations: set.policies().iter().map(|policy| review_kit_application::publication::PublicPolicyDeclaration { policy: policy.clone(), status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed failure path requires this policy.".into() }).collect(),
        effects: Effects::review(),
        answers: vec![],
        answer_targets: vec![],
        transitions: vec![],
        local_context: "PRIVATE local agent context".into(),
        allow_colocated: false,
    }
}

pub fn transition(
    request: &mut PublishRequest,
    finding: u64,
    from: FindingStatus,
    to: FindingStatus,
) {
    request.transitions.push(TransitionReply {
        finding: FindingId::try_from(finding).unwrap(),
        from,
        to,
        body: "Regression verifies the change".into(),
    });
}

#[derive(Default)]
pub struct Renderer {
    pub views: Vec<SummaryView>,
}
impl ReviewRenderer for Renderer {
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError> {
        let mut body = format!(
            "F{}: {}\n{}",
            finding.id().get(),
            finding.title(),
            finding.body()
        );
        if let Some(suggestion) = finding.suggestion() {
            let anchor = finding.location().line().get();
            let fence_length = suggestion
                .replacement()
                .iter()
                .flat_map(|line| line.split(|character| character != '`'))
                .map(str::len)
                .max()
                .unwrap_or(0)
                .max(2)
                + 1;
            let fence = "`".repeat(fence_length);
            body.push_str(&format!(
                "\n\n{fence}suggestion:-{}+{}\n",
                anchor - suggestion.start_line().get(),
                suggestion.end_line().get() - anchor
            ));
            if !suggestion.replacement().is_empty() {
                body.push_str(&suggestion.replacement().join("\n"));
                body.push('\n');
            }
            body.push_str(&fence);
        }
        Ok(body)
    }
    fn transition(
        &mut self,
        finding: &Finding,
        previous: FindingStatus,
        explanation: &str,
    ) -> Result<String, PortError> {
        Ok(format!(
            "F{}: {previous:?} -> {:?}; {explanation}",
            finding.id().get(),
            finding.status()
        ))
    }
    fn answer(&mut self, body: &str) -> Result<String, PortError> {
        Ok(format!("Answer: {body}"))
    }
    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError> {
        self.views.push(view.clone());
        Ok(format!(
            "Review round {}: {:?}\n{} findings; {} open",
            view.round.number, view.round.status, view.counts.total, view.counts.open
        ))
    }
}
