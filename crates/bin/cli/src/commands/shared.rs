use review_kit_domain::Subject;
use url::Url;

use crate::error::CliError;

pub const PREVIEW_NOTE: &str = "Preview counts and finding states use retained observations; publication recomputes them after verified effects.";

#[review_kit_observation::operations]
#[helper]
pub fn merge_request_url(subject: &Subject) -> Result<String, CliError> {
    let mut url = Url::parse(subject.host())
        .map_err(|error| CliError::adapter("parse retained host", "subject", 2, error))?;
    let mut segments = url.path_segments_mut().map_err(|()| CliError::Input {
        code: "subject",
        message: "retained host cannot form a merge-request URL".into(),
    })?;
    segments.pop_if_empty();
    for part in subject.repository().split('/') {
        segments.push(part);
    }
    segments.extend(["-", "merge_requests", &subject.request().to_string()]);
    drop(segments);
    Ok(url.into())
}
