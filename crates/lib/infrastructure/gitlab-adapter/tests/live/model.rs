use review_kit_application::{
    publication::{Effects, PolicyName},
    use_cases::{assessment::AssessmentPolicies, publishing::PublishRequest},
};
use review_kit_domain::{
    Assessment, AssessmentDraft, Disposition, Finding, FindingDraft, FindingId, FindingStatus,
    LineNumber, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision, Revision, Severity,
    SourceLocation, SourcePath, Suggestion,
};
use review_kit_gitlab_adapter::GitLab;

// The renderer is a deterministic fixture; all provider operations use GitLab.
#[allow(dead_code)]
#[path = "../support/model.rs"]
mod controlled_model;
pub use controlled_model::{Renderer, transition};

fn policy() -> PolicyId {
    PolicyId::try_from("correctness".to_owned()).unwrap()
}

pub fn finding(
    revision: &Revision,
    id: u64,
    path: &str,
    status: FindingStatus,
    range: Option<(u64, Vec<&str>, Vec<&str>)>,
) -> Finding {
    let location = SourceLocation::new(
        SourcePath::try_from(path.to_owned()).unwrap(),
        LineNumber::try_from(3).unwrap(),
    );
    let suggestion = range.map(|(start, original, replacement)| {
        Suggestion::new(
            location.clone(),
            revision.clone(),
            LineNumber::try_from(start).unwrap(),
            original.into_iter().map(str::to_owned).collect(),
            replacement.into_iter().map(str::to_owned).collect(),
            "Live suggestion readback verifies the replacement".into(),
        )
        .unwrap()
    });
    Finding::new(FindingDraft {
        id: FindingId::try_from(id).unwrap(),
        policy_id: policy(),
        title: format!("Finding {id}"),
        body: "The added line needs correction.".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status,
        evidence: "Disposable MR source".into(),
        validated: true,
        location,
        suggestion,
    })
    .unwrap()
}

pub fn request(client: &GitLab, revision: &Revision, findings: Vec<Finding>) -> PublishRequest {
    let policy = policy();
    let set = AssessmentPolicies::new(
        "live-policies".into(),
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
            revision: revision.clone(),
            decision: if findings.iter().any(|f| f.status() == FindingStatus::Open) {
                ReviewDecision::ChangesRequired
            } else {
                ReviewDecision::Ready
            },
            depth: "Changed source and provider results".into(),
            scope: "Disposable MR".into(),
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
        local_context: "PRIVATE fixture context".into(),
        allow_colocated: false,
    }
}
