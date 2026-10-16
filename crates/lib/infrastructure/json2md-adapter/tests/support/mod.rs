#![allow(dead_code)]

use std::error::Error;

use review_kit_application::{ports::*, publication::*, summary::*};
use review_kit_domain::*;
use review_kit_json2md_adapter::Json2MdRenderer;

pub mod coverage;
pub mod sections;

pub type TestResult = Result<(), Box<dyn Error>>;
pub const POLICY_NAME: &str = "Rust Error Contracts";

pub fn revision() -> Result<Revision, IdentityError> {
    Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)])
}

pub fn context() -> Result<PublicRenderContext, IdentityError> {
    Ok(PublicRenderContext {
        round: 2,
        revision: revision()?,
        publication_status: RoundStatus::Pending,
        review_workflow: Some("codebase-policies-review".into()),
        policy_set: "rust-codebase-policies".into(),
        scope: "src/lib.rs".into(),
        depth: "Standard".into(),
    })
}

pub fn finding(replacement: Option<Vec<String>>) -> Result<Finding, Box<dyn Error>> {
    let location = SourceLocation::new(
        SourcePath::try_from("src/lib.rs".to_owned())?,
        LineNumber::try_from(2)?,
    );
    let suggestion = replacement
        .map(|replacement| -> Result<_, Box<dyn Error>> {
            Ok(Suggestion::new(
                location.clone(),
                revision()?,
                LineNumber::try_from(1)?,
                vec!["one".into(), "two".into(), "three".into()],
                replacement,
                "Regression proves the correction".into(),
            )?)
        })
        .transpose()?;
    Ok(Finding::new(FindingDraft {
        id: FindingId::try_from(2)?,
        policy_id: PolicyId::try_from("policy|[name]".to_owned())?,
        title: "Preserve [result] *exactly*".into(),
        body: "The result disappears.\n\n```rust\n/close\n```".into(),
        severity: Severity::P2,
        disposition: Disposition::Blocking,
        status: FindingStatus::Open,
        evidence: "Reproduced with an unavailable peer.".into(),
        validated: true,
        location,
        suggestion,
    })?)
}

pub fn copy_finding(
    original: &Finding,
    disposition: Disposition,
    status: FindingStatus,
    evidence: &str,
) -> Result<Finding, Box<dyn Error>> {
    Ok(Finding::new(FindingDraft {
        id: original.id(),
        policy_id: original.policy_id().clone(),
        title: original.title().into(),
        body: original.body().into(),
        severity: original.severity(),
        disposition,
        status,
        evidence: evidence.into(),
        validated: true,
        location: original.location().clone(),
        suggestion: original.suggestion().cloned(),
    })?)
}

pub fn render_finding(
    renderer: &mut Json2MdRenderer,
    finding: &Finding,
) -> Result<String, Box<dyn Error>> {
    Ok(renderer.finding_context(FindingRenderInput {
        finding,
        policy_name: POLICY_NAME,
        context: &context()?,
    })?)
}

