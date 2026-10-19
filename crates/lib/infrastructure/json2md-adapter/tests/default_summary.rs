mod support;

use std::error::Error;

use review_kit_application::{ports::ReviewRenderer, publication::*, summary::*};
use review_kit_domain::*;
use review_kit_json2md_adapter::Json2MdRenderer;
use support::{
    sections::{groups, layout},
    *,
};

const STATUS_LINES: [&str; 4] = [
    "This round has not finished posting.",
    "Some updates for this round are not posted yet.",
    "The merge request changed before this round finished posting.",
    "This round was stopped before it finished posting.",
];

fn render(view: &SummaryView) -> Result<String, Box<dyn Error>> {
    Ok(Json2MdRenderer::new()?.summary(view)?)
}

fn policy(id: &str, name: Option<&str>, rounds: &[u64]) -> Result<PolicySummary, Box<dyn Error>> {
    Ok(PolicySummary {
        policy_id: PolicyId::try_from(id.to_owned())?,
        name: name.map(Into::into),
        applicable_rounds: rounds.to_vec(),
        declarations: rounds.iter().map(|round| Ok(PolicyDeclarationSummary { round: *round, declaration: PublicPolicyDeclaration { policy: PolicyName { id: PolicyId::try_from(id.to_owned())?, name: name.unwrap_or(id).into() }, status: review_kit_application::use_cases::checkpoints::ApplicabilityStatus::Applicable, reason: "This round changes the behavior governed by this policy.".into() }, result: None })).collect::<Result<_, Box<dyn Error>>>()?,
        comments: 0,
    })
}

fn row(
    template: &SummaryFinding,
    id: u64,
    policy_id: &PolicyId,
    status: FindingStatus,
) -> Result<SummaryFinding, Box<dyn Error>> {
    let original = &template.finding;
    let mut row = template.clone();
    row.finding = Finding::new(FindingDraft {
        id: FindingId::try_from(id)?,
        policy_id: policy_id.clone(),
        title: original.title().into(),
        body: original.body().into(),
        severity: original.severity(),
        disposition: original.disposition(),
        status,
        evidence: original.evidence().into(),
        validated: true,
        location: original.location().clone(),
        suggestion: None,
    })?;
    Ok(row)
}

fn completed_without_findings() -> Result<SummaryView, Box<dyn Error>> {
    let mut view = summary()?;
    view.findings.clear();
    view.unpublished_findings.clear();
    view.counts = FindingCounts::default();
    view.round.status = RoundStatus::Completed;
    view.round.publication_limitation = None;
    view.history[0].communicated_findings = Some(Vec::new());
    Ok(view)
}

#[test]
fn round_without_findings_says_so_and_omits_finding_tables() -> TestResult {
    let body = render(&completed_without_findings()?)?;
    assert!(body.starts_with("### Changes required\n\nRound 2 · no findings\n"));
    assert!(!body.contains("| ID |"));
    assert!(!body.contains("Not posted yet:"));
    let sections = layout(&body)?;
    assert!(!sections.visible.contains("No findings were posted."));
    assert!(
        sections
            .section("Round 1 · Changes required")?
            .contains("\nNo findings were posted.\n")
    );
    for line in STATUS_LINES {
        assert!(!body.contains(line), "{line}");
    }
    Ok(())
}

#[test]
fn recap_lists_only_nonzero_finding_states() -> TestResult {
    let mut view = summary()?;
    for (counts, recap) in [
        (
            FindingCounts {
                total: 5,
                open: 2,
                fixed: 1,
                withdrawn: 1,
                uncertain: 1,
            },
            "Round 2 · 2 open · 1 fixed · 1 withdrawn · 1 uncertain\n",
        ),
        (
            FindingCounts {
                total: 3,
                open: 2,
                withdrawn: 1,
                ..FindingCounts::default()
            },
            "Round 2 · 2 open · 1 withdrawn\n",
        ),
    ] {
        view.counts = counts;
        let body = render(&view)?;
        assert!(
            body.starts_with(&format!("### Changes required\n\n{recap}")),
            "{body}"
        );
    }
    Ok(())
}

#[test]
fn unfinished_publication_shows_one_status_line_and_labels_earlier_rounds() -> TestResult {
    for (status, line, label) in [
        (RoundStatus::Pending, Some(STATUS_LINES[0]), " · pending"),
        (RoundStatus::Partial, Some(STATUS_LINES[1]), " · partial"),
        (
            RoundStatus::Superseded,
            Some(STATUS_LINES[2]),
            " · superseded",
        ),
        (
            RoundStatus::Abandoned,
            Some(STATUS_LINES[3]),
            " · abandoned",
        ),
        (RoundStatus::Completed, None, ""),
        (RoundStatus::Blocked, None, ""),
    ] {
        let mut view = summary()?;
        view.round.status = status;
        view.history[0].status = status;
        let body = render(&view)?;
        let sections = layout(&body)?;
        for candidate in STATUS_LINES {
            assert_eq!(
                body.matches(candidate).count(),
                usize::from(Some(candidate) == line),
                "{status:?}: {candidate}"
            );
        }
        if let Some(line) = line {
            assert!(sections.visible.contains(line), "{status:?}: {body}");
        }
        assert_eq!(
            sections.outline(),
            [
                (0, format!("Round 1 · Changes required{label}").as_str()),
                (0, "Review details"),
            ],
            "{status:?}: {body}"
        );
    }
    Ok(())
}

