//! A review round shaped like a production blocked review: 29 declared
//! policies, 16 applicable and 13 excluded, with three open findings and one
//! blocked proof evaluation.

use std::error::Error;

use review_kit_application::{
    publication::*,
    summary::{HistoricalFinding, PublishedAction, SummaryView, summary_view},
    use_cases::checkpoints::ApplicabilityStatus,
};
use review_kit_domain::*;

pub const HOST: &str = "https://gitlab.example";
pub const REPOSITORY: &str = "team/review-playground";
pub const REQUEST: u64 = 33;

pub const SCOPE: &str = "Committed changes between the merge-request base and head: the retry queue in src/queue/retry.rs, its error types in src/queue/error.rs, the worker loop in src/worker.rs, the new integration test tests/retry.rs and the README section that describes retry limits. Generated lockfile changes were read but not reviewed line by line.";
pub const DEPTH: &str = "Standard depth. Read every changed line and the unchanged callers of RetryQueue::push and Worker::run; traced the failure branch from a send error to acknowledgement; compared the README claims with the implemented limits; checked that the new test exercises the documented retry bound. The code was not executed; see the limitation.";
pub const LIMITATION: &str = "Baseline proof is incomplete: no CI pipeline ran for this head, and the review host could not build the workspace because it has no Rust toolchain. The three findings are confirmed from source and an independent check.";
pub const PROOF_GAP: &str = "No CI pipeline exists for this head and the review host cannot build the workspace, so the changed retry path has no fresh build or test proof.";

