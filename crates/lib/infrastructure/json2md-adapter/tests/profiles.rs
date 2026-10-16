mod support;

use std::{error::Error, fs, path::Path};

use review_kit_application::ports::*;
use review_kit_application::publication::{PolicyName, RoundStatus};
use review_kit_application::summary::{HistoricalFinding, PublishedAction};
use review_kit_domain::FindingStatus;
use review_kit_json2md_adapter::{Json2MdRenderer, MANIFEST_FILENAME, RendererError};
use serde_json::{Value, json};
use support::*;
use tempfile::TempDir;

fn all_bodies(renderer: &mut Json2MdRenderer) -> Result<Vec<String>, Box<dyn Error>> {
    let finding = finding(Some(vec!["```rust".into(), "/close".into(), "last".into()]))?;
    let context = context()?;
    let view = summary()?;
    let (answer, target) = answer_parts(&view)?;
    Ok(vec![
        render_finding(renderer, &finding)?,
        renderer.summary(&view)?,
        renderer.transition_context(TransitionRenderInput {
            finding: FindingRenderInput {
                finding: &finding,
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
    ])
}

fn error_chain(error: &dyn Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        messages.push(error.to_string());
        source = error.source();
    }
    messages.join(": ")
}

fn export() -> Result<(TempDir, std::path::PathBuf), Box<dyn Error>> {
    let directory = TempDir::new()?;
    let manifest = Json2MdRenderer::export_default(directory.path())?;
    Ok((directory, manifest))
}

fn edit_manifest(path: &Path, edit: impl FnOnce(&mut Value)) -> TestResult {
    let mut manifest: Value = serde_json::from_slice(&fs::read(path)?)?;
    edit(&mut manifest);
    fs::write(path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

#[test]
fn unedited_export_matches_embedded_identity_and_every_artifact() -> TestResult {
    let (directory, path) = export()?;
    assert_eq!(path, directory.path().join(MANIFEST_FILENAME));
    let mut embedded = Json2MdRenderer::new()?;
    let mut external = Json2MdRenderer::from_profile(&path)?;
    assert_eq!(embedded.identity(), external.identity());
    assert!(embedded.identity().fingerprint.starts_with("sha256:"));
    assert_eq!(all_bodies(&mut embedded)?, all_bodies(&mut external)?);
    let manifest: Value = serde_json::from_slice(&fs::read(path)?)?;
    for role in ["finding", "summary", "transition", "answer", "suggestion"] {
        for kind in ["schema", "template"] {
            let asset = manifest["artifacts"][role][kind]
                .as_str()
                .ok_or("missing exported asset")?;
            assert!(directory.path().join(asset).is_file());
        }
    }
    Ok(())
}

#[test]
fn a_strict_v2_summary_schema_without_declarations_refuses_current_input() -> TestResult {
    let (directory, manifest) = export()?;
    let path = directory.path().join("summary.schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let item = &mut schema["properties"]["policies"]["items"];
    item["properties"]
        .as_object_mut()
        .ok_or("missing policy properties")?
        .remove("declarations");
    item["required"]
        .as_array_mut()
        .ok_or("missing policy requirements")?
        .retain(|field| field != "declarations");
    fs::write(&path, serde_json::to_vec_pretty(&schema)?)?;
    let mut renderer = Json2MdRenderer::from_profile(&manifest)?;
    let error = renderer
        .summary(&summary()?)
        .err()
        .ok_or("the old closed shape accepted declaration data")?;
    let diagnostic = error_chain(&error);
    assert!(diagnostic.contains("declarations"), "{diagnostic}");
    assert!(diagnostic.contains("summary.schema.json"), "{diagnostic}");
    Ok(())
}

#[test]
fn documented_compact_profile_renders_every_role_offline() -> TestResult {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../examples/render-profiles/compact/profile.json");
    let mut renderer = Json2MdRenderer::from_profile(&manifest)?;
    assert_eq!(renderer.identity().id, "compact-review");
    assert_eq!(
        renderer.identity().input_contract,
        "review-kit-render-input/v1"
    );
    let bodies = all_bodies(&mut renderer)?;
    assert!(bodies[0].starts_with("### Preserve \\[result\\] \\*exactly\\* [P2 / F2]"));
    assert!(bodies[0].contains("Policy: Rust Error Contracts"));
    assert!(
        bodies[0]
            .contains("Suggested replacement:\n\n````suggestion:-1+1\n```rust\n/close\nlast\n````")
    );
    assert!(bodies[1].starts_with("## Review 2: Changes required"));
    assert!(bodies[1].contains("<summary>Review context</summary>"));
    assert!(bodies[1].contains("<summary>Earlier review 1:"));
    assert!(bodies[1].contains("<summary>Policy record</summary>"));
    assert!(bodies[1].contains("| Rust Ownership Design | — | 0 |"));
    assert!(bodies[2].starts_with("**F2: Fixed to Open**"));
    assert!(bodies[2].contains("The defect recurs.\n\nPolicy: Rust Error Contracts"));
    assert!(bodies[3].starts_with("**F2: response to note 702**"));
    assert!(bodies[3].contains("The return value is retained."));
    Ok(())
}

#[test]
fn compact_profile_shows_a_same_state_reclassification_as_an_update() -> TestResult {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../examples/render-profiles/compact/profile.json");
    let mut renderer = Json2MdRenderer::from_profile(&manifest)?;
    let mut view = summary()?;
    let moved = copy_finding(
        &view.findings[0].finding,
        review_kit_domain::Disposition::Blocking,
        FindingStatus::Open,
        "The defect remains at the moved line.",
    )?;
    view.findings[0].finding = moved.clone();
    view.findings[0].actions = vec![PublishedAction::Transition {
        from: FindingStatus::Open,
        to: FindingStatus::Open,
    }];
    let body = renderer.summary(&view)?;
    assert!(body.contains("| Open | Updated |"), "{body}");
    assert!(!body.contains("Open to Open"), "{body}");
    let context = context()?;
    let reply = renderer.transition_context(TransitionRenderInput {
        finding: FindingRenderInput {
            finding: &moved,
            policy_name: POLICY_NAME,
            context: &context,
        },
        previous: FindingStatus::Open,
        explanation: "The defect remains at the moved line.",
    })?;
    assert!(reply.starts_with("**F2: Updated**"), "{reply}");
    Ok(())
}

#[test]
fn historical_finding_names_survive_catalog_removal_without_future_renaming() -> TestResult {
    let (directory, path) = export()?;
    let mut view = summary()?;
    let original = view.history[0].clone();
    let fixed_finding = view.findings[0].finding.clone();
    let policy_id = fixed_finding.policy_id().clone();
    let fixed_name = "Rust _Fixed_ Contracts";
    let reintroduced_name = "Reintroduced Error Contracts";
    let current_name = "Current Error Contracts";

    let mut fixed = view.round.clone();
    fixed.number = 2;
    fixed.status = RoundStatus::Completed;
    fixed.policy_names = vec![PolicyName {
        id: policy_id.clone(),
        name: fixed_name.into(),
    }];
    fixed.communicated_findings = Some(vec![HistoricalFinding {
        finding: fixed_finding.clone(),
        root_note: view.findings[0].root.root_note.clone(),
        thread: view.findings[0].root.id.clone(),
        actions: vec![PublishedAction::Transition {
            from: FindingStatus::Open,
            to: FindingStatus::Fixed,
        }],
    }]);
    let mut removed = fixed.clone();
    removed.number = 3;
    removed.policy_names.clear();
    if let Some(rows) = &mut removed.communicated_findings {
        rows[0].actions.clear();
    }
    let mut reintroduced = removed.clone();
    reintroduced.number = 4;
    reintroduced.policy_names = vec![PolicyName {
        id: policy_id.clone(),
        name: reintroduced_name.into(),
    }];
    view.round.number = 5;
    view.round.status = RoundStatus::Completed;
    view.round.policy_names = vec![PolicyName {
        id: policy_id,
        name: current_name.into(),
    }];
    view.policies[0].name = Some(current_name.into());
    view.history = vec![removed, original, reintroduced, fixed];

    fs::write(
        directory.path().join("summary.schema.json"),
        serde_json::to_vec(&json!({
            "type": "object",
            "required": ["history", "findings"],
            "properties": {
                "history": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["findings"],
                        "properties": {
                            "findings": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "required": ["policy"],
                                    "properties": {
                                        "policy": {
                                            "type": "object",
                                            "required": ["name", "name_markdown"],
                                            "properties": {
                                                "name": {"type": "string", "minLength": 1},
                                                "name_markdown": {"type": "string", "minLength": 1}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }))?,
    )?;
    fs::write(
        directory.path().join("summary.j2"),
        concat!(
            "{% for finding in findings %}Current F{{ finding.id }} {{ finding.policy.name }}\n{% endfor %}",
            "{% for old in history %}{% for finding in old.findings %}",
            "R{{ old.number }} F{{ finding.id }} {{ finding.status }} ",
            "{{ finding.policy.name }} | {{ finding.policy.name_markdown }}\n",
            "{% endfor %}{% endfor %}"
        ),
    )?;
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    let body = renderer.summary(&view)?;
    assert_eq!(
        body,
        concat!(
            "Current F2 Current Error Contracts\n",
            "R4 F2 Fixed Reintroduced Error Contracts | Reintroduced Error Contracts\n",
            "R3 F2 Fixed Rust _Fixed_ Contracts | Rust \\_Fixed\\_ Contracts\n",
            "R2 F2 Fixed Rust _Fixed_ Contracts | Rust \\_Fixed\\_ Contracts\n",
            "R1 F2 Open Rust Error Contracts | Rust Error Contracts\n"
        )
    );
    Ok(())
}

#[test]
fn export_preserves_existing_files_before_creating_any_profile_files() -> TestResult {
    let directory = TempDir::new()?;
    let path = directory.path().join("answer.j2");
    fs::write(&path, "Caller-owned answer")?;
    assert!(matches!(
        Json2MdRenderer::export_default(directory.path()),
        Err(RendererError::ExportExists(_))
    ));
    assert_eq!(fs::read_to_string(path)?, "Caller-owned answer");
    assert!(!directory.path().join(MANIFEST_FILENAME).exists());
    assert!(!directory.path().join("finding.j2").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn export_preserves_existing_symbolic_links() -> TestResult {
    use std::os::unix::fs::symlink;
    let directory = TempDir::new()?;
    let destination = directory.path().join("finding.j2");
    symlink("absent-caller-file", &destination)?;
    assert!(matches!(
        Json2MdRenderer::export_default(directory.path()),
        Err(RendererError::ExportExists(_))
    ));
    assert_eq!(fs::read_link(destination)?, Path::new("absent-caller-file"));
    assert!(!directory.path().join(MANIFEST_FILENAME).exists());
    Ok(())
}

#[test]
fn custom_profile_owns_schemas_and_all_five_presentations() -> TestResult {
    let (directory, path) = export()?;
    let templates = [
        (
            "finding",
            "Custom finding F{{ id }}: {{ title_markdown }}\nPolicy {{ policy.name_markdown }}\n{{ body }}\n{{ evidence }}\n{% if suggestion is not none %}{{ suggestion.markdown }}{% endif %}",
        ),
        (
            "summary",
            "Custom review R{{ round.number }} {{ round.policy_set }}\n{% for policy in policies %}{{ policy.name }}: {{ policy.comments }}\n{% endfor %}",
        ),
        (
            "transition",
            "Update F{{ finding.id }}: {{ previous_status }} to {{ new_status }}\n{{ explanation }}",
        ),
        (
            "answer",
            "Question {{ question.id }} from {{ question.author }} on F{{ id }}: {{ question.body }}\n{{ body }}",
        ),
        (
            "suggestion",
            "Custom correction F{{ id }} ({{ start_line }}-{{ end_line }})\n{{ fence }}{{ info }}\n{{ replacement_content }}{{ fence }}\n",
        ),
    ];
    for (role, template) in templates {
        fs::write(directory.path().join(format!("{role}.j2")), template)?;
        fs::write(
            directory.path().join(format!("{role}.schema.json")),
            "{\"type\":\"object\"}",
        )?;
    }
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    let bodies = all_bodies(&mut renderer)?;
    assert!(bodies[0].starts_with("Custom finding F2:"));
    assert!(bodies[0].contains("Policy Rust Error Contracts"));
    assert!(
        bodies[0].contains(
            "Custom correction F2 (1-3)\n````suggestion:-1+1\n```rust\n/close\nlast\n````"
        )
    );
    assert!(bodies[1].starts_with("Custom review R2 rust-codebase-policies"));
    assert!(bodies[1].contains("Rust Ownership Design: 0"));
    assert_eq!(bodies[2], "Update F2: Fixed to Open\nThe defect recurs.");
    assert!(bodies[3].starts_with("Question 702 from 21 on F2: Why does the error disappear?"));
    assert_ne!(renderer.identity(), Json2MdRenderer::new()?.identity());
    Ok(())
}

#[test]
fn loaded_profile_is_an_owned_snapshot_and_later_edits_change_identity() -> TestResult {
    let (directory, path) = export()?;
    let mut snapshot = Json2MdRenderer::from_profile(&path)?;
    let value = finding(None)?;
    let original = render_finding(&mut snapshot, &value)?;
    fs::write(
        directory.path().join("finding.j2"),
        "Changed F{{ id }}\n{{ body }}",
    )?;
    let mut changed = Json2MdRenderer::from_profile(&path)?;
    assert_ne!(
        snapshot.identity().fingerprint,
        changed.identity().fingerprint
    );
    assert_eq!(render_finding(&mut snapshot, &value)?, original);
    assert!(render_finding(&mut changed, &value)?.starts_with("Changed F2"));
    fs::remove_file(directory.path().join("finding.j2"))?;
    assert_eq!(render_finding(&mut snapshot, &value)?, original);
    Ok(())
}

#[test]
fn canonical_manifest_whitespace_does_not_change_asset_identity() -> TestResult {
    let (_directory, path) = export()?;
    let before = Json2MdRenderer::from_profile(&path)?;
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    fs::write(&path, serde_json::to_vec(&manifest)?)?;
    assert_eq!(
        before.identity(),
        Json2MdRenderer::from_profile(&path)?.identity()
    );
    Ok(())
}

#[test]
fn invalid_manifests_and_assets_fail_with_actionable_diagnostics() -> TestResult {
    for mutation in [
        "manifest-version",
        "input-contract",
        "missing-role",
        "missing-asset",
        "malformed-schema",
        "invalid-schema",
        "invalid-template",
        "remote-schema",
        "private-profile-id",
    ] {
        let (directory, path) = export()?;
        match mutation {
            "manifest-version" => edit_manifest(&path, |v| v["version"] = json!("unknown/v9"))?,
            "input-contract" => {
                edit_manifest(&path, |v| v["input_contract"] = json!("unknown/v9"))?
            }
            "missing-role" => edit_manifest(&path, |v| {
                if let Some(artifacts) = v["artifacts"].as_object_mut() {
                    artifacts.remove("suggestion");
                }
            })?,
            "missing-asset" => fs::remove_file(directory.path().join("answer.j2"))?,
            "malformed-schema" => fs::write(directory.path().join("transition.schema.json"), "{")?,
            "invalid-schema" => fs::write(
                directory.path().join("finding.schema.json"),
                "{\"type\":\"invalid-type\"}",
            )?,
            "invalid-template" => {
                fs::write(directory.path().join("suggestion.j2"), "{% if broken %}")?
            }
            "remote-schema" => fs::write(
                directory.path().join("finding.schema.json"),
                "{\"$ref\":\"https://example.invalid/download/schema.json\"}",
            )?,
            "private-profile-id" => {
                edit_manifest(&path, |v| v["id"] = json!("/home/private/profile.json"))?
            }
            _ => return Err("unknown test mutation".into()),
        }
        let error = match Json2MdRenderer::from_profile(&path) {
            Ok(_) => return Err(format!("accepted invalid profile: {mutation}").into()),
            Err(error) => error,
        };
        let diagnostic = error_chain(&error);
        assert!(
            diagnostic.contains("profile")
                || diagnostic.contains("schema")
                || diagnostic.contains("asset"),
            "{mutation}: {diagnostic}"
        );
        if mutation == "missing-role" {
            assert!(diagnostic.contains("suggestion"));
        }
        if mutation == "invalid-template" {
            assert!(diagnostic.contains("suggestion"));
            assert!(diagnostic.contains("syntax") || diagnostic.contains("unexpected"));
        }
        if mutation == "remote-schema" {
            assert!(diagnostic.contains("external schema retrieval is disabled"));
        }
    }
    Ok(())
}

#[test]
fn missing_schema_data_and_undefined_template_fields_name_the_role_and_input() -> TestResult {
    let (directory, path) = export()?;
    fs::write(
        directory.path().join("finding.schema.json"),
        serde_json::to_vec(&json!({"type":"object","required":["missing_render_data"]}))?,
    )?;
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    let error = render_finding(&mut renderer, &finding(None)?)
        .err()
        .ok_or("missing rendering data was accepted")?;
    let diagnostic = error_chain(error.as_ref());
    assert!(diagnostic.contains("finding"));
    assert!(diagnostic.contains("input / (schema /required)"));
    assert!(diagnostic.contains("missing_render_data"));
    fs::write(directory.path().join("finding.schema.json"), "{}")?;
    fs::write(
        directory.path().join("finding.j2"),
        "{{ missing_render_data }}",
    )?;
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    let error = render_finding(&mut renderer, &finding(None)?)
        .err()
        .ok_or("undefined template field was accepted")?;
    let diagnostic = error_chain(error.as_ref());
    assert!(diagnostic.contains("finding.j2"));
    assert!(diagnostic.contains("undefined"));
    assert!(diagnostic.contains("missing_render_data"));
    Ok(())
}

#[test]
fn private_context_and_credentials_are_absent_from_every_public_input() -> TestResult {
    let (directory, path) = export()?;
    for role in ["finding", "summary", "transition", "answer", "suggestion"] {
        fs::write(
            directory.path().join(format!("{role}.schema.json")),
            serde_json::to_vec(
                &json!({"type":"object","not":{"anyOf":[{"required":["local_context"]},{"required":["credentials"]}]}}),
            )?,
        )?;
    }
    let mut renderer = Json2MdRenderer::from_profile(&path)?;
    assert_eq!(all_bodies(&mut renderer)?.len(), 4);
    Ok(())
}
