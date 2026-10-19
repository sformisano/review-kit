mod support;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use review_kit_application::ports::*;
use review_kit_domain::*;
use review_kit_json2md_adapter::Json2MdRenderer;
use support::{
    sections::{groups, layout},
    *,
};

pub fn parsed_suggestions(body: &str) -> Vec<(String, String)> {
    let mut blocks = vec![];
    let mut block: Option<(String, String)> = None;
    for event in Parser::new(body) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.starts_with("suggestion") =>
            {
                block = Some((info.to_string(), String::new()))
            }
            Event::Text(text) => {
                if let Some((_, value)) = &mut block {
                    value.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(value) = block.take() {
                    blocks.push(value);
                }
            }
            _ => {}
        }
    }
    blocks
}

#[test]
fn finding_has_authored_body_and_severity_with_display_name_footer() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let finding = finding(None)?;
    let body = render_finding(&mut renderer, &finding)?;
    assert_eq!(body, include_str!("fixtures/finding.md"));
    assert_eq!(body.matches(finding.body()).count(), 1);
    assert!(!body.contains(finding.evidence()));
    assert!(body.ends_with("*P2 · Rust Error Contracts*\n"));
    assert!(!body.contains(finding.policy_id().as_str()));
    assert!(!body.contains("<!-- review-"));
    assert!(!body.contains("Co-Authored-By"));
    let request = serde_json::json!({"body":body});
    let round_trip: serde_json::Value = serde_json::from_str(&serde_json::to_string(&request)?)?;
    assert_eq!(round_trip["body"], request["body"]);
    Ok(())
}

#[test]
fn advisory_finding_renders_like_a_blocking_finding() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let original = finding(None)?;
    let advisory = copy_finding(
        &original,
        Disposition::Advisory,
        FindingStatus::Open,
        original.evidence(),
    )?;
    let body = render_finding(&mut renderer, &advisory)?;
    assert_eq!(body, render_finding(&mut renderer, &original)?);
    assert!(!body.to_lowercase().contains("advisory"));
    Ok(())
}

#[test]
fn suggestions_preserve_long_fences_multiline_and_deletion() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    for replacement in [
        vec!["new".into()],
        vec![
            "```rust".into(),
            "``````".into(),
            "/close".into(),
            "".into(),
            "last".into(),
        ],
        vec![],
        vec![String::new()],
    ] {
        let finding = finding(Some(replacement.clone()))?;
        let body = render_finding(&mut renderer, &finding)?;
        let intended = replacement.join("\n");
        let expected = if replacement.is_empty() {
            intended
        } else {
            format!("{intended}\n")
        };
        assert_eq!(
            parsed_suggestions(&body),
            vec![("suggestion:-1+1".into(), expected)]
        );
        assert!(body.ends_with("*P2 · Rust Error Contracts*\n"));
    }
    let original = finding(None)?;
    let suggestion = Suggestion::new(
        original.location().clone(),
        revision()?,
        original.location().line(),
        vec!["two".into()],
        vec!["replacement".into()],
        "Correction verified".into(),
    )?;
    let value = Finding::new(FindingDraft {
        id: original.id(),
        policy_id: original.policy_id().clone(),
        title: original.title().into(),
        body: original.body().into(),
        severity: original.severity(),
        disposition: original.disposition(),
        status: original.status(),
        evidence: original.evidence().into(),
        validated: true,
        location: original.location().clone(),
        suggestion: Some(suggestion),
    })?;
    assert_eq!(
        parsed_suggestions(&render_finding(&mut renderer, &value)?),
        vec![("suggestion".into(), "replacement\n".into())]
    );
    Ok(())
}

#[test]
fn transition_replies_open_with_the_status_outcome() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let value = finding(None)?;
    let context = context()?;
    for (previous, new, opening) in [
        (FindingStatus::Open, FindingStatus::Fixed, "**Fixed.**"),
        (
            FindingStatus::Open,
            FindingStatus::Withdrawn,
            "**Withdrawn.**",
        ),
        (FindingStatus::Fixed, FindingStatus::Open, "**Reopened.**"),
        (
            FindingStatus::Withdrawn,
            FindingStatus::Open,
            "**Reopened.**",
        ),
        (
            FindingStatus::Uncertain,
            FindingStatus::Open,
            "**Confirmed.**",
        ),
        (
            FindingStatus::Open,
            FindingStatus::Uncertain,
            "**Uncertain.**",
        ),
        (FindingStatus::Open, FindingStatus::Open, "**Updated.**"),
        (
            FindingStatus::Uncertain,
            FindingStatus::Uncertain,
            "**Updated.**",
        ),
    ] {
        let changed = copy_finding(&value, Disposition::Blocking, new, "Checked")?;
        assert_eq!(
            renderer.transition_context(TransitionRenderInput {
                finding: FindingRenderInput {
                    finding: &changed,
                    policy_name: POLICY_NAME,
                    context: &context
                },
                previous,
                explanation: "The `parse` result is kept."
            })?,
            format!("{opening} The `parse` result is kept.\n"),
            "{previous:?} to {new:?}"
        );
    }
    Ok(())
}

