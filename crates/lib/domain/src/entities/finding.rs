use crate::{
    Disposition, FindingId, FindingStatus, FindingTransition, PolicyId, Severity, SourceLocation,
    Suggestion,
};

/// Unchecked finding content. [`Finding::new`] establishes the entity's constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingDraft {
    pub id: FindingId,
    pub policy_id: PolicyId,
    pub title: String,
    pub body: String,
    pub severity: Severity,
    pub disposition: Disposition,
    pub status: FindingStatus,
    pub evidence: String,
    pub validated: bool,
    pub location: SourceLocation,
    pub suggestion: Option<Suggestion>,
}

/// One semantic defect whose identity survives reassessments and status changes.
///
/// It keeps a concern recognizable during follow-up, with its source anchor,
/// evidence, and the reviewer's current judgment. The containing assessment supplies
/// the overall conclusion for the reviewed revision.
///
/// [`FindingId`] is local to the enclosing review subject and reviewer. Equality
/// compares the complete recorded state, so two versions of the same finding can
/// differ. Publication recovery uses that comparison to detect changed intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    data: FindingDraft,
}

impl Finding {
    /// Rejects blank title, body or evidence, unsupported title delimiters, and a
    /// suggestion with a different anchor. Source existence and correctness need
    /// checks against the reviewed committed revision outside this entity.
    pub fn new(data: FindingDraft) -> Result<Self, FindingError> {
        for (name, value) in [
            ("title", &data.title),
            ("body", &data.body),
            ("evidence", &data.evidence),
        ] {
            if value.trim().is_empty() {
                return Err(FindingError::Blank(name));
            }
        }
        if data
            .title
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '|' | '<' | '>'))
        {
            return Err(FindingError::InvalidTitle);
        }
        if data
            .suggestion
            .as_ref()
            .is_some_and(|suggestion| suggestion.location() != &data.location)
        {
            return Err(FindingError::SuggestionLocationMismatch);
        }
        Ok(Self { data })
    }

    pub fn id(&self) -> FindingId {
        self.data.id
    }
    pub fn policy_id(&self) -> &PolicyId {
        &self.data.policy_id
    }
    pub fn title(&self) -> &str {
        &self.data.title
    }
    pub fn body(&self) -> &str {
        &self.data.body
    }
    pub fn severity(&self) -> Severity {
        self.data.severity
    }
    pub fn disposition(&self) -> Disposition {
        self.data.disposition
    }
    pub fn status(&self) -> FindingStatus {
        self.data.status
    }
    pub fn evidence(&self) -> &str {
        &self.data.evidence
    }
    /// The agent's validation claim; the flag cannot prove an independent check ran.
    pub fn validated(&self) -> bool {
        self.data.validated
    }
    pub fn location(&self) -> &SourceLocation {
        &self.data.location
    }
    pub fn suggestion(&self) -> Option<&Suggestion> {
        self.data.suggestion.as_ref()
    }

    /// Whether both versions record the same content, apart from their validation claims.
    ///
    /// Every other field counts, including title, body, evidence and suggestion.
    pub fn same_content(&self, other: &Self) -> bool {
        let content = |data: &FindingDraft| FindingDraft {
            validated: false,
            ..data.clone()
        };
        content(&self.data) == content(&other.data)
    }

    /// The same finding with its validation claim replaced; the claim constrains no content.
    #[must_use]
    pub fn with_validated(mut self, validated: bool) -> Self {
        self.data.validated = validated;
        self
    }

    /// Records a follow-up judgment about the same concern with a fresh explanation.
    /// Replaces status and evidence after rejecting a blank explanation or repeated status.
    /// Rejection leaves the finding unchanged. Identity, source location and suggestion
    /// are retained after a successful transition.
    /// Reopening retains any suggestion, whose revision [`crate::Assessment::new`]
    /// checks against the new assessment.
    pub fn transition(
        &mut self,
        next: FindingStatus,
        evidence: String,
    ) -> Result<FindingTransition, FindingError> {
        if evidence.trim().is_empty() {
            return Err(FindingError::Blank("transition evidence"));
        }
        if next == self.data.status {
            return Err(FindingError::UnchangedStatus);
        }
        let transition = FindingTransition::new(self.data.status, next, evidence.clone());
        self.data.status = next;
        self.data.evidence = evidence;
        Ok(transition)
    }
}

