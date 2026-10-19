mod support;

use std::{error::Error, fs, path::Path};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use review_kit_application::{
    ports::ReviewRenderer,
    publication::{Abandonment, Round, RoundStatus},
    summary::{FindingCounts, PublishedAction, SummaryView},
};
use review_kit_domain::*;
use review_kit_json2md_adapter::Json2MdRenderer;
use support::{
    TestResult,
    coverage::{self, Declared, Outcome},
    sections::{blocks, groups, layout},
};

fn render(view: &SummaryView) -> Result<String, Box<dyn Error>> {
    Ok(Json2MdRenderer::new()?.summary(view)?)
}

fn details(body: &str) -> Result<&str, Box<dyn Error>> {
    Ok(layout(body)?.section("Review details")?)
}

fn row<'a>(body: &'a str, start: &str) -> Result<&'a str, Box<dyn Error>> {
    let mut rows = body.lines().filter(|line| line.starts_with(start));
    let row = rows
        .next()
        .ok_or_else(|| format!("missing {start}: {body}"))?;
    assert!(rows.next().is_none(), "repeated {start}: {body}");
    Ok(row)
}

/// Rows of every parsed table, each as its cells' decoded text.
fn tables(body: &str) -> Vec<Vec<Vec<String>>> {
    let mut tables = Vec::new();
    let mut cell = None::<String>;
    for event in Parser::new_ext(body, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Table(_)) => tables.push(Vec::new()),
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(table) = tables.last_mut() {
                    table.push(Vec::new());
                }
            }
            Event::Start(Tag::TableCell) => cell = Some(String::new()),
            Event::End(TagEnd::TableCell) => {
                if let (Some(text), Some(row)) =
                    (cell.take(), tables.last_mut().and_then(|t| t.last_mut()))
                {
                    row.push(text);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(cell) = cell.as_mut() {
                    cell.push_str(&text);
                }
            }
            _ => {}
        }
    }
    tables
}

#[test]
fn realistic_blocked_review_presents_each_declared_policy_once() -> TestResult {
    let body = render(&coverage::realistic()?)?;
    let sections = layout(&body)?;
    assert_eq!(sections.outline(), [(0, "Review details")], "{body}");
    assert!(sections.trailing.trim().is_empty(), "{body}");
    let (visible, details) = (sections.visible, sections.section("Review details")?);
    assert!(
        visible.starts_with(&format!(
            "### Blocked\n\nRound 1 · 3 open\n\nLimitation: {}\n\n| ID | Finding |",
            coverage::LIMITATION
        )),
        "{visible}"
    );
    for (id, title) in [
        (1, "Retain the job after a failed send"),
        (2, "Return a typed error when the retry limit is reached"),
        (3, "Reject a zero retry limit at construction"),
    ] {
        let line = row(visible, &format!("| F{id} | "))?;
        assert!(
            line.contains(&format!(
                "[{title}](<https://gitlab.example/team/review-playground/-/merge_requests/33#note_2788570{id}>)"
            )),
            "{line}"
        );
        assert!(line.ends_with(" · Blocking | Open | Posted |"), "{line}");
    }

    assert!(details.contains("\n- Reviewed commit `4f2c1e9a7b3d` against `0a1b2c3d4e5f`\n"));
    assert!(details.contains(&format!("\n- Scope: {}\n", coverage::SCOPE)));
    assert!(details.contains(&format!("\n- Depth: {}\n", coverage::DEPTH)));
    assert!(details.contains(
        "\n**Applicable policies (16)**\n\n| Policy | Result | Open findings | Reason |\n"
    ));
    assert!(details.contains("\n**Excluded policies (13)**\n\n| Policy | Reason |\n"));
    for absent in [
        "unresolved",
        "not declared",
        "Not checked",
        "was not checked",
        " applies:",
        "| Yes",
        "| No: ",
    ] {
        assert!(!body.contains(absent), "{absent}: {body}");
    }
    for policy in coverage::realistic_policies() {
        assert_eq!(
            body.matches(policy.name).count(),
            1,
            "{}: {body}",
            policy.name
        );
        assert_eq!(
            body.matches(policy.reason).count(),
            1,
            "{}: {body}",
            policy.reason
        );
        let line = row(details, &format!("| {} | ", policy.name))?;
        let expected = match policy.outcome {
            Outcome::Excluded => format!("| {} | {} |", policy.name, policy.reason),
            Outcome::Satisfied => format!("| {} | Pass | — | {} |", policy.name, policy.reason),
            Outcome::Blocked(limitation) => {
                format!(
                    "| {} | Blocked: {limitation} | — | {} |",
                    policy.name, policy.reason
                )
            }
            Outcome::Violated if policy.id == "rust-error-contracts" => {
                format!("| {} | Fail | F1, F2 | {} |", policy.name, policy.reason)
            }
            Outcome::Violated => format!("| {} | Fail | F3 | {} |", policy.name, policy.reason),
            Outcome::Unresolved(_) => {
                return Err("the realistic round has no unresolved policy".into());
            }
        };
        assert_eq!(line, expected);
    }
    assert_eq!(body.matches(coverage::PROOF_GAP).count(), 1);
    assert_eq!(body.matches(coverage::LIMITATION).count(), 1);
    assert!(!body.contains("Private violation evidence"));

    let parsed = tables(&body);
    assert_eq!(
        parsed.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![4, 17, 14],
        "findings, applicable and excluded tables with their header rows"
    );
    assert!(
        parsed
            .iter()
            .all(|table| table.iter().all(|row| row.len() == table[0].len()))
    );
    Ok(())
}

