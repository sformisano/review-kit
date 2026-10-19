//! The default summary gives every round the same section: its number,
//! decision and findings table. One collapsed Review details section at the
//! bottom holds each round's commit, scope, depth, workflow and policy tables
//! under a heading per round, the current round first.

mod support;

use std::error::Error;

use FindingStatus::*;
use review_kit_application::{
    ports::ReviewRenderer,
    publication::{PublicationLimitation, Round, RoundStatus},
    summary::{FindingCounts, PublishedAction, SummaryFinding, SummaryView},
};
use review_kit_domain::{Finding, FindingStatus, ReviewDecision, Severity};
use review_kit_json2md_adapter::Json2MdRenderer;
use support::{
    TestResult,
    coverage::{self, Declared, Outcome},
    sections::{Blocks, blocks, groups, layout},
};

const EARLIER_LIMITATION: &str = "Build and test proof is unavailable for this head.";
const CURRENT_LIMITATION: &str = "The retry test cannot run without the queue service.";
const REVIEWED: &str = "Reviewed commit 4f2c1e9a7b3d against 0a1b2c3d4e5f";
const ATTEMPTED: &str = "Attempted commit 4f2c1e9a7b3d against 0a1b2c3d4e5f";
const WORKFLOW: &str = "Workflow: codebase-policies-review, policy set rust-codebase-policies";

const ERRORS: Declared = Declared {
    id: "rust-error-contracts",
    name: "Rust Error Contracts",
    reason: "The send path is fallible.",
    outcome: Outcome::Violated,
};
const SHARED: Declared = Declared {
    id: "state-coherence-policy",
    name: "State Coherence Policy",
    reason: "A job can exist in two places during a retry.",
    outcome: Outcome::Violated,
};
const UNSHARED: Declared = Declared {
    reason: "The change no longer shares job state.",
    outcome: Outcome::Excluded,
    ..SHARED
};
const PROOF: Declared = Declared {
    id: "baseline-proof-policy",
    name: "Baseline Proof Policy",
    reason: "Every changed surface needs build and test proof.",
    outcome: Outcome::Blocked(EARLIER_LIMITATION),
};
const CLAIMS: Declared = Declared {
    id: "technical-claims-policy",
    name: "Technical Claims Policy",
    reason: "The README is unchanged.",
    outcome: Outcome::Excluded,
};

/// Findings with the actions that a round recorded for them.
type Rows = Vec<(Finding, Vec<PublishedAction>)>;

fn render(view: &SummaryView) -> Result<String, Box<dyn Error>> {
    Ok(Json2MdRenderer::new()?.summary(view)?)
}

fn retained(status: FindingStatus) -> Result<Finding, Box<dyn Error>> {
    coverage::with_status(
        1,
        ERRORS.id,
        "Retain the job after a failed send",
        Severity::P2,
        88,
        status,
    )
}

fn owner(status: FindingStatus) -> Result<Finding, Box<dyn Error>> {
    coverage::with_status(
        2,
        SHARED.id,
        "Keep one owner for a retried job",
        Severity::P3,
        41,
        status,
    )
}

fn typed(status: FindingStatus) -> Result<Finding, Box<dyn Error>> {
    coverage::with_status(
        3,
        ERRORS.id,
        "Return a typed error when the retry limit is reached",
        Severity::P2,
        112,
        status,
    )
}

/// The table that round 1 recorded after posting F1 and F2.
fn posted() -> Result<Rows, Box<dyn Error>> {
    Ok(vec![
        (retained(Open)?, vec![PublishedAction::Created]),
        (owner(Open)?, vec![PublishedAction::Created]),
    ])
}

/// Round 1, blocked on build proof with F1 and F2 open; `rows` is the
/// findings table it recorded, or `None` when no table was retained.
fn first(rows: Option<Rows>) -> Result<Round, Box<dyn Error>> {
    let mut round = coverage::round(
        1,
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some(EARLIER_LIMITATION),
        &[ERRORS, SHARED, PROOF, CLAIMS],
        vec![retained(Open)?, owner(Open)?],
    )?;
    round.communicated_findings = rows.map(coverage::communicated).transpose()?;
    Ok(round)
}