#[test]
fn abandonment_reason_appears_in_review_details() -> TestResult {
    let mut view = summary()?;
    view.round.status = RoundStatus::Abandoned;
    view.round.abandonment = Some(Abandonment {
        reason: "Superseded by *manual* review".into(),
        verified_actions: Vec::new(),
    });
    let body = render(&view)?;
    let sections = layout(&body)?;
    assert!(
        groups(sections.section("Review details")?)
            .group("Round 2")?
            .contains("\n- Stopped: Superseded by \\*manual\\* review\n")
    );
    assert_eq!(body.matches("- Stopped: ").count(), 1, "{body}");
    Ok(())
}

#[test]
fn unpublished_findings_are_listed_without_links() -> TestResult {
    let mut view = summary()?;
    view.unpublished_findings = vec![FindingId::try_from(10)?, FindingId::try_from(11)?];
    let body = render(&view)?;
    assert!(body.contains("\nNot posted yet: F10, F11\n"));
    assert!(!body.contains("#note_10"));
    assert!(!body.contains("#note_11"));
    Ok(())
}

#[test]
fn findings_waiting_to_post_are_not_reported_as_no_findings() -> TestResult {
    let mut view = completed_without_findings()?;
    view.unpublished_findings = vec![FindingId::try_from(1)?];
    let body = render(&view)?;
    assert!(body.starts_with("### Changes required\n\nRound 2\n"));
    assert!(body.contains("\nNot posted yet: F1\n"));
    assert!(!body.contains("no findings"));
    Ok(())
}

#[test]
fn policy_applicability_names_all_partial_and_earlier_rounds() -> TestResult {
    let mut view = completed_without_findings()?;
    view.round.number = 4;
    view.history = [1, 2, 3]
        .into_iter()
        .map(|number| {
            let mut round = view.history[0].clone();
            round.number = number;
            round
        })
        .collect();
    view.policies = vec![
        policy("every", Some("Every Round"), &[1, 2, 3, 4])?,
        policy("since", Some("Since Two"), &[2, 3, 4])?,
        policy("gap", Some("With Gap"), &[1, 3, 4])?,
        policy("earlier", Some("Earlier One"), &[1])?,
        policy("earlier-gap", Some("Earlier Gap"), &[1, 3])?,
        policy("unused", Some("Unused"), &[])?,
    ];
    let body = render(&view)?;
    let reason = "This round changes the behavior governed by this policy.";
    for row in [
        format!("| Every Round | — | — | {reason} |"),
        format!("| Since Two | — | — | {reason} |"),
        format!("| With Gap | — | — | {reason} |"),
        "| Earlier One | 1 |".into(),
        "| Earlier Gap | 1, 3 |".into(),
        "| Unused | — |".into(),
    ] {
        assert!(body.lines().any(|line| line == row), "{row}: {body}");
    }
    assert!(body.contains(
        "\n**Policies not declared in this round (3)**\n\n| Policy | Earlier applicable rounds |\n"
    ));
    Ok(())
}

#[test]
fn policy_results_and_open_findings_follow_the_latest_round() -> TestResult {
    let mut view = summary()?;
    let violated = view.round.assessment.findings()[0].policy_id().clone();
    let satisfied = PolicyId::try_from("satisfied-policy".to_owned())?;
    let blocked = PolicyId::try_from("blocked|policy".to_owned())?;
    let current = &view.round.assessment;
    view.round.assessment = Assessment::new(AssessmentDraft {
        subject: current.subject().clone(),
        reviewer: current.reviewer().clone(),
        revision: current.revision().clone(),
        decision: ReviewDecision::Blocked,
        depth: current.depth().into(),
        scope: current.scope().into(),
        limitation: Some("Proof is unavailable".into()),
        selected_policies: vec![violated.clone(), satisfied.clone(), blocked.clone()],
        evaluations: vec![
            PolicyEvaluation::new(
                violated.clone(),
                PolicyJudgment::Violated {
                    evidence: "Private violation evidence".into(),
                },
            )?,
            PolicyEvaluation::new(satisfied.clone(), PolicyJudgment::Satisfied)?,
            PolicyEvaluation::new(
                blocked.clone(),
                PolicyJudgment::Blocked {
                    limitation: "CI logs `expired`".into(),
                },
            )?,
        ],
        findings: current.findings().to_vec(),
    })
    .map_err(|errors| format!("{errors:?}"))?;
    let template = view.findings[0].clone();
    view.findings = vec![
        row(&template, 1, &violated, FindingStatus::Open)?,
        row(&template, 2, &violated, FindingStatus::Fixed)?,
        row(&template, 3, &violated, FindingStatus::Open)?,
        row(&template, 4, &violated, FindingStatus::Uncertain)?,
    ];
    view.policies = vec![
        policy(violated.as_str(), Some(POLICY_NAME), &[1, 2])?,
        policy(satisfied.as_str(), Some("Satisfied Policy"), &[2])?,
        policy(blocked.as_str(), None, &[2])?,
        policy("retired", Some("Retired Policy"), &[1])?,
    ];
    let body = render(&view)?;
    assert!(!body.contains("Private violation evidence"));
    let reason = "This round changes the behavior governed by this policy.";
    for row in [
        format!("| Rust Error Contracts | Fail | F1, F3 | {reason} |"),
        format!("| Satisfied Policy | Pass | — | {reason} |"),
        format!("| blocked&#124;policy | Blocked: CI logs \\`expired\\` | — | {reason} |"),
        "| Retired Policy | 1 |".into(),
    ] {
        assert!(body.lines().any(|line| line == row), "{row}: {body}");
    }
    assert_eq!(body.matches("CI logs \\`expired\\`").count(), 1, "{body}");
    assert!(!body.contains("Not checked"), "{body}");
    Ok(())
}