#[test]
fn round_without_findings_keeps_its_complete_coverage() -> TestResult {
    let mut policies = coverage::realistic_policies();
    for policy in &mut policies {
        if !matches!(policy.outcome, Outcome::Excluded) {
            policy.outcome = Outcome::Satisfied;
        }
    }
    let current = coverage::round(
        1,
        RoundStatus::Completed,
        ReviewDecision::Ready,
        None,
        &policies,
        vec![],
    )?;
    let body = render(&coverage::view(&current, &[], vec![])?)?;
    assert!(
        body.starts_with("### Ready\n\nRound 1 · no findings\n\n<details>\n"),
        "{body}"
    );
    assert!(!body.contains("| ID |"));
    assert!(!body.contains("Limitation:"));
    assert!(body.contains("\n- Reviewed commit `4f2c1e9a7b3d` against `0a1b2c3d4e5f`\n"));
    assert!(body.contains("**Applicable policies (16)**"));
    assert!(body.contains("**Excluded policies (13)**"));
    assert_eq!(body.matches(" | Pass | — | ").count(), 16);
    Ok(())
}

#[test]
fn each_blocked_evaluation_is_labeled_blocked_with_its_limitation_once() -> TestResult {
    const OVERALL: &str = "Proof and two policy definitions are unavailable.";
    const DEFINITION: &str = "The selected definition could not be read.";
    let policies = [
        Declared {
            id: "proof",
            name: "Proof",
            reason: "Every change needs proof.",
            outcome: Outcome::Blocked("No CI pipeline ran for this head."),
        },
        Declared {
            id: "integrity",
            name: "Integrity",
            reason: "Every change needs integrity checks.",
            outcome: Outcome::Blocked(OVERALL),
        },
        Declared {
            id: "satisfied",
            name: "Satisfied",
            reason: "The change touches this area.",
            outcome: Outcome::Satisfied,
        },
        Declared {
            id: "unreadable",
            name: "Unreadable",
            reason: DEFINITION,
            outcome: Outcome::Unresolved(DEFINITION),
        },
        Declared {
            id: "unlisted",
            name: "Unlisted",
            reason: "The policy set lists it without a description.",
            outcome: Outcome::Unresolved(
                "Its applicability depends on an unavailable service map.",
            ),
        },
        Declared {
            id: "excluded",
            name: "Excluded",
            reason: "No data is persisted.",
            outcome: Outcome::Excluded,
        },
    ];
    let current = coverage::round(
        1,
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some(OVERALL),
        &policies,
        vec![],
    )?;
    let body = render(&coverage::view(&current, &[], vec![])?)?;
    let details = details(&body)?;
    assert!(body.starts_with(&format!(
        "### Blocked\n\nRound 1 · no findings\n\nLimitation: {OVERALL}\n"
    )));
    assert!(details.contains("\n**Applicable policies (3)**\n"));
    assert!(details.contains("\n**Policies with unresolved applicability (2)**\n\n| Policy | Result | Open findings | Reason |\n"));
    assert!(details.contains("\n**Excluded policies (1)**\n"));
    for line in [
        "| Proof | Blocked: No CI pipeline ran for this head. | — | Every change needs proof. |",
        "| Integrity | Blocked: see the limitation above | — | Every change needs integrity checks. |",
        "| Satisfied | Pass | — | The change touches this area. |",
        "| Unreadable | Blocked | — | The selected definition could not be read. |",
        "| Unlisted | Blocked: Its applicability depends on an unavailable service map. | — | The policy set lists it without a description. |",
        "| Excluded | No data is persisted. |",
    ] {
        assert!(
            details.lines().any(|value| value == line),
            "{line}: {details}"
        );
    }
    for text in [
        OVERALL,
        DEFINITION,
        "No CI pipeline ran for this head.",
        "Its applicability depends on an unavailable service map.",
    ] {
        assert_eq!(body.matches(text).count(), 1, "{text}: {body}");
    }
    assert!(!body.contains("Not checked"));
    Ok(())
}

