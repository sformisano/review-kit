//! The confirmations an isolated checker returns, for suites that test publication rather than admission.

use serde_json::{Value, json};

/// Record an exact checker confirmation for every validated open or uncertain blocking finding.
///
/// Replaces earlier confirmations, so it reflects the final finding versions.
pub fn confirm_blocking(draft: &mut Value) {
    let findings = draft["findings"].as_array().cloned().unwrap_or_default();
    let new_findings = draft["new_findings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let current = findings
        .iter()
        .map(|slot| slot["reassessment"].clone())
        .chain(new_findings)
        .filter(|finding| {
            finding["validated"] == true
                && finding["disposition"] == "blocking"
                && matches!(finding["status"].as_str(), Some("open" | "uncertain"))
        })
        .map(|finding| {
            json!({
                "source": "checker",
                "finding": finding["id"],
                "result": "confirmed",
                "checked": finding,
                "basis": "An isolated checker reproduced this finding against the reviewed source.",
            })
        })
        .collect::<Vec<_>>();
    draft["confirmations"] = json!(current);
}

/// Whether `value` is an assessment draft that records confirmations.
pub fn is_assessment(value: &Value) -> bool {
    value["kind"] == "assessment"
        && value["findings"].is_array()
        && value["new_findings"].is_array()
        && value["confirmations"].is_array()
}
