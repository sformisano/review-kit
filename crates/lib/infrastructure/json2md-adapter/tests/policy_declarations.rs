mod support;

use std::error::Error;

use pulldown_cmark::{Event, Options, Parser, Tag};
use review_kit_application::{
    ports::ReviewRenderer,
    publication::*,
    summary::{SummaryView, summary_view},
    use_cases::checkpoints::ApplicabilityStatus,
};
use review_kit_domain::*;
use review_kit_json2md_adapter::Json2MdRenderer;
use support::sections::layout;

fn declared_view() -> Result<SummaryView, Box<dyn Error>> {
    let mut current = support::summary()?.round;
    let applicable = current.assessment.selected_policies()[0].clone();
    let excluded = PolicyId::try_from("async-work".to_owned())?;
    let unresolved = PolicyId::try_from("unavailable".to_owned())?;
    let assessment = &current.assessment;
    current.assessment = Assessment::new(AssessmentDraft {
        subject: assessment.subject().clone(),
        reviewer: assessment.reviewer().clone(),
        revision: assessment.revision().clone(),
        decision: ReviewDecision::Blocked,
        scope: assessment.scope().into(),
        depth: assessment.depth().into(),
        limitation: Some("The selected definition is unavailable.".into()),
        selected_policies: vec![applicable.clone(), unresolved.clone()],
        evaluations: vec![
            PolicyEvaluation::new(
                applicable.clone(),
                PolicyJudgment::Blocked {
                    limitation: "Proof logs are unavailable.".into(),
                },
            )?,
            PolicyEvaluation::new(
                unresolved.clone(),
                PolicyJudgment::Blocked {
                    limitation: "The selected definition is unavailable.".into(),
                },
            )?,
        ],
        findings: vec![],
    })
    .map_err(|errors| format!("{errors:?}"))?;
    current.status = RoundStatus::Blocked;
    current.policy_declarations = vec![
        PublicPolicyDeclaration {
            policy: PolicyName {
                id: applicable,
                name: "Applicable with blocked proof".into(),
            },
            status: ApplicabilityStatus::Applicable,
            reason: "This change requires the policy.".into(),
        },
        PublicPolicyDeclaration {
            policy: PolicyName {
                id: excluded,
                name: "Async | <script>@team\nwork".into(),
            },
            status: ApplicabilityStatus::Excluded,
            reason: "No | <script>@team\nasync work.".into(),
        },
        PublicPolicyDeclaration {
            policy: PolicyName {
                id: unresolved,
                name: "Unavailable policy".into(),
            },
            status: ApplicabilityStatus::Unresolved,
            reason: "The selected definition is unavailable.".into(),
        },
    ];
    current.policy_names = vec![
        current.policy_declarations[0].policy.clone(),
        current.policy_declarations[2].policy.clone(),
    ];
    let mut old = current.clone();
    old.number = 1;
    old.policy_declarations[1].policy.name = "Original Async Work".into();
    old.policy_declarations[1].reason = "Original exclusion reason.".into();
    let snapshot = Snapshot {
        subject: current.assessment.subject().clone(),
        reviewer: current.assessment.reviewer().clone(),
        revision: current.assessment.revision().clone(),
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::none(),
    };
    Ok(summary_view(&current, &[old], &snapshot)?)
}

#[test]
fn full_inventory_distinguishes_exclusion_unresolved_applicability_and_blocked_proof()
-> Result<(), Box<dyn Error>> {
    let view = declared_view()?;
    assert_eq!(view.policies.len(), 3);
    for policy in &view.policies {
        if policy.policy_id.as_str() == "async-work" || policy.policy_id.as_str() == "unavailable" {
            assert!(policy.applicable_rounds.is_empty());
        } else {
            assert_eq!(policy.applicable_rounds, vec![1, 2]);
        }
        assert_eq!(policy.declarations.len(), 2);
    }
    let body = Json2MdRenderer::new()?.summary(&view)?;
    let sections = layout(&body)?;
    let [(0, round), (0, "Review details")] = sections.outline()[..] else {
        return Err(format!("expected one earlier round, then review details: {body}").into());
    };
    let current = sections.section("Review details")?;
    let history = sections.section(round)?;
    for row in [
        "| Applicable with blocked proof | Blocked: Proof logs are unavailable. | — | This change requires the policy. |",
        "| Unavailable policy | Blocked | — | The selected definition is unavailable. |",
        "| Async &#124; &lt;script&gt;@\u{200b}team&#10;work | No &#124; &lt;script&gt;@\u{200b}team&#10;async work. |",
    ] {
        assert!(current.lines().any(|line| line == row), "{row}: {current}");
    }
    for row in [
        "| Applicable with blocked proof | Blocked: Proof logs are unavailable. | This change requires the policy. |",
        "| Unavailable policy | Blocked | The selected definition is unavailable. |",
        "| Original Async Work | Original exclusion reason. |",
    ] {
        assert!(history.lines().any(|line| line == row), "{row}: {history}");
    }
    let mut removed = view.round.clone();
    removed.policy_declarations.remove(1);
    let snapshot = Snapshot {
        subject: removed.assessment.subject().clone(),
        reviewer: removed.assessment.reviewer().clone(),
        revision: removed.assessment.revision().clone(),
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::none(),
    };
    let absent = summary_view(&removed, &view.history, &snapshot)?;
    let body = Json2MdRenderer::new()?.summary(&absent)?;
    assert!(
        body.contains(
            "\n**Policies not declared in this round (1)**\n\n| Policy | Earlier applicable rounds |\n| --- | --- |\n| Original Async Work | — |\n"
        ),
        "{body}"
    );
    Ok(())
}

#[test]
fn declaration_labels_and_reasons_remain_text_inside_parsed_markdown_table_cells()
-> Result<(), Box<dyn Error>> {
    let body = Json2MdRenderer::new()?.summary(&declared_view()?)?;
    let mut text = String::new();
    let mut cells = 0;
    for event in Parser::new_ext(&body, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::TableCell) => cells += 1,
            Event::Html(value) | Event::InlineHtml(value) => {
                assert!(!value.contains("<script>"), "{value}")
            }
            Event::Start(Tag::Link { dest_url, .. }) => assert!(!dest_url.contains("team")),
            Event::Text(value) => text.push_str(&value),
            _ => {}
        }
    }
    assert_eq!(
        cells, 36,
        "pipes or newlines changed the table structure: {body}"
    );
    assert!(
        text.contains("Async | <script>@\u{200b}team\nwork"),
        "{text}"
    );
    assert!(
        text.contains("No | <script>@\u{200b}team\nasync work."),
        "{text}"
    );
    assert!(!body.contains("@team"));
    Ok(())
}