#[test]
fn unfinished_publication_keeps_the_compact_layout_and_unverified_findings_unlinked() -> TestResult
{
    let findings = coverage::realistic_findings()?;
    for (status, line) in [
        (RoundStatus::Pending, "This round has not finished posting."),
        (
            RoundStatus::Partial,
            "Some updates for this round are not posted yet.",
        ),
    ] {
        let current = coverage::round(
            1,
            status,
            ReviewDecision::Blocked,
            Some(coverage::LIMITATION),
            &coverage::realistic_policies(),
            findings.clone(),
        )?;
        let body = render(&coverage::view(
            &current,
            &[],
            vec![coverage::root(&findings[0], 1)?],
        )?)?;
        assert!(
            body.starts_with("### Blocked\n\nRound 1 · 1 open\n"),
            "{body}"
        );
        assert_eq!(body.matches(line).count(), 1, "{body}");
        assert!(body.contains("#note_27885701>) | P2 · Blocking | Open | Posted |\n"));
        assert!(body.contains("\nNot posted yet: F2, F3\n"));
        assert!(!body.contains("#note_27885702") && !body.contains("#note_27885703"));
        assert!(body.contains("\n- Attempted commit "));
        assert!(row(&body, "| Rust Error Contracts | ")?.contains(" | Fail | F1 | "));
        assert!(row(&body, "| Rust Invariant Preservation | ")?.contains(" | Fail | — | "));
        assert!(
            body.contains("**Applicable policies (16)**")
                && body.contains("**Excluded policies (13)**")
        );
    }
    Ok(())
}

fn history_round(
    number: u64,
    policies: &[Declared],
    findings: Vec<Finding>,
    rows: Vec<(Finding, Vec<PublishedAction>)>,
) -> Result<Round, Box<dyn Error>> {
    let mut round = coverage::round(
        number,
        RoundStatus::Completed,
        ReviewDecision::ChangesRequired,
        None,
        policies,
        findings,
    )?;
    round.communicated_findings = Some(coverage::communicated(rows)?);
    Ok(round)
}