#[test]
fn multiline_transition_explanation_starts_its_own_paragraph() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let value = finding(None)?;
    let changed = copy_finding(
        &value,
        Disposition::Blocking,
        FindingStatus::Fixed,
        "Checked",
    )?;
    let body = renderer.transition_context(TransitionRenderInput {
        finding: FindingRenderInput {
            finding: &changed,
            policy_name: POLICY_NAME,
            context: &context()?,
        },
        previous: FindingStatus::Open,
        explanation: "- kept the error\n- added a regression test",
    })?;
    assert_eq!(
        body,
        "**Fixed.**\n\n- kept the error\n- added a regression test\n"
    );
    let items = Parser::new(&body)
        .filter(|event| matches!(event, Event::Start(Tag::Item)))
        .count();
    assert_eq!(items, 2);
    Ok(())
}

#[test]
fn trailing_spaces_do_not_break_title_or_footer_emphasis() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let value = finding(None)?;
    let body = renderer.finding_context(FindingRenderInput {
        finding: &value,
        policy_name: "Rust Error Contracts ",
        context: &context()?,
    })?;
    assert!(body.ends_with("*P2 · Rust Error Contracts*\n"), "{body}");
    Ok(())
}

#[test]
fn transition_and_answer_render_authored_text_with_structured_context() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let value = finding(None)?;
    let fixed = copy_finding(
        &value,
        Disposition::Blocking,
        FindingStatus::Fixed,
        "Regression passes",
    )?;
    let context = context()?;
    assert_eq!(
        renderer.transition_context(TransitionRenderInput {
            finding: FindingRenderInput {
                finding: &fixed,
                policy_name: POLICY_NAME,
                context: &context
            },
            previous: FindingStatus::Open,
            explanation: "Regression passes."
        })?,
        "**Fixed.** Regression passes.\n"
    );
    let view = summary()?;
    let (answer, target) = answer_parts(&view)?;
    assert_eq!(
        renderer.answer_context(AnswerRenderInput {
            answer: &answer,
            finding: &view.findings[0].root,
            current_finding: &view.findings[0].finding,
            target: &target,
            policy_name: POLICY_NAME,
            context: &context
        })?,
        format!("{}\n", answer.body)
    );
    Ok(())
}

#[test]
fn fenced_body_does_not_swallow_the_suggestion_or_footer() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let original = finding(Some(vec!["replacement".into()]))?;
    let value = Finding::new(FindingDraft {
        id: original.id(),
        policy_id: original.policy_id().clone(),
        title: original.title().into(),
        body: "The result disappears:\n\n```text\nobserved loss\n```".into(),
        severity: original.severity(),
        disposition: original.disposition(),
        status: original.status(),
        evidence: "```text\nprivate trace\n```".into(),
        validated: true,
        location: original.location().clone(),
        suggestion: original.suggestion().cloned(),
    })?;
    let body = render_finding(&mut renderer, &value)?;
    assert_eq!(
        parsed_suggestions(&body),
        vec![("suggestion:-1+1".into(), "replacement\n".into())]
    );
    assert!(!body.contains("private trace"));
    let mut in_code = false;
    let mut code_seen = false;
    let mut footer_seen = false;
    for event in Parser::new(&body) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(value) if value.contains("observed loss") => {
                assert!(in_code);
                code_seen = true;
            }
            Event::Text(value) if value.contains("Rust Error Contracts") => {
                assert!(!in_code);
                footer_seen = true;
            }
            _ => {}
        }
    }
    assert!(code_seen && footer_seen);
    Ok(())
}

#[test]
fn same_status_transition_shows_as_an_update_in_the_round_table() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let mut view = summary()?;
    let row = &mut view.findings[0];
    row.finding = copy_finding(
        &row.finding,
        Disposition::Blocking,
        FindingStatus::Open,
        "Moved",
    )?;
    row.actions = vec![
        review_kit_application::summary::PublishedAction::Transition {
            from: FindingStatus::Open,
            to: FindingStatus::Open,
        },
    ];
    let body = renderer.summary(&view)?;
    assert!(
        body.contains(" | P2 · Blocking | Open | Updated |\n"),
        "{body}"
    );
    assert!(!body.contains("Open → Open"));
    Ok(())
}

