use super::*;

fn read(fixture: &Fixture) -> Value {
    harness::read_json(&fixture.assessment())
}

#[test]
fn arbitrary_policy_ids_need_no_installed_definitions_and_keep_display_names() {
    let fixture = Fixture::new();
    fixture.prepare_named(
        "Team review criteria",
        &[
            ("customer-contract", "Customer contract"),
            ("linked-guidance", "Linked guidance"),
        ],
    );
    assert!(!fixture.root.path().join(".agents").exists());
    let assessment = read(&fixture);
    assert_eq!(assessment["policy_set"], "Team review criteria");
    assert_eq!(assessment["policies"][0]["policy_id"], "customer-contract");
    assert_eq!(assessment["policies"][1]["policy_id"], "linked-guidance");
    assert!(assessment.get("requested_policies").is_none());
    fixture.fill_ready();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    let summary =
        fs::read_to_string(fixture.file(&fixture.selected(), "rendered/review-summary.md"))
            .unwrap();
    assert!(summary.contains("Customer contract"), "{summary}");
    assert!(summary.contains("Linked guidance"), "{summary}");
    assert!(!summary.contains("baseline-integrity-policy"), "{summary}");
}

#[test]
fn duplicate_policy_rows_and_unfinished_judgments_are_rejected() {
    let fixture = Fixture::new();
    fixture.ready();
    let original = read(&fixture);
    for duplicate in [false, true] {
        let mut assessment = original.clone();
        if duplicate {
            let first = assessment["policies"][0].clone();
            assessment["policies"].as_array_mut().unwrap().push(first);
        } else {
            assessment["policies"][0]["outcome"] = Value::Null;
        }
        write(&fixture.assessment(), &assessment);
        let (output, invalid) = fixture.run(&["check"]);
        assert_eq!(output.status.code(), Some(2), "{invalid}");
        assert!(!invalid["diagnostics"].as_array().unwrap().is_empty());
    }
}

#[test]
fn blocked_policy_requires_an_explicit_blocked_review_with_its_limitation() {
    let fixture = Fixture::new();
    fixture.ready();
    let mut assessment = read(&fixture);
    assessment["policies"][0]["outcome"] =
        json!({"kind":"blocked", "limitation":"Required evidence is unavailable."});
    write(&fixture.assessment(), &assessment);
    let (output, invalid) = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(2), "{invalid}");
    assessment["decision"] = json!("blocked");
    assessment["limitation"] = json!("Required evidence is unavailable.");
    write(&fixture.assessment(), &assessment);
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    assert_eq!(checked["status"], "valid");
}

#[test]
fn policy_coverage_changes_in_one_revision_without_loading_external_sources() {
    let fixture = Fixture::new();
    fixture.ready();
    fixture.ok(&["accept"]);
    let id = fixture.selected();
    fixture.ok(&["revise", "--reason", "Replace the selected criteria."]);
    fixture.declare_policies(&id, &["new-policy"]);
    fixture.fill_ready();
    let before = fixture.http.state.lock().unwrap().requests.len();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    let summary = fs::read_to_string(fixture.file(&id, "rendered/review-summary.md")).unwrap();
    assert!(summary.contains("new-policy"), "{summary}");
    assert!(!summary.contains("baseline-integrity-policy"), "{summary}");
    assert_eq!(fixture.http.state.lock().unwrap().requests.len(), before);
    assert_eq!(fixture.writes(), 0);
    assert!(!fixture.root.path().join(".agents").exists());
}

#[test]
fn obsolete_policy_configuration_fields_are_rejected() {
    for field in ["skills_dir", "policy_catalog"] {
        let fixture = Fixture::new();
        fixture.ready();
        let mut config = json!({"version":1});
        config[field] = json!("unused-path");
        write(&fixture.config, &config);
        let (output, invalid) = fixture.run(&["check"]);
        assert_eq!(output.status.code(), Some(2), "{invalid}");
        assert_eq!(invalid["diagnostics"][0]["code"], "configuration");
        assert!(
            !fixture
                .file(&fixture.selected(), "rendered/review-summary.md")
                .exists()
        );
    }
}

#[test]
fn agent_declared_policy_names_and_ids_stay_out_of_operation_logs() {
    let fixture = Fixture::stderr();
    fixture.prepare_named(
        "SECRET_POLICY_SET",
        &[("SECRET_POLICY_ID", "SECRET_POLICY_NAME")],
    );
    let (output, incomplete) = fixture.run(&["check"]);
    assert_eq!(output.status.code(), Some(2), "{incomplete}");
    assert!(!super::logging::records(&output).is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_"));
    fixture.fill_ready();
    let (output, checked) = fixture.run(&["check"]);
    assert!(output.status.success(), "{checked}");
    assert!(!super::logging::records(&output).is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_"));
}