/// Round 2 fixes F1, withdraws F2 as its policy becomes excluded, adds F3 and
/// ends the proof policy with `proof`.
fn second(
    status: RoundStatus,
    decision: ReviewDecision,
    limitation: Option<&str>,
    proof: Outcome,
) -> Result<Round, Box<dyn Error>> {
    coverage::round(
        2,
        status,
        decision,
        limitation,
        &[
            ERRORS,
            UNSHARED,
            Declared {
                outcome: proof,
                ..PROOF
            },
            CLAIMS,
        ],
        vec![retained(Fixed)?, owner(Withdrawn)?, typed(Open)?],
    )
}

fn counts(rows: &[SummaryFinding]) -> FindingCounts {
    let mut counts = FindingCounts {
        total: rows.len(),
        ..FindingCounts::default()
    };
    for row in rows {
        match row.finding.status() {
            Open => counts.open += 1,
            Fixed => counts.fixed += 1,
            Withdrawn => counts.withdrawn += 1,
            Uncertain => counts.uncertain += 1,
        }
    }
    counts
}

/// The summary of `current` after `earlier`, once the findings whose IDs are
/// in `verified` have verified roots. Every row then takes round 2's state and
/// actions: F1 fixed, F2 withdrawn and F3 posted; the counts follow those rows.
fn view(current: &Round, earlier: Round, verified: &[u64]) -> Result<SummaryView, Box<dyn Error>> {
    let mut roots = Vec::new();
    for (finding, created_in) in [(retained(Open)?, 1), (owner(Open)?, 1), (typed(Open)?, 2)] {
        if verified.contains(&finding.id().get()) {
            roots.push(coverage::root(&finding, created_in)?);
        }
    }
    let mut view = coverage::view(current, &[earlier], roots)?;
    let resolved = |to| {
        vec![
            PublishedAction::Transition { from: Open, to },
            PublishedAction::Resolved,
        ]
    };
    for row in &mut view.findings {
        (row.finding, row.actions) = match row.finding.id().get() {
            1 => (retained(Fixed)?, resolved(Fixed)),
            2 => (owner(Withdrawn)?, resolved(Withdrawn)),
            _ => (typed(Open)?, vec![PublishedAction::Created]),
        };
    }
    view.counts = counts(&view.findings);
    Ok(view)
}

/// The completed second round after the blocked first round.
fn two_rounds() -> Result<SummaryView, Box<dyn Error>> {
    let current = second(
        RoundStatus::Completed,
        ReviewDecision::ChangesRequired,
        None,
        Outcome::Satisfied,
    )?;
    view(&current, first(Some(posted()?))?, &[1, 2, 3])
}

/// Round 1 as the current round of a first review.
fn first_review() -> Result<SummaryView, Box<dyn Error>> {
    let roots = vec![
        coverage::root(&retained(Open)?, 1)?,
        coverage::root(&owner(Open)?, 1)?,
    ];
    let mut view = coverage::view(&first(None)?, &[], roots)?;
    for row in &mut view.findings {
        row.actions = vec![PublishedAction::Created];
    }
    Ok(view)
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.map(String::from).to_vec()
}

fn findings_header(round: &str) -> Vec<String> {
    strings(["ID", "Finding", "Severity", "Status", round])
}

/// The supporting facts of a round in the default renderer's order.
fn round_facts(revision: &str, limitation: Option<&str>) -> Vec<String> {
    let mut facts = vec![
        revision.to_owned(),
        format!("Scope: {}", coverage::SCOPE),
        format!("Depth: {}", coverage::DEPTH),
        WORKFLOW.to_owned(),
    ];
    facts.extend(limitation.map(|text| format!("Limitation: {text}")));
    facts
}

fn row<'a>(text: &'a str, start: &str) -> Result<&'a str, Box<dyn Error>> {
    let mut rows = text.lines().filter(|line| line.starts_with(start));
    let row = rows
        .next()
        .ok_or_else(|| format!("missing {start}: {text}"))?;
    assert!(rows.next().is_none(), "repeated {start}: {text}");
    Ok(row)
}

#[test]
fn two_round_summary_matches_its_reviewed_rendering() -> TestResult {
    assert_eq!(
        render(&two_rounds()?)?,
        include_str!("fixtures/two-round-summary.md")
    );
    Ok(())
}