/// Round 3 after round 1 posted three findings and round 2 fixed one and
/// withdrew another; round 3 reopens the fixed one, fixes the third and
/// excludes the state policy that rounds 1 and 2 applied.
fn three_rounds() -> Result<SummaryView, Box<dyn Error>> {
    use FindingStatus::*;
    let error = |status| {
        coverage::with_status(
            1,
            "rust-error-contracts",
            "Retain the job after a failed send",
            Severity::P2,
            88,
            status,
        )
    };
    let typed = |status| {
        support::copy_finding(
            &coverage::with_status(
                2,
                "rust-error-contracts",
                "Return a typed error",
                Severity::P4,
                112,
                status,
            )?,
            Disposition::Advisory,
            status,
            "The caller cannot tell a full queue from a lost job.",
        )
    };
    let shared = |status| {
        coverage::with_status(
            3,
            "state-coherence-policy",
            "Keep one owner for a retried job",
            Severity::P3,
            41,
            status,
        )
    };
    let applies = Declared {
        id: "state-coherence-policy",
        name: "State Coherence Policy",
        reason: "A job can exist in two places during a retry.",
        outcome: Outcome::Violated,
    };
    let excluded = Declared {
        reason: "The change no longer shares job state.",
        outcome: Outcome::Excluded,
        ..applies
    };
    let errors = Declared {
        id: "rust-error-contracts",
        name: "Rust Error Contracts",
        reason: "The send path is fallible.",
        outcome: Outcome::Violated,
    };
    let claims = Declared {
        id: "technical-claims-policy",
        name: "Technical Claims Policy",
        reason: "The README is unchanged.",
        outcome: Outcome::Excluded,
    };
    let created = vec![PublishedAction::Created];
    let first = history_round(
        1,
        &[errors, applies, claims],
        vec![error(Open)?, typed(Open)?, shared(Open)?],
        vec![
            (error(Open)?, created.clone()),
            (typed(Open)?, created.clone()),
            (shared(Open)?, created),
        ],
    )?;
    let fixed = |to| {
        vec![
            PublishedAction::Transition { from: Open, to },
            PublishedAction::Resolved,
        ]
    };
    let second = history_round(
        2,
        &[errors, applies, claims],
        vec![error(Fixed)?, typed(Withdrawn)?, shared(Open)?],
        vec![
            (error(Fixed)?, fixed(Fixed)),
            (typed(Withdrawn)?, fixed(Withdrawn)),
            (shared(Open)?, vec![]),
        ],
    )?;
    let current = coverage::round(
        3,
        RoundStatus::Completed,
        ReviewDecision::ChangesRequired,
        None,
        &[errors, excluded, claims],
        vec![error(Open)?, typed(Withdrawn)?, shared(Fixed)?],
    )?;
    let roots = [error(Open)?, typed(Open)?, shared(Open)?]
        .iter()
        .map(|finding| coverage::root(finding, 1))
        .collect::<Result<_, _>>()?;
    let mut view = coverage::view(&current, &[first, second], roots)?;
    for (row, (finding, actions)) in view.findings.iter_mut().zip([
        (
            error(Open)?,
            vec![
                PublishedAction::Transition {
                    from: Fixed,
                    to: Open,
                },
                PublishedAction::Reopened,
            ],
        ),
        (typed(Withdrawn)?, vec![]),
        (shared(Fixed)?, fixed(Fixed)),
    ]) {
        row.finding = finding;
        row.actions = actions;
    }
    view.counts = FindingCounts {
        total: 3,
        open: 1,
        fixed: 1,
        withdrawn: 1,
        uncertain: 0,
    };
    Ok(view)
}

#[test]
fn each_earlier_round_keeps_its_finding_states_and_its_own_coverage_group() -> TestResult {
    let body = render(&three_rounds()?)?;
    let sections = layout(&body)?;
    assert_eq!(
        sections.outline(),
        [
            (0, "Round 1 · Changes required"),
            (0, "Round 2 · Changes required"),
            (0, "Review details"),
        ],
        "{body}"
    );
    assert!(sections.trailing.trim().is_empty(), "{body}");

    let visible = sections.visible;
    assert!(
        visible.starts_with("### Changes required\n\nRound 3 · 1 open · 1 fixed · 1 withdrawn\n"),
        "{body}"
    );
    assert!(row(visible, "| F1 | ")?.ends_with(" | Open | Fixed → Open, reopened |"));
    assert!(row(visible, "| F2 | ")?.ends_with(" | P4 · Advisory | Withdrawn | — |"));
    assert!(row(visible, "| F3 | ")?.ends_with(" | Fixed | Open → Fixed, resolved |"));
    assert!(!visible.contains("| Policy |"), "{visible}");

    let details = sections.section("Review details")?;
    assert!(!details.contains("| ID |"), "{details}");
    let rounds = groups(details);
    assert!(rounds.lead.trim().is_empty(), "{body}");
    assert_eq!(
        rounds.headings(),
        ["Round 3", "Round 1", "Round 2"],
        "{body}"
    );
    let current = rounds.group("Round 3")?;
    assert!(current.contains(&format!("\n- Scope: {}\n", coverage::SCOPE)));
    assert!(current.contains(&format!("\n- Depth: {}\n", coverage::DEPTH)));
    assert!(
        current.contains("\n**Applicable policies (1)**\n"),
        "{current}"
    );
    assert!(current.contains("\n**Excluded policies (2)**\n\n| Policy | Reason |\n"));
    assert_eq!(
        row(current, "| Rust Error Contracts | ")?,
        "| Rust Error Contracts | Fail | F1 | The send path is fallible. |"
    );
    assert_eq!(
        row(current, "| State Coherence Policy | ")?,
        "| State Coherence Policy | The change no longer shares job state. |"
    );
    assert!(!current.contains("**Policies not declared"));

    for number in ["Round 1", "Round 2"] {
        let group = rounds.group(number)?;
        assert!(group.contains("\n- Reviewed commit `4f2c1e9a7b3d` against `0a1b2c3d4e5f`\n"));
        assert!(group.contains(
            "\n- Workflow: codebase-policies-review, policy set rust-codebase-policies\n"
        ));
        assert!(group.contains("\n**Applicable policies (2)**\n\n| Policy | Result | Reason |\n"));
        assert!(group.contains("\n**Excluded policies (1)**\n"));
        assert_eq!(
            row(group, "| State Coherence Policy | ")?,
            "| State Coherence Policy | Fail | A job can exist in two places during a retry. |"
        );
        assert_eq!(
            row(group, "| Technical Claims Policy | ")?,
            "| Technical Claims Policy | The README is unchanged. |"
        );
    }

    let first = sections.section("Round 1 · Changes required")?;
    let second = sections.section("Round 2 · Changes required")?;
    for section in [first, second] {
        assert_eq!(
            section
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.starts_with("| F"))
                .collect::<Vec<_>>(),
            [false, false, true, true, true],
            "only the header, the delimiter and three rows: {section}"
        );
    }
    assert!(row(second, "| F1 | ")?.ends_with(" | Fixed | Open → Fixed, resolved |"));
    assert!(
        row(second, "| F2 | ")?
            .ends_with(" | P4 · Advisory | Withdrawn | Open → Withdrawn, resolved |")
    );
    assert!(row(second, "| F3 | ")?.ends_with(" | Open | — |"));
    for id in 1..=3 {
        let line = row(first, &format!("| F{id} | "))?;
        assert!(line.ends_with(" | Open | Posted |"), "{line}");
        assert!(
            line.contains(&format!(
                "(<https://gitlab.example/team/review-playground/-/merge_requests/33#note_2788570{id}>)"
            )),
            "{line}"
        );
    }
    Ok(())
}

