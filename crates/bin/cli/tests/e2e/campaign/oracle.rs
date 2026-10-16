//! Independent REST observations of one merge request.
//!
//! An observation reads the merge request, every note page, every discussion
//! page and the diff versions through the fixture's own REST client, never
//! through `rkc`. It saves the raw responses with a compact digest so that
//! before and after states can be compared later. Absence is meaningful only
//! because every read must succeed: a failed page fails the observation.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::PathBuf;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{Campaign, Role, Target, digest};
use crate::fixture::marker;

impl Campaign {
    /// Records a complete observation as `evidence/oracle/<name>.json` and
    /// returns its compact digest. Names are unique within a campaign.
    pub fn observe(&self, target: &Target, name: &str) -> Result<(PathBuf, Value)> {
        ensure!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '.'))
                && !name.starts_with('.'),
            "observation names use letters, digits, dots, hyphens and underscores"
        );
        let path = self
            .directory
            .join("evidence/oracle")
            .join(format!("{name}.json"));
        ensure!(!path.exists(), "observation {name} already exists");
        self.verify_merge_request(target)?;
        let api = self.api(Role::Author)?;
        let prefix = self.merge_request_path(target)?;
        let raw = json!({
            "merge_request": api.get(&prefix)?,
            "notes": api.pages(&format!("{prefix}/notes"))?,
            "discussions": api.pages(&format!("{prefix}/discussions"))?,
            "versions": api.pages(&format!("{prefix}/versions"))?,
        });
        let summary = digest::summarize(&raw, self.user_id(Role::Reviewer)?, marker);
        let observed_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let leaks = self.private_text(&raw);
        let observation = json!({"name": name, "family": target.family, "merge_request": target.label, "observed_at": observed_at, "summary": summary, "private_text_in_provider_bodies": leaks, "raw": raw});
        let text = serde_json::to_string(&observation)?;
        ensure!(
            !self.tokens.iter().any(|token| text.contains(token)),
            "provider state contains a fixture credential; observation not saved"
        );
        crate::fixture::write_json(&path, &observation)?;
        ensure!(
            leaks.is_empty(),
            "provider bodies contain private campaign text: {leaks:?}; see {}",
            path.display()
        );
        Ok((path, summary))
    }

    /// Notes whose bodies contain this campaign's private directory path.
    fn private_text(&self, raw: &Value) -> Vec<Value> {
        let directory = self.directory.to_string_lossy();
        raw["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|note| {
                note["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(directory.as_ref()))
            })
            .map(|note| note["id"].clone())
            .collect()
    }

    pub fn load_observation(&self, name: &str) -> Result<Value> {
        crate::fixture::read_json(
            self.directory
                .join("evidence/oracle")
                .join(format!("{name}.json")),
        )
        .with_context(|| format!("read observation {name}"))
    }
}
