//! Carried-finding decisions and recorded confirmations in the authored assessment.

use review_kit_application::use_cases::checkpoints::{
    Confirmation, ConfirmationResult, admit_findings, requires_confirmation,
};
use review_kit_domain::{Finding, FindingId};
use serde::{Deserialize, Serialize};

use super::diagnostics;
use crate::{Diagnostic, JsonError, domain::FindingData};

/// A carried finding's current decision.
///
/// The wire form is `null` while undecided, the string `"unchanged"` when the
/// captured version stands, or a complete finding object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "serde_json::Value", into = "Option<ReassessmentWire>")]
pub enum ReassessmentData {
    /// No decision yet; checking reports the missing reassessment.
    Pending,
    /// The captured version stands exactly as before.
    Unchanged,
    /// A complete current version of the finding.
    Reassessed(Box<FindingData>),
}

const UNCHANGED: &str = "unchanged";

/// Encoded form of [`ReassessmentData`]; `None` encodes the undecided state.
#[derive(Serialize)]
#[serde(untagged)]
enum ReassessmentWire {
    Marker(&'static str),
    Finding(Box<FindingData>),
}

#[review_kit_observation::operations]
impl From<ReassessmentData> for Option<ReassessmentWire> {
    #[helper]
    fn from(value: ReassessmentData) -> Self {
        match value {
            ReassessmentData::Pending => None,
            ReassessmentData::Unchanged => Some(ReassessmentWire::Marker(UNCHANGED)),
            ReassessmentData::Reassessed(finding) => Some(ReassessmentWire::Finding(finding)),
        }
    }
}

#[review_kit_observation::operations]
impl TryFrom<serde_json::Value> for ReassessmentData {
    type Error = String;

    #[helper]
    fn try_from(value: serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::Null => Ok(Self::Pending),
            serde_json::Value::String(marker) if marker == UNCHANGED => Ok(Self::Unchanged),
            serde_json::Value::String(marker) => Err(format!(
                "unknown reassessment `{marker}`; expected null, \"{UNCHANGED}\" or a finding"
            )),
            value => serde_json::from_value(value)
                .map(|finding| Self::Reassessed(Box::new(finding)))
                .map_err(|error| error.to_string()),
        }
    }
}