#[test]
fn first_review_keeps_its_details_under_round_1_in_one_bottom_section() -> TestResult {
    let body = render(&first_review()?)?;
    let sections = layout(&body)?;
    assert_eq!(sections.outline(), [(0, "Review details")], "{body}");
    assert!(sections.trailing.trim().is_empty(), "{body}");
    assert_eq!(
        blocks(sections.visible),
        Blocks {
            headings: vec![(3, "Blocked".into())],
            paragraphs: vec![
                "Round 1 · 2 open".into(),
                format!("Limitation: {EARLIER_LIMITATION}"),
            ],
            items: vec![],
            tables: vec![findings_header("This round")],
        },
        "{body}"
    );
    let details = groups(sections.section("Review details")?);
    assert!(details.lead.trim().is_empty(), "{body}");
    assert_eq!(details.headings(), ["Round 1"], "{body}");
    let group = details.group("Round 1")?;
    assert_eq!(
        blocks(group),
        Blocks {
            headings: vec![],
            paragraphs: strings(["Applicable policies (3)", "Excluded policies (1)"]),
            items: round_facts(REVIEWED, None),
            tables: vec![
                strings(["Policy", "Result", "Open findings", "Reason"]),
                strings(["Policy", "Reason"]),
            ],
        },
        "{body}"
    );
    assert_eq!(
        row(group, "| Baseline Proof Policy | ")?,
        format!(
            "| Baseline Proof Policy | Blocked: see the limitation above | — | {} |",
            PROOF.reason
        )
    );
    assert_eq!(body.matches(EARLIER_LIMITATION).count(), 1, "{body}");
    Ok(())
}

#[test]
fn every_round_section_holds_only_its_decision_and_findings() -> TestResult {
    let body = render(&two_rounds()?)?;
    let sections = layout(&body)?;
    assert_eq!(
        sections.outline(),
        [(0, "Round 1 · Blocked"), (0, "Review details")],
        "{body}"
    );
    assert!(sections.trailing.trim().is_empty(), "{body}");
    assert_eq!(
        blocks(sections.visible),
        Blocks {
            headings: vec![(3, "Changes required".into())],
            paragraphs: strings(["Round 2 · 1 open · 1 fixed · 1 withdrawn"]),
            items: vec![],
            tables: vec![findings_header("This round")],
        },
        "{body}"
    );
    assert_eq!(
        blocks(sections.section("Round 1 · Blocked")?),
        Blocks {
            tables: vec![findings_header("That round")],
            ..Blocks::default()
        },
        "{body}"
    );
    let details = blocks(sections.section("Review details")?);
    assert!(
        details.tables.iter().all(|table| table[0] == "Policy"),
        "{body}"
    );
    assert!(
        details.headings.iter().all(|(level, _)| *level == 4),
        "{body}"
    );
    let all = blocks(&body);
    assert_eq!(
        all.tables
            .iter()
            .filter(|table| table[0] == "ID")
            .collect::<Vec<_>>(),
        [
            &findings_header("This round"),
            &findings_header("That round")
        ],
        "{body}"
    );
    Ok(())
}

#[test]
fn review_details_holds_each_rounds_supporting_details_once_current_round_first() -> TestResult {
    let body = render(&two_rounds()?)?;
    let sections = layout(&body)?;
    let details = groups(sections.section("Review details")?);
    assert!(details.lead.trim().is_empty(), "{body}");
    assert_eq!(details.headings(), ["Round 2", "Round 1"], "{body}");
    assert_eq!(
        blocks(details.group("Round 2")?),
        Blocks {
            headings: vec![],
            paragraphs: strings(["Applicable policies (2)", "Excluded policies (2)"]),
            items: round_facts(REVIEWED, None),
            tables: vec![
                strings(["Policy", "Result", "Open findings", "Reason"]),
                strings(["Policy", "Reason"]),
            ],
        },
        "{body}"
    );
    assert_eq!(
        blocks(details.group("Round 1")?),
        Blocks {
            headings: vec![],
            paragraphs: strings(["Applicable policies (3)", "Excluded policies (1)"]),
            items: round_facts(REVIEWED, Some(EARLIER_LIMITATION)),
            tables: vec![
                strings(["Policy", "Result", "Reason"]),
                strings(["Policy", "Reason"]),
            ],
        },
        "{body}"
    );
    let mut every_fact = round_facts(REVIEWED, None);
    every_fact.extend(round_facts(REVIEWED, Some(EARLIER_LIMITATION)));
    assert_eq!(blocks(&body).items, every_fact, "{body}");
    Ok(())
}