#[test]
fn summary_shows_decision_findings_details_and_immutable_historical_rows() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let view = summary()?;
    let body = renderer.summary(&view)?;
    let sections = layout(&body)?;
    assert_eq!(
        sections.outline(),
        [(0, "Round 1 · Changes required"), (0, "Review details")],
        "{body}"
    );
    assert!(sections.trailing.trim().is_empty(), "{body}");
    assert_eq!(
        sections.visible,
        concat!(
            "### Changes required\n\n",
            "Round 2 · 1 fixed\n\n",
            "Limitation: Review context is incomplete\n\n",
            "Some updates for this round are not posted yet.\n\n",
            "| ID | Finding | Severity | Status | This round |\n",
            "| --- | --- | --- | --- | --- |\n",
            "| F2 | [Preserve \\[result\\] \\*exactly\\*]",
            "(<https://gitlab.example/deployment/group/sub/project%20(test)/-/merge_requests/12#note_701>)",
            " | P2 · Blocking | Fixed | Open → Fixed, resolved |\n\n",
            "Not posted yet: F10\n\n",
        )
    );
    assert!(!body.contains("discussion-abc"));
    assert!(!body.contains("#note_10"));
    let details = sections.section("Review details")?;
    let rounds = groups(details);
    assert!(rounds.lead.trim().is_empty(), "{body}");
    assert_eq!(rounds.headings(), ["Round 2", "Round 1"], "{body}");
    let current = rounds.group("Round 2")?;
    let earlier = rounds.group("Round 1")?;
    let history = sections.section("Round 1 · Changes required")?;
    for line in [
        "- Attempted commit `111111111111` against `222222222222`",
        "- Scope: src/\\[lib\\].rs&#10;&lt;details&gt;",
        "- Depth: Standard &#124; focused",
        "- Workflow: codebase-policies-review, policy set rust-codebase-policies",
        "- Publishing: A publication effect has an uncertain outcome",
        "| Policy | Result | Open findings | Reason |",
        "| Rust Error Contracts | Fail | — | The changed error handling requires this policy. |",
        "| Rust Ownership Design | No ownership boundary changes. |",
    ] {
        assert!(
            current.lines().any(|value| value == line),
            "{line}: {current}"
        );
    }
    for line in [
        "- Reviewed commit `111111111111` against `222222222222`",
        "- Scope: src/\\[lib\\].rs&#10;&lt;details&gt;",
        "- Depth: Standard &#124; focused",
        "- Workflow: codebase-policies-review, policy set rust-codebase-policies",
        "- Limitation: Review context is incomplete",
        "| Policy | Result | Reason |",
        "| Rust Error Contracts | Fail | The changed error handling requires this policy. |",
    ] {
        assert!(
            earlier.lines().any(|value| value == line),
            "{line}: {earlier}"
        );
    }
    assert!(history.contains("| ID | Finding | Severity | Status | That round |"));
    assert!(history.contains("| P2 · Blocking | Open | Posted |"));
    assert!(!history.contains("| Fixed |"));
    assert!(
        history
            .lines()
            .all(|line| line.is_empty() || line.starts_with("| ")),
        "{history}"
    );
    assert!(!details.contains("| ID |"), "{details}");
    assert_eq!(body, renderer.summary(&view)?);
    Ok(())
}

#[test]
fn resolution_labels_distinguish_recovered_observations() -> TestResult {
    use review_kit_application::summary::PublishedAction;

    let mut renderer = Json2MdRenderer::new()?;
    for (action, label) in [
        (PublishedAction::ObservedResolved, "| resolved (observed) |"),
        (PublishedAction::ObservedReopened, "| reopened (observed) |"),
        (PublishedAction::Resolved, "| resolved |"),
        (PublishedAction::Reopened, "| reopened |"),
    ] {
        let mut view = summary()?;
        view.findings[0].actions = vec![action.clone()];
        let history = view.history[0]
            .communicated_findings
            .as_mut()
            .ok_or("missing historical findings")?;
        history[0].actions = vec![action];
        let body = renderer.summary(&view)?;
        let sections = layout(&body)?;
        let current = sections.visible;
        let retained = sections.section("Round 1 · Changes required")?;
        assert!(current.contains(label), "{label}: {current}");
        assert!(retained.contains(label), "{label}: {retained}");
    }
    Ok(())
}

#[test]
fn legacy_history_is_explicitly_unavailable_and_never_reconstructed() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let mut view = summary()?;
    view.history[0].communicated_findings = None;
    let body = renderer.summary(&view)?;
    let history = body
        .split_once("<summary>Round 1")
        .ok_or("missing historical section")?
        .1;
    assert!(history.contains("\nFindings for this round were not recorded.\n"));
    assert!(!history.contains("| F2 |"));
    assert!(!history.contains("No findings were posted."));
    Ok(())
}

#[test]
fn undeclared_policy_without_a_retained_name_shows_its_id() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let mut view = summary()?;
    view.policies[0].name = None;
    view.policies[0].declarations.clear();
    let body = renderer.summary(&view)?;
    assert!(
        body.contains("\n| policy&#124;\\[name\\] | 1, 2 |\n"),
        "{body}"
    );
    Ok(())
}

#[test]
fn invalid_note_identity_and_missing_trusted_context_fail() -> TestResult {
    let mut renderer = Json2MdRenderer::new()?;
    let value = finding(None)?;
    assert!(renderer.finding(&value).is_err());
    assert!(
        renderer
            .finding_context(FindingRenderInput {
                finding: &value,
                policy_name: "",
                context: &context()?
            })
            .is_err()
    );
    let mut view = summary()?;
    view.findings[0].root.root_note =
        review_kit_application::publication::ProviderId::new("not-a-note".into())?;
    assert!(renderer.summary(&view).is_err());
    Ok(())
}