/// A recorded independent verdict on one finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfirmationData {
    /// An isolated checker examined the `checked` version, whose ID is `finding`.
    Checker {
        finding: u64,
        result: ConfirmationResultData,
        checked: Box<FindingData>,
        basis: String,
    },
    /// The agent reuses the recorded confirmation of the unchanged captured version.
    Retained { finding: u64, basis: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationResultData {
    Confirmed,
    Rejected,
    EvidenceGap,
}

#[review_kit_observation::operations]
impl ConfirmationData {
    #[helper]
    pub fn finding_number(&self) -> u64 {
        match self {
            Self::Checker { finding, .. } | Self::Retained { finding, .. } => *finding,
        }
    }

    /// The application verdict, or why this entry cannot be one.
    #[helper]
    fn verdict(&self) -> Result<Confirmation, String> {
        let id = FindingId::try_from(self.finding_number())
            .map_err(|_| "Confirmation must name a positive finding ID.".to_owned())?;
        Ok(match self {
            Self::Checker {
                result,
                checked,
                basis,
                ..
            } => {
                let checked = checked
                    .as_ref()
                    .clone()
                    .into_finding()
                    .map_err(|error| format!("Checked finding is invalid: {error}."))?;
                if checked.id() != id {
                    return Err("Checked finding has another ID than the entry names.".into());
                }
                Confirmation::Checked {
                    result: match result {
                        ConfirmationResultData::Confirmed => ConfirmationResult::Confirmed,
                        ConfirmationResultData::Rejected => ConfirmationResult::Rejected,
                        ConfirmationResultData::EvidenceGap => ConfirmationResult::EvidenceGap,
                    },
                    checked: Box::new(checked),
                    basis: basis.clone(),
                }
            }
            Self::Retained { basis, .. } => Confirmation::Retained {
                finding: id,
                basis: basis.clone(),
            },
        })
    }
}

/// The captured version for an `"unchanged"` decision.
///
/// A finding that requires confirmation restarts unvalidated, so only a current
/// confirmation can admit it; other findings keep their captured validation.
#[review_kit_observation::operations]
#[helper]
pub(super) fn unchanged(previous: &Finding) -> FindingData {
    let validated = previous.validated() && !requires_confirmation(previous);
    (&previous.clone().with_validated(validated)).into()
}

/// Apply recorded confirmations to the final carried and new findings.
///
/// `reusable` holds the captured versions whose recorded confirmations retained
/// entries can reuse. Findings stay as written when a finding does not decode,
/// which draft completion reports, or when a confirmation problem is recorded.
#[review_kit_observation::operations]
#[helper]
pub(super) fn admit(
    carried: Vec<(FindingData, Option<FindingData>)>,
    new_findings: &[FindingData],
    entries: &[ConfirmationData],
    reusable: &[Finding],
    problems: &mut Vec<Diagnostic>,
) -> (Vec<(FindingData, Option<FindingData>)>, Vec<FindingData>) {
    let mut confirmations = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match entry.verdict() {
            Ok(confirmation) => confirmations.push(confirmation),
            Err(message) => problems.push(Diagnostic::input(
                "invalid_confirmation",
                format!("confirmations[{index}]"),
                Some(entry.finding_number()),
                &message,
            )),
        }
    }
    let decoded: Result<Vec<Finding>, JsonError> = carried
        .iter()
        .filter_map(|(_, current)| current.clone())
        .chain(new_findings.iter().cloned())
        .map(FindingData::into_finding)
        .collect();
    let Ok(decoded) = decoded else {
        return (carried, new_findings.to_vec());
    };
    let admitted = match admit_findings(decoded, reusable, &confirmations) {
        Ok(admitted) => admitted,
        Err(issues) => {
            problems.extend(issues.iter().map(diagnostics::confirmation));
            return (carried, new_findings.to_vec());
        }
    };
    let validated = |data: FindingData| {
        let number = data.finding_number();
        match admitted.iter().find(|finding| finding.id().get() == number) {
            Some(finding) => data.with_validated(finding.validated()),
            None => data,
        }
    };
    (
        carried
            .into_iter()
            .map(|(previous, current)| (previous, current.map(validated)))
            .collect(),
        new_findings.iter().cloned().map(validated).collect(),
    )
}

#[cfg(test)]
mod tests {
    use review_kit_domain::{
        Disposition, Finding, FindingDraft, FindingId, FindingStatus, LineNumber, PolicyId,
        Severity, SourceLocation, SourcePath,
    };

    use super::unchanged;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn captured(
        disposition: Disposition,
        status: FindingStatus,
    ) -> Result<Finding, Box<dyn std::error::Error>> {
        Ok(Finding::new(FindingDraft {
            id: FindingId::try_from(1)?,
            policy_id: PolicyId::try_from("money".to_owned())?,
            title: "Round fees half away from zero".into(),
            body: "A 28.5-cent fee rounds down to 28 cents.".into(),
            severity: Severity::P2,
            disposition,
            status,
            evidence: "percentage_fee(Cents(5_000), 0.57) returns Cents(28).".into(),
            validated: true,
            location: SourceLocation::new(
                SourcePath::try_from("src/fees.rs".to_owned())?,
                LineNumber::try_from(5)?,
            ),
            suggestion: None,
        })?)
    }

    #[test]
    fn only_findings_that_require_confirmation_restart_unvalidated() -> TestResult {
        for (disposition, status, validated) in [
            (Disposition::Blocking, FindingStatus::Open, false),
            (Disposition::Blocking, FindingStatus::Uncertain, false),
            (Disposition::Blocking, FindingStatus::Fixed, true),
            (Disposition::Blocking, FindingStatus::Withdrawn, true),
            (Disposition::Advisory, FindingStatus::Open, true),
        ] {
            let previous = captured(disposition, status)?;
            let current = unchanged(&previous).into_finding()?;
            assert_eq!(current.validated(), validated, "{disposition:?} {status:?}");
            assert!(current.same_content(&previous));
        }
        Ok(())
    }
}
