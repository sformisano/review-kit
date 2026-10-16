mod support;

use std::{error::Error, fs, path::Path};

use review_kit_application::{ports::*, summary::*};
use review_kit_domain::*;
use review_kit_json2md_adapter::{Json2MdRenderer, MANIFEST_FILENAME, RendererError};
use serde_json::{Value, json};
use support::*;
use tempfile::TempDir;

const V1: &str = "review-kit-render-input/v1";
const V2: &str = "review-kit-render-input/v2";

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

/// Byte-for-byte copy of the v1 default profile, the reference for unchanged v1 rendering.
fn v1_profile() -> std::path::PathBuf {
    fixtures().join("v1-profile").join(MANIFEST_FILENAME)
}

fn expected(name: &str) -> Result<String, Box<dyn Error>> {
    Ok(fs::read_to_string(fixtures().join("v1-output").join(name))?)
}

/// Copies a profile and replaces its templates with ones that print input fields.
fn input_dump(manifest: &Path) -> Result<(TempDir, std::path::PathBuf), Box<dyn Error>> {
    let directory = TempDir::new()?;
    let source = manifest.parent().ok_or("manifest has no directory")?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        fs::copy(entry.path(), directory.path().join(entry.file_name()))?;
    }
    for (role, template) in [
        (
            "finding",
            r#"{{ {"version": version, "suggestion": suggestion} | tojson }}"#,
        ),
        ("suggestion", "{{ version }}"),
        (
            "summary",
            r#"{{ {"version": version, "findings": findings, "history": history, "policies": policies} | tojson }}"#,
        ),
        (
            "transition",
            r#"{{ {"version": version, "finding": finding} | tojson }}"#,
        ),
        (
            "answer",
            r#"{{ {"version": version, "finding": finding} | tojson }}"#,
        ),
    ] {
        fs::write(directory.path().join(format!("{role}.j2")), template)?;
    }
    let manifest = directory.path().join(MANIFEST_FILENAME);
    Ok((directory, manifest))
}

fn versions(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                match (key.as_str(), field) {
                    ("version", Value::String(version)) => found.push(version.clone()),
                    // The suggestion role's rendered output, which prints its own version.
                    ("markdown", Value::String(markdown)) => found.push(markdown.clone()),
                    _ => versions(field, found),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| versions(item, found)),
        _ => {}
    }
}

fn summary_with_suggestion() -> Result<SummaryView, Box<dyn Error>> {
    let mut view = summary()?;
    let suggested = finding(Some(vec!["replacement".into()]))?;
    view.findings[0].finding = copy_finding(
        &suggested,
        Disposition::Blocking,
        FindingStatus::Fixed,
        "Regression passes",
    )?;
    Ok(view)
}

#[test]
fn previous_default_profile_keeps_its_identity_and_exact_bodies_under_v1() -> TestResult {
    let mut renderer = Json2MdRenderer::from_profile(&v1_profile())?;
    assert_eq!(
        renderer.identity(),
        &RenderProfileIdentity {
            id: "review-kit-default".into(),
            input_contract: V1.into(),
            fingerprint: "sha256:65929ce149a860947824989bb114b76f688e958d8908211fa5aae2f5694d100e"
                .into(),
        }
    );
    let plain = finding(None)?;
    assert_eq!(
        render_finding(&mut renderer, &plain)?,
        expected("finding.md")?
    );
    let suggested = finding(Some(vec!["```rust".into(), "/close".into(), "last".into()]))?;
    let advisory = copy_finding(
        &suggested,
        Disposition::Advisory,
        FindingStatus::Open,
        "Seen in `logs`.",
    )?;
    assert_eq!(
        render_finding(&mut renderer, &advisory)?,
        expected("advisory-suggestion.md")?
    );
    let view = summary()?;
    assert_eq!(renderer.summary(&view)?, expected("summary.md")?);
    let context = context()?;
    assert_eq!(
        renderer.transition_context(TransitionRenderInput {
            finding: FindingRenderInput {
                finding: &plain,
                policy_name: POLICY_NAME,
                context: &context,
            },
            previous: FindingStatus::Fixed,
            explanation: "The defect recurs.",
        })?,
        expected("transition.md")?
    );
    let (answer, target) = answer_parts(&view)?;
    assert_eq!(
        renderer.answer_context(AnswerRenderInput {
            answer: &answer,
            finding: &view.findings[0].root,
            current_finding: &view.findings[0].finding,
            target: &target,
            policy_name: POLICY_NAME,
            context: &context,
        })?,
        expected("answer.md")?
    );
    Ok(())
}