#[test]
fn fixed_and_withdrawn_findings_keep_their_open_rows_in_the_earlier_round() -> TestResult {
    let body = render(&two_rounds()?)?;
    let sections = layout(&body)?;
    let earlier = sections.section("Round 1 · Blocked")?;
    for (start, current, recorded) in [
        (
            "| F1 | ",
            " | P2 · Blocking | Fixed | Open → Fixed, resolved |",
            " | P2 · Blocking | Open | Posted |",
        ),
        (
            "| F2 | ",
            " | P3 · Blocking | Withdrawn | Open → Withdrawn, resolved |",
            " | P3 · Blocking | Open | Posted |",
        ),
    ] {
        for (text, ending) in [(sections.visible, current), (earlier, recorded)] {
            let line = row(text, start)?;
            assert!(line.ends_with(ending), "{line}");
        }
    }
    assert!(
        row(sections.visible, "| F3 | ")?.ends_with(" | P2 · Blocking | Open | Posted |"),
        "{body}"
    );
    assert!(!earlier.contains("| F3 | "), "{earlier}");
    for id in 1..=2 {
        let link = format!(
            "(<https://gitlab.example/team/review-playground/-/merge_requests/33#note_2788570{id}>)"
        );
        assert!(row(sections.visible, &format!("| F{id} | "))?.contains(&link));
        assert!(row(earlier, &format!("| F{id} | "))?.contains(&link));
    }
    Ok(())
}

#[test]
fn earlier_declarations_keep_their_own_results_after_a_change_or_exclusion() -> TestResult {
    let body = render(&two_rounds()?)?;
    let sections = layout(&body)?;
    let details = groups(sections.section("Review details")?);
    let current = details.group("Round 2")?;
    let earlier = details.group("Round 1")?;
    for (text, start, expected) in [
        (
            current,
            "| Rust Error Contracts | ",
            format!("| Rust Error Contracts | Fail | F3 | {} |", ERRORS.reason),
        ),
        (
            current,
            "| Baseline Proof Policy | ",
            format!("| Baseline Proof Policy | Pass | — | {} |", PROOF.reason),
        ),
        (
            current,
            "| State Coherence Policy | ",
            format!("| State Coherence Policy | {} |", UNSHARED.reason),
        ),
        (
            earlier,
            "| Rust Error Contracts | ",
            format!("| Rust Error Contracts | Fail | {} |", ERRORS.reason),
        ),
        (
            earlier,
            "| Baseline Proof Policy | ",
            format!(
                "| Baseline Proof Policy | Blocked: see the limitation above | {} |",
                PROOF.reason
            ),
        ),
        (
            earlier,
            "| State Coherence Policy | ",
            format!("| State Coherence Policy | Fail | {} |", SHARED.reason),
        ),
        (
            earlier,
            "| Technical Claims Policy | ",
            format!("| Technical Claims Policy | {} |", CLAIMS.reason),
        ),
    ] {
        assert_eq!(row(text, start)?, expected, "{body}");
    }
    let limitation = earlier
        .find(&format!("\n- Limitation: {EARLIER_LIMITATION}\n"))
        .ok_or("round 1 lost its limitation")?;
    let reference = earlier
        .find("see the limitation above")
        .ok_or("round 1 lost its reference")?;
    assert!(limitation < reference, "{earlier}");
    Ok(())
}

