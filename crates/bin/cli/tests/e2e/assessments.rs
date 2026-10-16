use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::source::{self, AUDIT_ORIGINAL, AUDIT_REPLACEMENT, QUANTITY_DERIVE};

#[path = "../shared/confirmations.rs"]
#[allow(
    dead_code,
    reason = "Draft writes in these scenarios are always assessments."
)]
mod confirmations;
pub use confirmations::confirm_blocking;

pub const POLICY_SET: &str = "rust-codebase-policies";
/// The required proof baseline, then the list's Rust policies.
pub const POLICIES: [&str; 5] = [
    "baseline-proof-policy",
    "rust-invariant-preservation",
    "rust-error-contracts",
    "rust-test-design",
    "code-structure-policy",
];
pub const PRIVATE_CONTEXT: &str = "PRIVATE_CLI_E2E_CONTEXT_9b3e78";

fn policy_name(id: &str) -> String {
    id.split('-')
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fills a generated assessment draft with `policies` from the scripted list.
pub fn declare_policies_of(mut draft: Value, policies: &[&str]) -> Value {
    draft["policy_set"] = json!(POLICY_SET);
    draft["reason"] = json!("Read the review policy list and every policy it names.");
    draft["policies"] = json!(
        policies
            .iter()
            .map(|id| json!({
                "policy_id": id,
                "name": policy_name(id),
                "required": *id == POLICIES[0],
                "reason": "The change edits Rust source covered by this policy.",
                "outcome": null,
            }))
            .collect::<Vec<_>>()
    );
    draft
}

/// The declared policy selection: every policy of the list.
pub fn declare_policies(draft: Value) -> Value {
    declare_policies_of(draft, &POLICIES)
}

/// Repeats `previous` in a later run of the same head: the same coverage and
/// decision, and every carried finding reassessed exactly as communicated.
pub fn equivalent(previous: &Value, mut draft: Value) -> Value {
    for field in ["decision", "depth", "scope", "limitation", "local_context"] {
        draft[field] = previous[field].clone();
    }
    for policy in draft["policies"].as_array_mut().expect("draft policies") {
        policy["outcome"] = previous["policies"]
            .as_array()
            .expect("previous policies")
            .iter()
            .find(|row| row["policy_id"] == policy["policy_id"])
            .expect("previous outcome")["outcome"]
            .clone();
    }
    draft
}

pub fn ready(mut value: Value) -> Value {
    value["decision"] = json!("ready");
    value["depth"] = json!("Read the committed behavior and its regression cases.");
    value["scope"] = json!("Changed quantity and storage contracts.");
    value["limitation"] = Value::Null;
    value["local_context"] = json!(PRIVATE_CONTEXT);
    for policy in value["policies"].as_array_mut().expect("draft policies") {
        policy["outcome"] = json!({"kind": "satisfied"});
    }
    value
}

fn violations(value: &mut Value, evidence: &[(&str, &str)]) {
    for policy in value["policies"]
        .as_array_mut()
        .expect("assessment policies")
    {
        if let Some((_, evidence)) = evidence.iter().find(|(id, _)| policy["policy_id"] == *id) {
            policy["outcome"] = json!({"kind": "violated", "evidence": evidence});
        }
    }
    value["decision"] = json!("changes_required");
}

pub fn initial(draft: Value) -> Value {
    let mut value = ready(draft);
    let quantity_evidence = "Derived Deserialize accepts zero, bypassing Quantity::new and its positive-value invariant.";
    let audit_evidence =
        "An unavailable optional audit returns Retryable after the primary save succeeds.";
    let quantity_line = source::line_of(&source::quantity_input(), QUANTITY_DERIVE);
    let audit_line = source::line_of(&source::storage_audit(), AUDIT_ORIGINAL);
    value["new_findings"] = json!([
        {"id": 1, "policy_id": "rust-invariant-preservation",
         "title": "Validate the quantity during deserialization",
         "body": "JSON input accepts zero even though Quantity::new rejects it.\n\n- Invalid input:\n  ```rust\n  let input = r#\"{\"value\":0}\"#;\n  assert!(serde_json::from_str::<Quantity>(input).is_err());\n  ```\n\nRoute deserialization through the constructor.",
         "evidence": quantity_evidence, "location": {"path": "src/quantity.rs", "line": quantity_line}, "suggestion": null},
        {"id": 2, "policy_id": "rust-error-contracts",
         "title": "Preserve successful primary saves when the optional audit fails",
         "body": "The primary write succeeds before optional audit output fails. Returning Retryable contradicts its no-primary-data contract.",
         "evidence": audit_evidence, "location": {"path": "src/storage.rs", "line": audit_line},
         "suggestion": {"location": {"path": "src/storage.rs", "line": audit_line},
                        "revision": value["revision"], "start_line": audit_line,
                        "original": [AUDIT_ORIGINAL], "replacement": [AUDIT_REPLACEMENT],
                        "evidence": "The best-effort audit correction preserves primary success and leaves primary-write failures retryable."}}
    ]);
    for finding in value["new_findings"].as_array_mut().expect("new findings") {
        finding["severity"] = json!("p2");
        finding["disposition"] = json!("blocking");
        finding["status"] = json!("open");
        finding["validated"] = json!(true);
    }
    violations(
        &mut value,
        &[
            ("rust-invariant-preservation", quantity_evidence),
            ("rust-error-contracts", audit_evidence),
            (
                "baseline-proof-policy",
                "The initial tests omit invalid JSON input and optional-audit failure.",
            ),
            (
                "rust-test-design",
                "The positive-input test does not reject zero quantities.",
            ),
        ],
    );
    value
}

pub fn fixed(draft: Value, replies: &BTreeMap<u64, Vec<Value>>) -> Value {
    let mut value = ready(draft);
    let mut transitions = Vec::new();
    let mut answers = Vec::new();
    for slot in value["findings"].as_array_mut().expect("finding slots") {
        let previous = slot["reassessment"].clone();
        let id = previous["id"].as_u64().expect("finding id");
        let explanation = if id == 1 {
            "Deserialization calls Quantity::new; the committed regression rejects zero input."
        } else {
            "Optional audit output is best effort; the committed regression preserves primary success."
        };
        let mut finding = previous.clone();
        finding["status"] = json!("fixed");
        finding["evidence"] = json!(explanation);
        finding["suggestion"] = Value::Null;
        slot["reassessment"] = finding;
        transitions.push(
            json!({"finding": id, "from": previous["status"], "to": "fixed", "body": explanation}),
        );
        for note in &replies[&id] {
            answers.push(
                json!({"finding": id, "in_reply_to": note["id"].to_string(), "body": explanation}),
            );
        }
    }
    value["transitions"] = json!(transitions);
    value["answers"] = json!(answers);
    value
}

pub fn recurrent(draft: Value) -> Value {
    let mut value = ready(draft);
    let evidence =
        "Derived Deserialize again accepts zero and bypasses the positive-value constructor.";
    let mut transitions = Vec::new();
    for slot in value["findings"].as_array_mut().expect("finding slots") {
        let previous = slot["reassessment"].clone();
        let mut finding = previous.clone();
        if finding["id"] == 1 {
            finding["status"] = json!("open");
            finding["evidence"] = json!(evidence);
            finding["location"] = json!({"path": "src/quantity.rs", "line": source::line_of(&source::quantity_input(), QUANTITY_DERIVE)});
            transitions.push(
                json!({"finding": 1, "from": previous["status"], "to": "open", "body": evidence}),
            );
        } else {
            finding["evidence"] = json!(
                "The optional audit remains best effort and its regression remains committed."
            );
        }
        slot["reassessment"] = finding;
    }
    value["transitions"] = json!(transitions);
    violations(
        &mut value,
        &[
            ("rust-invariant-preservation", evidence),
            (
                "baseline-proof-policy",
                "The recurrent quantity input lacks a zero-rejection regression.",
            ),
            (
                "rust-test-design",
                "The recurrent positive-input test accepts the constructor bypass.",
            ),
        ],
    );
    value
}

pub fn blocked(draft: Value) -> Value {
    let mut value = ready(draft);
    value["decision"] = json!("blocked");
    value["limitation"] = json!("The external acceptance evidence is unavailable.");
    let limitation = value["limitation"].clone();
    for policy in value["policies"].as_array_mut().expect("policies") {
        if policy["policy_id"] == "baseline-proof-policy" {
            policy["outcome"] = json!({"kind": "blocked", "limitation": limitation});
        }
    }
    value
}