#[test]
fn second_completed_round_keeps_round_one_beside_review_details() -> TestResult {
    use FindingStatus::*;
    let error = |status| {
        coverage::with_status(
            1,
            "rust-error-contracts",
            "Retain the job after a failed send",
            Severity::P2,
            88,
            status,
        )
    };
    let errors = Declared {
        id: "rust-error-contracts",
        name: "Rust Error Contracts",
        reason: "The send path is fallible.",
        outcome: Outcome::Violated,
    };
    let first = history_round(
        1,
        &[errors],
        vec![error(Open)?],
        vec![(error(Open)?, vec![PublishedAction::Created])],
    )?;
    let current = coverage::round(
        2,
        RoundStatus::Completed,
        ReviewDecision::Ready,
        None,
        &[Declared {
            outcome: Outcome::Satisfied,
            ..errors
        }],
        vec![error(Fixed)?],
    )?;
    let mut view = coverage::view(&current, &[first], vec![coverage::root(&error(Open)?, 1)?])?;
    view.findings[0].finding = error(Fixed)?;
    view.findings[0].actions = vec![
        PublishedAction::Transition {
            from: Open,
            to: Fixed,
        },
        PublishedAction::Resolved,
    ];
    view.counts = FindingCounts {
        total: 1,
        fixed: 1,
        ..FindingCounts::default()
    };
    let body = render(&view)?;
    let sections = layout(&body)?;
    assert_eq!(
        sections.outline(),
        [(0, "Round 1 · Changes required"), (0, "Review details")],
        "{body}"
    );
    assert!(sections.trailing.trim().is_empty(), "{body}");
    assert!(
        sections
            .visible
            .starts_with("### Ready\n\nRound 2 · 1 fixed\n"),
        "{body}"
    );
    assert!(row(sections.visible, "| F1 | ")?.ends_with(" | Fixed | Open → Fixed, resolved |"));

    let first = sections.section("Round 1 · Changes required")?;
    assert!(!first.contains("| Rust Error Contracts | "), "{first}");
    let line = row(first, "| F1 | ")?;
    assert!(
        line.ends_with(" | P2 · Blocking | Open | Posted |"),
        "{line}"
    );
    assert!(line.contains("#note_27885701>)"), "{line}");

    let details = sections.section("Review details")?;
    assert!(!details.contains("| ID |"), "{details}");
    let rounds = groups(details);
    assert_eq!(rounds.headings(), ["Round 2", "Round 1"], "{body}");
    let current = rounds.group("Round 2")?;
    assert_eq!(
        blocks(current).items[0],
        "Reviewed commit 4f2c1e9a7b3d against 0a1b2c3d4e5f"
    );
    assert!(current.contains(&format!("\n- Scope: {}\n", coverage::SCOPE)));
    assert!(current.contains(&format!("\n- Depth: {}\n", coverage::DEPTH)));
    assert_eq!(
        row(current, "| Rust Error Contracts | ")?,
        "| Rust Error Contracts | Pass | — | The send path is fallible. |"
    );
    assert_eq!(
        row(rounds.group("Round 1")?, "| Rust Error Contracts | ")?,
        "| Rust Error Contracts | Fail | The send path is fallible. |"
    );
    Ok(())
}