pub fn summary() -> Result<SummaryView, Box<dyn Error>> {
    let finding = finding(None)?;
    let subject = Subject::new(
        "gitlab".into(),
        "https://gitlab.example/deployment".into(),
        "group/sub/project (test)".into(),
        12,
    )?;
    let assessment = Assessment::new(AssessmentDraft {
        subject,
        reviewer: ReviewerId::try_from("17".to_owned())?,
        revision: revision()?,
        decision: ReviewDecision::ChangesRequired,
        depth: "Standard | focused".into(),
        scope: "src/[lib].rs\n<details>".into(),
        limitation: Some("Review context is incomplete".into()),
        selected_policies: vec![finding.policy_id().clone()],
        evaluations: vec![PolicyEvaluation::new(
            finding.policy_id().clone(),
            PolicyJudgment::Violated {
                evidence: "Observed loss".into(),
            },
        )?],
        findings: vec![finding.clone()],
    })
    .map_err(|errors| format!("{errors:?}"))?;
    let round = Round {
        confirmed_validation: true,
        number: 2,
        assessment,
        status: RoundStatus::Partial,
        abandonment: None,
        publication_limitation: Some(PublicationLimitation::Uncertain),
        answers: vec![],
        transitions: vec![],
        answer_targets: vec![],
        actions: vec![],
        review_workflow: Some("codebase-policies-review".into()),
        policy_set: Some("rust-codebase-policies".into()),
        policy_declarations: vec![PublicPolicyDeclaration {
            policy: PolicyName {
                id: finding.policy_id().clone(),
                name: POLICY_NAME.into(),
            },
            status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable,
            reason: "The changed error handling requires this policy.".into(),
        }],
        policy_names: vec![PolicyName {
            id: finding.policy_id().clone(),
            name: POLICY_NAME.into(),
        }],
        renderer: None,
        communicated_findings: None,
        verified_resolutions: Vec::new(),
    };
    let root = FindingRecord {
        id: ProviderId::new("discussion-abc".into())?,
        root_note: ProviderId::new("701".into())?,
        author: round.assessment.reviewer().clone(),
        created_in: 1,
        revision: revision()?,
        finding: finding.clone(),
        body: "published body".into(),
        resolved: true,
    };
    let mut old = round.clone();
    old.number = 1;
    old.status = RoundStatus::Completed;
    old.publication_limitation = None;
    old.communicated_findings = Some(vec![HistoricalFinding {
        finding: finding.clone(),
        root_note: root.root_note.clone(),
        thread: root.id.clone(),
        actions: vec![PublishedAction::Created],
    }]);
    let fixed = copy_finding(
        &finding,
        Disposition::Blocking,
        FindingStatus::Fixed,
        "Regression passes",
    )?;
    Ok(SummaryView {
        round,
        history: vec![old],
        findings: vec![SummaryFinding {
            root,
            finding: fixed,
            actions: vec![
                PublishedAction::Transition {
                    from: FindingStatus::Open,
                    to: FindingStatus::Fixed,
                },
                PublishedAction::Resolved,
            ],
        }],
        counts: FindingCounts {
            total: 1,
            fixed: 1,
            ..FindingCounts::default()
        },
        policies: vec![
            PolicySummary {
                policy_id: finding.policy_id().clone(),
                name: Some(POLICY_NAME.into()),
                applicable_rounds: vec![1, 2],
                declarations: [1, 2].into_iter().map(|round| PolicyDeclarationSummary { round, declaration: PublicPolicyDeclaration { policy: PolicyName { id: finding.policy_id().clone(), name: POLICY_NAME.into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "The changed error handling requires this policy.".into() }, result: Some(PolicyJudgment::Violated { evidence: "Observed loss".into() }) }).collect(),
                comments: 1,
            },
            PolicySummary {
                policy_id: PolicyId::try_from("rust-ownership-design".to_owned())?,
                name: Some("Rust Ownership Design".into()),
                applicable_rounds: vec![],
                declarations: vec![PolicyDeclarationSummary { round: 2, declaration: PublicPolicyDeclaration { policy: PolicyName { id: PolicyId::try_from("rust-ownership-design".to_owned())?, name: "Rust Ownership Design".into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Excluded, reason: "No ownership boundary changes.".into() }, result: None }],
                comments: 0,
            },
        ],
        unpublished_findings: vec![FindingId::try_from(10)?],
    })
}

pub fn answer_parts(view: &SummaryView) -> Result<(Answer, ReplyRecord), Box<dyn Error>> {
    let target = ReplyRecord {
        id: ProviderId::new("702".into())?,
        author: ReviewerId::try_from("21".to_owned())?,
        thread: view.findings[0].root.id.clone(),
        action: None,
        body: "Why does the error disappear?".into(),
    };
    let answer = Answer {
        finding: view.findings[0].finding.id(),
        in_reply_to: target.id.clone(),
        body: "The return value is retained.\n\n`Ok(result)`".into(),
    };
    Ok((answer, target))
}