/// How a declared policy ends in the round.
#[derive(Clone, Copy)]
pub enum Outcome {
    Excluded,
    Satisfied,
    Violated,
    Blocked(&'static str),
    Unresolved(&'static str),
}

#[derive(Clone, Copy)]
pub struct Declared {
    pub id: &'static str,
    pub name: &'static str,
    pub reason: &'static str,
    pub outcome: Outcome,
}

const fn declared(
    id: &'static str,
    name: &'static str,
    reason: &'static str,
    outcome: Outcome,
) -> Declared {
    Declared {
        id,
        name,
        reason,
        outcome,
    }
}

/// The 29 policies of the realistic round, with reasons written the way agents
/// currently write them.
pub fn realistic_policies() -> Vec<Declared> {
    use Outcome::*;
    vec![
        declared(
            "baseline-integrity-policy",
            "Baseline Integrity Policy",
            "Applies because every change can introduce unrelated churn, incomplete wiring or stale documentation, and this change edits code, tests and the README.",
            Satisfied,
        ),
        declared(
            "baseline-proof-policy",
            "Baseline Proof Policy",
            "Applies because every changed codebase surface needs fresh build and test proof, and this change alters runtime behavior in the worker loop.",
            Blocked(PROOF_GAP),
        ),
        declared(
            "business-risk-policy",
            "Business Risk Policy",
            "Excluded: the retry queue is an internal developer tool with no user, financial, operational or compliance exposure.",
            Excluded,
        ),
        declared(
            "change-integrity-policy",
            "Change Integrity Policy",
            "Applies because the change spans four files and must stay limited to the retry behavior described in the merge request.",
            Satisfied,
        ),
        declared(
            "code-structure-policy",
            "Code Structure Policy",
            "Applies because the change adds a module and moves retry decisions out of the worker loop.",
            Satisfied,
        ),
        declared(
            "cold-start-policy",
            "Cold Start Policy",
            "Excluded: no first-run, empty-state or initialization path changes; the queue is created exactly as before.",
            Excluded,
        ),
        declared(
            "contract-compatibility-policy",
            "Contract Compatibility Policy",
            "Applies because RetryQueue::push is public and its error type gains a variant.",
            Satisfied,
        ),
        declared(
            "data-migration-safety-policy",
            "Data Migration Safety Policy",
            "Excluded: nothing is persisted; retry state lives in memory for the lifetime of the worker.",
            Excluded,
        ),
        declared(
            "frontend-backend-parity-policy",
            "Frontend Backend Parity Policy",
            "Excluded: the repository has no frontend and the change crosses no client contract.",
            Excluded,
        ),
        declared(
            "frontend-experience-policy",
            "Frontend Experience Policy",
            "Excluded: no user-visible interface or workflow changes.",
            Excluded,
        ),
        declared(
            "library-compatibility-policy",
            "Library Compatibility Policy",
            "Excluded: no dependency is added or upgraded; the lockfile change only reorders existing entries.",
            Excluded,
        ),
        declared(
            "release-readiness-policy",
            "Release Readiness Policy",
            "Excluded: the merge request does not prepare a release, version bump or changelog.",
            Excluded,
        ),
        declared(
            "risk-based-testing",
            "Risk Based Testing",
            "Applies because the retry bound and the failure branch are new behavior that needs proportionate regression coverage.",
            Satisfied,
        ),
        declared(
            "runtime-lifecycle-policy",
            "Runtime Lifecycle Policy",
            "Excluded: startup and shutdown are unchanged; the worker still starts and stops through the existing supervisor.",
            Excluded,
        ),
        declared(
            "rust-abstraction-selection",
            "Rust Abstraction Selection",
            "Excluded: no trait, generic or dynamic dispatch is introduced or changed.",
            Excluded,
        ),
        declared(
            "rust-async-work-placement",
            "Rust Async Work Placement",
            "Excluded: the change adds no blocking call, CPU-heavy work or new task.",
            Excluded,
        ),
        declared(
            "rust-conventions-policy",
            "Rust Conventions Policy",
            "Applies because the change adds public Rust items and documentation comments.",
            Satisfied,
        ),
        declared(
            "rust-data-modeling",
            "Rust Data Modeling",
            "Applies because the retry outcome is modeled as a new enum.",
            Satisfied,
        ),
        declared(
            "rust-error-contracts",
            "Rust Error Contracts",
            "Applies because the change adds fallible send and retry paths that callers must be able to recover from.",
            Violated,
        ),
        declared(
            "rust-error-reporting",
            "Rust Error Reporting",
            "Applies because failed sends are now logged with context before a retry.",
            Satisfied,
        ),
        declared(
            "rust-invariant-preservation",
            "Rust Invariant Preservation",
            "Applies because RetryQueue::new accepts a retry limit that must stay positive.",
            Violated,
        ),
        declared(
            "rust-ownership-design",
            "Rust Ownership Design",
            "Applies because the queue now takes ownership of jobs between attempts.",
            Satisfied,
        ),
        declared(
            "rust-semantic-types",
            "Rust Semantic Types",
            "Applies because the retry limit and attempt counter are new integer parameters that could be confused.",
            Satisfied,
        ),
        declared(
            "rust-shared-resource-design",
            "Rust Shared Resource Design",
            "Excluded: no shared state, lock or connection pool is added or changed.",
            Excluded,
        ),
        declared(
            "rust-task-lifecycle",
            "Rust Task Lifecycle",
            "Excluded: no task is spawned, cancelled or joined by the change.",
            Excluded,
        ),
        declared(
            "rust-test-design",
            "Rust Test Design",
            "Applies because the change adds an integration test for the retry bound.",
            Satisfied,
        ),
        declared(
            "security-boundaries-policy",
            "Security Boundaries Policy",
            "Excluded: no authentication, authorization, secret handling or untrusted input boundary changes.",
            Excluded,
        ),
        declared(
            "state-coherence-policy",
            "State Coherence Policy",
            "Applies because a job can now exist in both the worker and the retry queue during a retry.",
            Satisfied,
        ),
        declared(
            "technical-claims-policy",
            "Technical Claims Policy",
            "Applies because the README now states the retry limit and back-off behavior.",
            Satisfied,
        ),
    ]
}

pub fn subject() -> Result<Subject, Box<dyn Error>> {
    Ok(Subject::new(
        "gitlab".into(),
        HOST.into(),
        REPOSITORY.into(),
        REQUEST,
    )?)
}

pub fn reviewer() -> Result<ReviewerId, Box<dyn Error>> {
    Ok(ReviewerId::try_from("17".to_owned())?)
}

pub fn revision() -> Result<Revision, Box<dyn Error>> {
    Ok(Revision::new(
        "4f2c1e9a7b3d5f60718293a4b5c6d7e8f9012345".into(),
        vec!["0a1b2c3d4e5f60718293a4b5c6d7e8f901234567".into()],
    )?)
}

pub fn open_finding(
    id: u64,
    policy: &str,
    title: &str,
    severity: Severity,
    line: u64,
) -> Result<Finding, Box<dyn Error>> {
    with_status(id, policy, title, severity, line, FindingStatus::Open)
}

pub fn with_status(
    id: u64,
    policy: &str,
    title: &str,
    severity: Severity,
    line: u64,
    status: FindingStatus,
) -> Result<Finding, Box<dyn Error>> {
    Ok(Finding::new(FindingDraft {
        id: FindingId::try_from(id)?,
        policy_id: PolicyId::try_from(policy.to_owned())?,
        title: title.into(),
        body: "The failure branch acknowledges the job before another attempt can run.".into(),
        severity,
        disposition: Disposition::Blocking,
        status,
        evidence: "The error branch returns without retaining the job.".into(),
        validated: true,
        location: SourceLocation::new(
            SourcePath::try_from("src/worker.rs".to_owned())?,
            LineNumber::try_from(line)?,
        ),
        suggestion: None,
    })?)
}

/// The three open findings of the realistic round.
pub fn realistic_findings() -> Result<Vec<Finding>, Box<dyn Error>> {
    Ok(vec![
        open_finding(
            1,
            "rust-error-contracts",
            "Retain the job after a failed send",
            Severity::P2,
            88,
        )?,
        open_finding(
            2,
            "rust-error-contracts",
            "Return a typed error when the retry limit is reached",
            Severity::P2,
            112,
        )?,
        open_finding(
            3,
            "rust-invariant-preservation",
            "Reject a zero retry limit at construction",
            Severity::P3,
            41,
        )?,
    ])
}

/// One retained or current round with the given declarations and findings.
pub fn round(
    number: u64,
    status: RoundStatus,
    decision: ReviewDecision,
    limitation: Option<&str>,
    policies: &[Declared],
    findings: Vec<Finding>,
) -> Result<Round, Box<dyn Error>> {
    let mut selected = Vec::new();
    let mut evaluations = Vec::new();
    let mut declarations = Vec::new();
    for policy in policies {
        let id = PolicyId::try_from(policy.id.to_owned())?;
        let (applicability, judgment) = match policy.outcome {
            Outcome::Excluded => (ApplicabilityStatus::Excluded, None),
            Outcome::Satisfied => (
                ApplicabilityStatus::Applicable,
                Some(PolicyJudgment::Satisfied),
            ),
            Outcome::Violated => (
                ApplicabilityStatus::Applicable,
                Some(PolicyJudgment::Violated {
                    evidence: "Private violation evidence".into(),
                }),
            ),
            Outcome::Blocked(limitation) => (
                ApplicabilityStatus::Applicable,
                Some(PolicyJudgment::Blocked {
                    limitation: limitation.into(),
                }),
            ),
            Outcome::Unresolved(limitation) => (
                ApplicabilityStatus::Unresolved,
                Some(PolicyJudgment::Blocked {
                    limitation: limitation.into(),
                }),
            ),
        };
        if let Some(judgment) = judgment {
            selected.push(id.clone());
            evaluations.push(PolicyEvaluation::new(id.clone(), judgment)?);
        }
        declarations.push(PublicPolicyDeclaration {
            policy: PolicyName {
                id,
                name: policy.name.into(),
            },
            status: applicability,
            reason: policy.reason.into(),
        });
    }
    let mut names: Vec<_> = declarations
        .iter()
        .filter(|row| row.status != ApplicabilityStatus::Excluded)
        .map(|row| row.policy.clone())
        .collect();
    names.sort_by(|left, right| left.id.cmp(&right.id));
    let assessment = Assessment::new(AssessmentDraft {
        subject: subject()?,
        reviewer: reviewer()?,
        revision: revision()?,
        decision,
        depth: DEPTH.into(),
        scope: SCOPE.into(),
        limitation: limitation.map(Into::into),
        selected_policies: selected,
        evaluations,
        findings,
    })
    .map_err(|errors| format!("{errors:?}"))?;
    Ok(Round {
        confirmed_validation: true,
        number,
        assessment,
        status,
        abandonment: None,
        publication_limitation: None,
        answers: vec![],
        transitions: vec![],
        answer_targets: vec![],
        actions: vec![],
        review_workflow: Some("codebase-policies-review".into()),
        policy_set: Some("rust-codebase-policies".into()),
        policy_declarations: declarations,
        policy_names: names,
        renderer: None,
        communicated_findings: None,
        verified_resolutions: Vec::new(),
    })
}

/// A verified root note for `finding`, created in round `created_in`.
pub fn root(finding: &Finding, created_in: u64) -> Result<FindingRecord, Box<dyn Error>> {
    let note = 27_885_700 + finding.id().get();
    Ok(FindingRecord {
        id: ProviderId::new(format!("discussion-{}", finding.id().get()))?,
        root_note: ProviderId::new(note.to_string())?,
        author: reviewer()?,
        created_in,
        revision: revision()?,
        finding: finding.clone(),
        body: "published body".into(),
        resolved: false,
    })
}

/// The verified rows an earlier round communicated, each under the root that
/// [`root`] gives its finding.
pub fn communicated(
    rows: Vec<(Finding, Vec<PublishedAction>)>,
) -> Result<Vec<HistoricalFinding>, Box<dyn Error>> {
    rows.into_iter()
        .map(|(finding, actions)| {
            let root = root(&finding, 1)?;
            Ok(HistoricalFinding {
                finding,
                root_note: root.root_note,
                thread: root.id,
                actions,
            })
        })
        .collect()
}

/// The summary view of `current` after its findings were posted, projected by
/// the application exactly as publication does.
pub fn view(
    current: &Round,
    history: &[Round],
    roots: Vec<FindingRecord>,
) -> Result<SummaryView, Box<dyn Error>> {
    let snapshot = Snapshot {
        subject: subject()?,
        reviewer: reviewer()?,
        revision: revision()?,
        summaries: vec![],
        findings: roots,
        replies: vec![],
        permissions: Effects::none(),
    };
    Ok(summary_view(current, history, &snapshot)?)
}

/// The realistic blocked round after its three findings were posted.
pub fn realistic() -> Result<SummaryView, Box<dyn Error>> {
    let findings = realistic_findings()?;
    let roots = findings
        .iter()
        .map(|finding| root(finding, 1))
        .collect::<Result<_, _>>()?;
    let current = round(
        1,
        RoundStatus::Blocked,
        ReviewDecision::Blocked,
        Some(LIMITATION),
        &realistic_policies(),
        findings,
    )?;
    view(&current, &[], roots)
}
