mod round;
mod tables;

use review_kit_application::publication::Round;
use review_kit_domain::ReviewerId;
use review_kit_observation::{Completion, Fields};
use serde::{Deserialize, Serialize};

use crate::{JsonError, domain::SubjectData, observation::json_error};
use round::CompactRound;
use tables::{Tables, ValidatedTables};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    version: u64,
    subject: SubjectData,
    reviewer: String,
    findings: Vec<crate::domain::FindingData>,
    policy_names: Vec<super::PolicyNameData>,
    current: CompactRound,
    history: Vec<CompactRound>,
}

#[review_kit_observation::operations]
impl State {
    pub const VERSION: u64 = 3;
    pub const MAX_BYTES: usize = 8 * 1024 * 1024;
    #[helper]
    pub fn from_rounds(current: &Round, history: &[Round]) -> Result<Self, JsonError> {
        super::validate_history(current, history)?;
        for retained in history.iter().chain(std::iter::once(current)) {
            for receipt in &retained.verified_resolutions {
                if &receipt.subject != current.assessment.subject()
                    || &receipt.reviewer != current.assessment.reviewer()
                    || receipt.round != retained.number
                {
                    return Err(JsonError::ContextBinding);
                }
            }
        }
        let mut tables = Tables::default();
        let compact_current = CompactRound::from_round(current, &mut tables)?;
        let compact_history = history
            .iter()
            .map(|retained| CompactRound::from_round(retained, &mut tables))
            .collect::<Result<_, _>>()?;
        let state = Self {
            version: Self::VERSION,
            subject: current.assessment.subject().into(),
            reviewer: current.assessment.reviewer().as_str().into(),
            findings: tables.findings,
            policy_names: tables.policy_names,
            current: compact_current,
            history: compact_history,
        };
        state.validate_tables()?;
        Ok(state)
    }

    /// Checks the version before interpreting its shape and bounds raw input.
    #[helper]
    pub fn decode(bytes: &[u8]) -> Result<Self, JsonError> {
        if bytes.len() > Self::MAX_BYTES {
            return Err(JsonError::RetainedStateLimit);
        }
        #[derive(Deserialize)]
        struct Version {
            version: u64,
        }
        let header: Version = crate::decode(bytes)?;
        if header.version != Self::VERSION {
            return Err(JsonError::Version(header.version));
        }
        crate::decode(bytes)
    }

    #[operation(
        name = "json.restore_rounds",
        input = Fields::new().count("history", self.history.len()),
        output = |result| Completion::result(
            result,
            |(_, history)| Fields::new().count("history", history.len()),
            json_error,
        ),
    )]
    pub fn into_rounds(self) -> Result<(Round, Vec<Round>), JsonError> {
        if self.version != Self::VERSION {
            return Err(JsonError::Version(self.version));
        }
        self.validate_tables()?;
        let tables = ValidatedTables::new(self.findings, self.policy_names)?;
        let subject = self.subject.into_subject()?;
        let reviewer = crate::domain::valid(ReviewerId::try_from(self.reviewer))?;
        let current = self.current.into_round(&subject, &reviewer, &tables)?;
        let history = self
            .history
            .into_iter()
            .map(|retained| retained.into_round(&subject, &reviewer, &tables))
            .collect::<Result<Vec<_>, _>>()?;
        super::validate_history(&current, &history)?;
        Ok((current, history))
    }

    #[helper]
    fn validate_tables(&self) -> Result<(), JsonError> {
        let bytes = serde_json::to_vec(self).map_err(JsonError::Syntax)?.len();
        if bytes > Self::MAX_BYTES {
            return Err(JsonError::RetainedStateLimit);
        }
        let header_bytes = serde_json::to_vec(&(&self.subject, &self.reviewer))
            .map_err(JsonError::Syntax)?
            .len();
        tables::validate(
            &self.findings,
            &self.policy_names,
            std::iter::once(&self.current).chain(&self.history),
            header_bytes,
            bytes,
        )
    }
}