/// A finding construction or transition rejection. Error payloads exclude input text.
#[derive(Debug, Clone, PartialEq, Eq, displaydoc::Display, thiserror::Error)]
pub enum FindingError {
    /// The named title, body, evidence or transition explanation is blank.
    #[displaydoc("finding {0} must not be blank")]
    Blank(&'static str),
    /// The title contains CR, LF, `|`, `<` or `>`.
    #[displaydoc("finding title must be one line without |, <, or >")]
    InvalidTitle,
    /// The suggestion and finding name different source anchors.
    #[displaydoc("suggestion anchor must match its finding location")]
    SuggestionLocationMismatch,
    /// A transition requests the status the finding already has.
    #[displaydoc("finding already has the requested status")]
    UnchangedStatus,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::{LineNumber, SourcePath};
    type TestResult = Result<(), Box<dyn Error>>;

    fn draft() -> Result<FindingDraft, Box<dyn Error>> {
        Ok(FindingDraft {
            id: FindingId::try_from(1)?,
            policy_id: PolicyId::try_from("policy".to_owned())?,
            title: "Request is dropped".into(),
            body: "The error path discards the request.".into(),
            severity: Severity::P1,
            disposition: Disposition::Advisory,
            status: FindingStatus::Open,
            evidence: "Reproduced with an unavailable peer.".into(),
            validated: true,
            location: SourceLocation::new(
                SourcePath::try_from("src/lib.rs".to_owned())?,
                LineNumber::try_from(3)?,
            ),
            suggestion: None,
        })
    }

    #[test]
    fn transition_keeps_identity_and_requires_fresh_explanation() -> TestResult {
        let mut finding = Finding::new(draft()?)?;
        let before = finding.clone();
        assert_eq!(
            finding.transition(FindingStatus::Fixed, " ".into()),
            Err(FindingError::Blank("transition evidence"))
        );
        assert_eq!(finding, before);
        let fixed = finding.transition(
            FindingStatus::Fixed,
            "Retries are covered by the regression test.".into(),
        )?;
        assert_eq!(fixed.from(), FindingStatus::Open);
        assert_eq!(finding.status(), FindingStatus::Fixed);
        assert_eq!(finding.id(), before.id());
        assert_eq!(finding.evidence(), fixed.evidence());
        let recurrence = finding.transition(
            FindingStatus::Open,
            "The new branch bypasses the retry.".into(),
        )?;
        assert!(recurrence.is_recurrence());
        assert_eq!(
            finding.transition(FindingStatus::Open, "same evidence".into()),
            Err(FindingError::UnchangedStatus)
        );
        Ok(())
    }

    #[test]
    fn same_content_ignores_only_the_validation_claim() -> TestResult {
        let finding = Finding::new(draft()?)?;
        let unvalidated = finding.clone().with_validated(false);
        assert!(!unvalidated.validated());
        assert!(finding.same_content(&unvalidated));
        assert_ne!(finding, unvalidated);
        let mut reworded = draft()?;
        reworded.body = "The error path drops the request.".into();
        assert!(!finding.same_content(&Finding::new(reworded)?));
        let mut renumbered = draft()?;
        renumbered.id = FindingId::try_from(2)?;
        assert!(!finding.same_content(&Finding::new(renumbered)?));
        let mut changes: Vec<FindingDraft> = Vec::new();
        let mut moved = draft()?;
        moved.location = SourceLocation::new(
            SourcePath::try_from("src/lib.rs".to_owned())?,
            LineNumber::try_from(4)?,
        );
        changes.push(moved);
        let mut closed = draft()?;
        closed.status = FindingStatus::Fixed;
        changes.push(closed);
        let mut blocking = draft()?;
        blocking.disposition = Disposition::Blocking;
        changes.push(blocking);
        let mut suggested = draft()?;
        suggested.suggestion = Some(crate::Suggestion::new(
            suggested.location.clone(),
            crate::Revision::new("a".repeat(40), vec!["b".repeat(40)])?,
            LineNumber::try_from(3)?,
            vec!["    drop(request);".into()],
            vec!["    retry(request);".into()],
            "The retry keeps the request.".into(),
        )?);
        changes.push(suggested);
        for change in changes {
            assert!(!finding.same_content(&Finding::new(change)?));
        }
        Ok(())
    }

    #[test]
    fn severity_does_not_determine_disposition() -> TestResult {
        let finding = Finding::new(draft()?)?;
        assert_eq!(finding.severity(), Severity::P1);
        assert_eq!(finding.disposition(), Disposition::Advisory);
        Ok(())
    }

    #[test]
    fn construction_rejects_missing_evidence() -> TestResult {
        let mut input = draft()?;
        input.evidence = "\t".into();
        assert_eq!(Finding::new(input), Err(FindingError::Blank("evidence")));
        Ok(())
    }

    #[test]
    fn title_rejects_line_breaks_and_rendering_delimiters() -> TestResult {
        for title in [
            "first\nsecond",
            "first\rsecond",
            "first|second",
            "<first",
            "first>",
        ] {
            let mut input = draft()?;
            input.title = title.into();
            assert_eq!(Finding::new(input), Err(FindingError::InvalidTitle));
        }
        let mut input = draft()?;
        input.title = "Échec: ownership check (request #2)".into();
        let finding = Finding::new(input)?;
        assert_eq!(finding.title(), "Échec: ownership check (request #2)");
        Ok(())
    }
}