#[test]
fn unfinished_and_blocked_rounds_keep_their_notices_beside_the_decision() -> TestResult {
    for (status, line) in [
        (RoundStatus::Pending, "This round has not finished posting."),
        (
            RoundStatus::Partial,
            "Some updates for this round are not posted yet.",
        ),
    ] {
        let current = second(
            status,
            ReviewDecision::ChangesRequired,
            None,
            Outcome::Satisfied,
        )?;
        let body = render(&view(&current, first(Some(posted()?))?, &[1, 2])?)?;
        let sections = layout(&body)?;
        assert_eq!(
            blocks(sections.visible).paragraphs,
            strings([
                "Round 2 · 1 fixed · 1 withdrawn",
                line,
                "Not posted yet: F3"
            ]),
            "{status:?}: {body}"
        );
        assert_eq!(body.matches(line).count(), 1, "{status:?}: {body}");
        let details = groups(sections.section("Review details")?);
        assert_eq!(
            blocks(details.group("Round 2")?).items,
            round_facts(ATTEMPTED, None),
            "{status:?}: {body}"
        );
    }

    let current = second(
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some(CURRENT_LIMITATION),
        Outcome::Blocked(CURRENT_LIMITATION),
    )?;
    let body = render(&view(&current, first(Some(posted()?))?, &[1, 2, 3])?)?;
    let sections = layout(&body)?;
    assert_eq!(
        blocks(sections.visible).paragraphs,
        [
            "Round 2 · 1 open · 1 fixed · 1 withdrawn".to_owned(),
            format!("Limitation: {CURRENT_LIMITATION}"),
        ],
        "{body}"
    );
    assert_eq!(body.matches(CURRENT_LIMITATION).count(), 1, "{body}");
    let details = groups(sections.section("Review details")?);
    let group = details.group("Round 2")?;
    assert_eq!(blocks(group).items, round_facts(REVIEWED, None), "{body}");
    assert_eq!(
        row(group, "| Baseline Proof Policy | ")?,
        format!(
            "| Baseline Proof Policy | Blocked: see the limitation above | — | {} |",
            PROOF.reason
        )
    );
    Ok(())
}

#[test]
fn earlier_rounds_without_findings_or_records_stay_readable() -> TestResult {
    for (rows, notice) in [
        (Some(vec![]), "No findings were posted."),
        (None, "Findings for this round were not recorded."),
    ] {
        let current = second(
            RoundStatus::Completed,
            ReviewDecision::ChangesRequired,
            None,
            Outcome::Satisfied,
        )?;
        let body = render(&view(&current, first(rows)?, &[1, 2, 3])?)?;
        let sections = layout(&body)?;
        assert_eq!(
            blocks(sections.section("Round 1 · Blocked")?),
            Blocks {
                paragraphs: strings([notice]),
                ..Blocks::default()
            },
            "{body}"
        );
        assert_eq!(body.matches(notice).count(), 1, "{body}");
        let details = groups(sections.section("Review details")?);
        let earlier = blocks(details.group("Round 1")?);
        assert_eq!(
            earlier.items,
            round_facts(REVIEWED, Some(EARLIER_LIMITATION)),
            "{body}"
        );
        assert_eq!(earlier.tables.len(), 2, "{body}");
    }
    Ok(())
}

#[test]
fn earlier_publication_limitation_stays_under_that_rounds_heading() -> TestResult {
    const REJECTED: &str = "A required publication effect was rejected";
    let mut earlier = first(Some(posted()?))?;
    earlier.status = RoundStatus::Partial;
    earlier.publication_limitation = Some(PublicationLimitation::Rejected);
    let current = second(
        RoundStatus::Completed,
        ReviewDecision::ChangesRequired,
        None,
        Outcome::Satisfied,
    )?;
    let body = render(&view(&current, earlier, &[1, 2, 3])?)?;
    let sections = layout(&body)?;
    assert_eq!(
        sections.outline(),
        [(0, "Round 1 · Blocked · partial"), (0, "Review details")],
        "{body}"
    );
    assert_eq!(
        blocks(sections.section("Round 1 · Blocked · partial")?),
        Blocks {
            tables: vec![findings_header("That round")],
            ..Blocks::default()
        },
        "{body}"
    );
    assert_eq!(body.matches(REJECTED).count(), 1, "{body}");
    let details = groups(sections.section("Review details")?);
    let mut facts = round_facts(ATTEMPTED, Some(EARLIER_LIMITATION));
    facts.push(format!("Publishing: {REJECTED}"));
    assert_eq!(blocks(details.group("Round 1")?).items, facts, "{body}");
    assert_eq!(
        blocks(details.group("Round 2")?).items,
        round_facts(REVIEWED, None),
        "{body}"
    );
    Ok(())
}