#[test]
fn every_round_stays_a_top_level_section_whatever_its_publication_status() -> TestResult {
    const STOPPED: &str = "A manual review replaced this round.";
    let errors = Declared {
        id: "rust-error-contracts",
        name: "Rust Error Contracts",
        reason: "The send path is fallible.",
        outcome: Outcome::Violated,
    };
    let finding = coverage::open_finding(
        1,
        "rust-error-contracts",
        "Retain the job after a failed send",
        Severity::P2,
        88,
    )?;
    let mut abandoned = history_round(1, &[errors], vec![finding.clone()], vec![])?;
    abandoned.status = RoundStatus::Abandoned;
    abandoned.abandonment = Some(Abandonment {
        reason: STOPPED.into(),
        verified_actions: vec![],
    });
    let mut superseded = history_round(
        2,
        &[errors],
        vec![finding.clone()],
        vec![(finding.clone(), vec![PublishedAction::Created])],
    )?;
    superseded.status = RoundStatus::Superseded;
    let blocked = coverage::round(
        3,
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some("Proof is unavailable."),
        &[errors],
        vec![finding.clone()],
    )?;
    let history = [abandoned, superseded, blocked];
    for (status, line, kind) in [
        (
            RoundStatus::Pending,
            Some("This round has not finished posting."),
            "Attempted",
        ),
        (
            RoundStatus::Partial,
            Some("Some updates for this round are not posted yet."),
            "Attempted",
        ),
        (RoundStatus::Completed, None, "Reviewed"),
    ] {
        let current = coverage::round(
            4,
            status,
            ReviewDecision::ChangesRequired,
            None,
            &[errors],
            vec![finding.clone()],
        )?;
        let body = render(&coverage::view(
            &current,
            &history,
            vec![coverage::root(&finding, 2)?],
        )?)?;
        let sections = layout(&body)?;
        assert_eq!(
            sections.outline(),
            [
                (0, "Round 1 · Changes required · abandoned"),
                (0, "Round 2 · Changes required · superseded"),
                (0, "Round 3 · Blocked"),
                (0, "Review details"),
            ],
            "{status:?}: {body}"
        );
        assert!(sections.trailing.trim().is_empty(), "{body}");
        if let Some(line) = line {
            assert_eq!(body.matches(line).count(), 1, "{body}");
            assert!(sections.visible.contains(&format!("\n{line}\n")), "{body}");
        }
        let rounds = groups(sections.section("Review details")?);
        assert_eq!(
            rounds.headings(),
            ["Round 4", "Round 1", "Round 2", "Round 3"],
            "{status:?}: {body}"
        );
        assert!(
            blocks(rounds.group("Round 4")?).items[0]
                .starts_with(&format!("{kind} commit 4f2c1e9a7b3d")),
            "{status:?}: {body}"
        );

        let first = sections.section("Round 1 · Changes required · abandoned")?;
        assert_eq!(first.trim(), "No findings were posted.", "{body}");
        let first = rounds.group("Round 1")?;
        assert!(first.contains("\n- Attempted commit `4f2c1e9a7b3d`"));
        assert!(first.contains(&format!("\n- Stopped: {STOPPED}\n")));
        assert_eq!(body.matches(STOPPED).count(), 1, "{body}");
        let second = sections.section("Round 2 · Changes required · superseded")?;
        assert!(row(second, "| F1 | ")?.ends_with(" | Open | Posted |"));
        assert!(
            rounds
                .group("Round 2")?
                .contains("\n- Attempted commit `4f2c1e9a7b3d`")
        );
        let third = sections.section("Round 3 · Blocked")?;
        assert_eq!(
            third.trim(),
            "Findings for this round were not recorded.",
            "{body}"
        );
        let third = rounds.group("Round 3")?;
        assert!(third.contains("\n- Reviewed commit `4f2c1e9a7b3d`"));
        assert!(third.contains("\n- Limitation: Proof is unavailable.\n"));
    }
    Ok(())
}