#[test]
fn every_role_input_carries_the_declared_contract_and_only_v2_adds_results() -> TestResult {
    let exported = TempDir::new()?;
    let default = Json2MdRenderer::export_default(exported.path())?;
    for (manifest, contract) in [(v1_profile(), V1), (default, V2)] {
        let (_directory, path) = input_dump(&manifest)?;
        let mut renderer = Json2MdRenderer::from_profile(&path)?;
        assert_eq!(renderer.identity().input_contract, contract);
        let view = summary_with_suggestion()?;
        let (answer, target) = answer_parts(&view)?;
        let context = context()?;
        let suggested = finding(Some(vec!["replacement".into()]))?;
        let bodies = [
            render_finding(&mut renderer, &suggested)?,
            renderer.summary(&view)?,
            renderer.transition_context(TransitionRenderInput {
                finding: FindingRenderInput {
                    finding: &suggested,
                    policy_name: POLICY_NAME,
                    context: &context,
                },
                previous: FindingStatus::Fixed,
                explanation: "The defect recurs.",
            })?,
            renderer.answer_context(AnswerRenderInput {
                answer: &answer,
                finding: &view.findings[0].root,
                current_finding: &view.findings[0].finding,
                target: &target,
                policy_name: POLICY_NAME,
                context: &context,
            })?,
        ];
        let mut found = Vec::new();
        for body in &bodies {
            versions(&serde_json::from_str(body)?, &mut found);
        }
        // Finding: itself, its suggestion and the suggestion body (3). Summary: itself,
        // the current row and its suggestion, the history row (4). Transition: itself,
        // its finding, suggestion and suggestion body (4). Answer: itself and its finding (2).
        assert_eq!(found.len(), 13, "{contract}: {found:?}");
        assert!(found.iter().all(|version| version == contract), "{found:?}");
        let summary: Value = serde_json::from_str(&bodies[1])?;
        let policies = summary["policies"]
            .as_array()
            .ok_or("missing summary policies")?;
        assert_eq!(policies.len(), 2);
        for policy in policies {
            let fields = policy.as_object().ok_or("policy is not an object")?;
            assert_eq!(fields.contains_key("result"), contract == V2, "{policy}");
        }
    }
    Ok(())
}

#[test]
fn v2_summary_policies_report_only_the_latest_round_result() -> TestResult {
    let exported = TempDir::new()?;
    let (_directory, path) = input_dump(&Json2MdRenderer::export_default(exported.path())?)?;
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    let mut view = summary()?;
    let violated = view.round.assessment.findings()[0].policy_id().clone();
    let satisfied = PolicyId::try_from("rust-ownership-design".to_owned())?;
    let blocked = PolicyId::try_from("proof-policy".to_owned())?;
    let retired = PolicyId::try_from("retired-policy".to_owned())?;
    let current = &view.round.assessment;
    // Round 1 evaluated the retired policy; the latest round does not.
    view.history[0].assessment = Assessment::new(AssessmentDraft {
        subject: current.subject().clone(),
        reviewer: current.reviewer().clone(),
        revision: current.revision().clone(),
        decision: ReviewDecision::ChangesRequired,
        depth: current.depth().into(),
        scope: current.scope().into(),
        limitation: None,
        selected_policies: vec![violated.clone(), retired.clone()],
        evaluations: vec![
            PolicyEvaluation::new(violated.clone(), PolicyJudgment::Satisfied)?,
            PolicyEvaluation::new(retired.clone(), PolicyJudgment::Satisfied)?,
        ],
        findings: current.findings().to_vec(),
    })
    .map_err(|errors| format!("{errors:?}"))?;
    view.round.assessment = Assessment::new(AssessmentDraft {
        subject: current.subject().clone(),
        reviewer: current.reviewer().clone(),
        revision: current.revision().clone(),
        decision: ReviewDecision::Blocked,
        depth: current.depth().into(),
        scope: current.scope().into(),
        limitation: Some("Proof logs are missing".into()),
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
                    limitation: "CI logs `expired` | *gone*".into(),
                },
            )?,
        ],
        findings: current.findings().to_vec(),
    })
    .map_err(|errors| format!("{errors:?}"))?;
    view.policies = [&violated, &satisfied, &blocked, &retired]
        .into_iter()
        .map(|id| PolicySummary {
            policy_id: id.clone(),
            name: None,
            applicable_rounds: if id == &retired { vec![1] } else { vec![2] },
            comments: 0,
            declarations: vec![],
        })
        .collect();
    let body = renderer.summary(&view)?;
    assert!(!body.contains("Private violation evidence"));
    let summary: Value = serde_json::from_str(&body)?;
    let results: Vec<_> = summary["policies"]
        .as_array()
        .ok_or("missing summary policies")?
        .iter()
        .map(|policy| (policy["id"].clone(), policy["result"].clone()))
        .collect();
    assert_eq!(
        results,
        vec![
            (
                json!(violated.as_str()),
                json!({"kind": "Violated", "limitation": null, "limitation_markdown": null})
            ),
            (
                json!("rust-ownership-design"),
                json!({"kind": "Satisfied", "limitation": null, "limitation_markdown": null})
            ),
            (
                json!("proof-policy"),
                json!({
                    "kind": "Blocked",
                    "limitation": "CI logs `expired` | *gone*",
                    "limitation_markdown": "CI logs \\`expired\\` &#124; \\*gone\\*",
                })
            ),
            (json!("retired-policy"), Value::Null),
        ]
    );
    Ok(())
}

#[test]
fn unsupported_input_contracts_fail_loading_and_name_supported_contracts() -> TestResult {
    for unsupported in [
        "review-kit-render-input/v3",
        "review-kit-render-input/V2",
        "",
    ] {
        let directory = TempDir::new()?;
        let path = Json2MdRenderer::export_default(directory.path())?;
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
        manifest["input_contract"] = json!(unsupported);
        fs::write(&path, serde_json::to_vec(&manifest)?)?;
        match Json2MdRenderer::from_profile(&path) {
            Err(error @ RendererError::UnsupportedContract { .. }) => {
                let diagnostic = error.to_string();
                assert!(
                    diagnostic.contains(V1) && diagnostic.contains(V2),
                    "{diagnostic}"
                );
            }
            Err(error) => return Err(format!("{unsupported:?}: unexpected {error}").into()),
            Ok(_) => return Err(format!("accepted contract {unsupported:?}").into()),
        }
    }
    Ok(())
}