#[test]
fn long_and_markdown_sensitive_authored_text_stays_whole_inside_its_cell() -> TestResult {
    const LONG: &str = "The change rewrites how the worker hands a failed job back to the retry queue, so this policy decides whether the queue, the worker and the README still agree. The queue now owns the job between attempts, the worker only borrows it for one send, and the README promises at most five attempts with exponential back-off starting at one second. Each of those claims needs a matching code path: the ownership transfer in RetryQueue::push, the attempt counter in RetryQueue::next, the back-off calculation in Backoff::delay and the acknowledgement in Worker::run. The reviewer also compared the new integration test with the documented limit, because a test that stops after three attempts would pass while the documented fifth attempt silently disappears. None of this text is shortened by the renderer; the cell keeps every clause the agent wrote, including this final sentence.";
    let hostile = "Uses `code`, *emphasis*, a | pipe,\nnew line, <details>, @team and [a link](https://elsewhere.example).";
    let policies = [
        Declared {
            id: "long",
            name: "Long | Reason",
            reason: LONG,
            outcome: Outcome::Blocked(hostile),
        },
        Declared {
            id: "hostile",
            name: "Hostile <b>name</b>",
            reason: hostile,
            outcome: Outcome::Excluded,
        },
    ];
    let current = coverage::round(
        1,
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some(LONG),
        &policies,
        vec![],
    )?;
    let body = render(&coverage::view(&current, &[], vec![])?)?;
    let parsed = tables(&body);
    assert_eq!(parsed.len(), 2, "{body}");
    assert_eq!(
        parsed[0][1],
        vec![
            "Long | Reason".to_owned(),
            format!("Blocked: {}", hostile.replace('@', "@\u{200b}")),
            "—".into(),
            LONG.to_owned()
        ]
    );
    assert_eq!(
        parsed[1][1],
        vec![
            "Hostile <b>name</b>".to_owned(),
            hostile.replace('@', "@\u{200b}")
        ]
    );
    for event in Parser::new_ext(&body, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                return Err(format!("authored text became a link: {dest_url}").into());
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                assert!(
                    ["<details>", "<summary>", "</summary>", "</details>"]
                        .iter()
                        .any(|tag| html.trim().starts_with(tag)),
                    "{html}"
                );
            }
            _ => {}
        }
    }
    assert_eq!(body.matches("<details>").count(), 1);
    assert!(!body.contains("@team"));
    Ok(())
}

#[test]
fn existing_custom_profiles_render_the_realistic_review() -> TestResult {
    let view = coverage::realistic()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for manifest in [
        root.join("../../../../examples/render-profiles/compact/profile.json"),
        root.join("tests/fixtures/v1-profile/profile.json"),
    ] {
        let body = Json2MdRenderer::from_profile(&manifest)?.summary(&view)?;
        assert!(
            body.contains("Rust Error Contracts"),
            "{}: {body}",
            manifest.display()
        );
        assert!(
            body.contains("#note_27885701"),
            "{}: {body}",
            manifest.display()
        );
    }
    let directory = tempfile::TempDir::new()?;
    let exported = Json2MdRenderer::export_default(directory.path())?;
    assert_eq!(
        Json2MdRenderer::from_profile(&exported)?.summary(&view)?,
        render(&view)?
    );
    Ok(())
}

#[test]
fn custom_profile_keeps_its_own_round_layout() -> TestResult {
    const NESTED: &str = "<details>\n<summary>Team details</summary>\n{% for old in history %}\n<details>\n<summary>Round {{ old.number }}</summary>\n\n{{ old.decision }}\n</details>\n{% endfor %}\n</details>\n";
    let directory = tempfile::TempDir::new()?;
    let manifest = Json2MdRenderer::export_default(directory.path())?;
    fs::write(directory.path().join("summary.j2"), NESTED)?;
    let mut renderer = Json2MdRenderer::from_profile(&manifest)?;
    assert_ne!(
        renderer.identity().fingerprint,
        Json2MdRenderer::new()?.identity().fingerprint
    );
    let body = renderer.summary(&three_rounds()?)?;
    assert_eq!(
        layout(&body)?.outline(),
        [(0, "Team details"), (1, "Round 2"), (1, "Round 1")],
        "{body}"
    );
    Ok(())
}
