use crate::observation;
use review_kit_observation::Fields;
use std::io::Read;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::{Method, blocking::Client, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use crate::{GitLabConfig, GitLabError, Limits};

const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');
#[review_kit_observation::operations]
#[helper]
pub(crate) fn segment(value: &str) -> String {
    utf8_percent_encode(value, SEGMENT).to_string()
}

#[derive(Clone)]
pub(crate) struct Api {
    client: Client,
    base: Url,
    pub(crate) server: Url,
    pub(crate) limits: Limits,
}
#[review_kit_observation::operations]
impl Api {
    #[helper]
    pub(crate) fn new(config: GitLabConfig) -> Result<Self, GitLabError> {
        let client = Client::builder()
            .default_headers(config.headers)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(config.limits.timeout)
            .connect_timeout(config.limits.connect_timeout)
            .user_agent("review-kit-gitlab-adapter/0.1")
            .build()
            .map_err(|_| GitLabError::Config)?;
        let base = config
            .server
            .join("api/v4/")
            .map_err(|_| GitLabError::Config)?;
        Ok(Self {
            client,
            base,
            server: config.server,
            limits: config.limits,
        })
    }
    #[helper]
    fn url(&self, path: &str) -> Result<Url, GitLabError> {
        if path.starts_with('/')
            || path.contains('?')
            || path.contains('#')
            || path.split('/').any(|part| part == "." || part == "..")
        {
            return Err(GitLabError::Protocol);
        }
        let url = self.base.join(path).map_err(|_| GitLabError::Protocol)?;
        if url.origin() != self.base.origin() || !url.path().starts_with(self.base.path()) {
            return Err(GitLabError::Protocol);
        }
        Ok(url)
    }
    #[helper]
    pub(crate) fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, GitLabError> {
        let bytes = self.bytes(path, query)?;
        serde_json::from_slice(&bytes).map_err(|_| GitLabError::Protocol)
    }
    #[operation(
        name = "gitlab.http.read",
        input = Fields::new()
            .secret("path", path)
            .count("query_parameters", query.len()),
        output = |result| {
            observation::result(result, |(bytes, _)| {
                Fields::new().count("bytes", bytes.len())
            })
        },
    )]
    fn read(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<(Vec<u8>, reqwest::header::HeaderMap), GitLabError> {
        let mut response = self
            .client
            .get(self.url(path)?)
            .query(query)
            .send()
            .map_err(|_| GitLabError::Transport)?;
        status(response.status().as_u16())?;
        let headers = response.headers().clone();
        let bytes = bounded(&mut response, self.limits.response_bytes)?;
        Ok((bytes, headers))
    }
    #[helper]
    pub(crate) fn bytes(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, GitLabError> {
        self.read(path, query).map(|(bytes, _)| bytes)
    }
    #[operation(
        name = "gitlab.http.pages",
        input = Fields::new()
            .secret("path", path)
            .count("query_parameters", query.len()),
        output = |result| observation::result(result, |items| Fields::new().count("items", items.len())),
    )]
    pub(crate) fn pages<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>, GitLabError> {
        let mut output = Vec::new();
        let mut total = 0usize;
        for page in 1..=self.limits.pages {
            let mut query = query.to_vec();
            query.extend([("page", page.to_string()), ("per_page", "100".into())]);
            let (bytes, headers) = self.read(path, &query)?;
            total = total.checked_add(bytes.len()).ok_or(GitLabError::Limit)?;
            if total > self.limits.total_bytes {
                return Err(GitLabError::Limit);
            }
            let items: Vec<T> =
                serde_json::from_slice(&bytes).map_err(|_| GitLabError::Protocol)?;
            let count = items.len();
            if count > 100 {
                return Err(GitLabError::Protocol);
            }
            output.extend(items);
            // GitLab.com can omit pagination headers. An empty following page
            // ends a full page without inferring an omitted tail.
            match headers.get("x-next-page") {
                Some(next) => {
                    let next = next.to_str().map_err(|_| GitLabError::Protocol)?;
                    if next.is_empty() {
                        return Ok(output);
                    }
                    if next.parse::<u32>().ok() != page.checked_add(1) {
                        return Err(GitLabError::Protocol);
                    }
                }
                None if count < 100 => return Ok(output),
                None => {}
            }
        }
        Err(GitLabError::Limit)
    }
    #[cfg(test)]
    #[helper]
    pub(crate) fn mutate(
        &self,
        method: Method,
        path: &str,
        payload: &Value,
    ) -> Result<(), GitLabError> {
        self.mutate_with_checkpoint(method, path, payload, &mut || Ok(()))
    }

    #[helper]
    pub(crate) fn mutate_with_checkpoint(
        &self,
        method: Method,
        path: &str,
        payload: &Value,
        before_dispatch: &mut dyn FnMut() -> Result<(), GitLabError>,
    ) -> Result<(), GitLabError> {
        let bytes = serde_json::to_vec(payload).map_err(|_| GitLabError::Protocol)?;
        if bytes.len() > self.limits.body_bytes {
            return Err(GitLabError::Limit);
        }
        let url = self.url(path)?;
        self.dispatch(method, path, url, bytes, before_dispatch)
    }

    #[operation(
        name = "gitlab.http.write",
        input = Fields::new()
            .label("method", match method {
                Method::POST => "POST",
                Method::PUT => "PUT",
                _ => "other",
            })
            .label("effect", "attempted")
            .secret("path", path)
            .secret("payload", "provider payload")
            .count("bytes", bytes.len()),
        output = observation::mutation_result,
    )]
    fn dispatch(
        &self,
        method: Method,
        path: &str,
        url: Url,
        bytes: Vec<u8>,
        before_dispatch: &mut dyn FnMut() -> Result<(), GitLabError>,
    ) -> Result<(), GitLabError> {
        let request = self
            .client
            .request(method, url)
            .header("content-type", "application/json")
            .body(bytes)
            .build()
            .map_err(|_| GitLabError::Protocol)?;
        before_dispatch()?;
        let mut response = self
            .client
            .execute(request)
            .map_err(|_| GitLabError::Uncertain)?;
        let code = response.status().as_u16();
        // Server errors can follow a committed mutation; they do not prove rejection.
        if code >= 500 || code == 408 || code == 202 || (300..400).contains(&code) {
            return Err(GitLabError::Uncertain);
        }
        status(code)?;
        bounded(&mut response, self.limits.response_bytes).map_err(|_| GitLabError::Uncertain)?;
        Ok(())
    }
}
#[review_kit_observation::operations]
#[helper]
fn bounded(
    response: &mut reqwest::blocking::Response,
    limit: usize,
) -> Result<Vec<u8>, GitLabError> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(GitLabError::Limit);
    }
    let mut bytes = Vec::new();
    response
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GitLabError::Transport)?;
    if bytes.len() > limit {
        return Err(GitLabError::Limit);
    }
    Ok(bytes)
}
#[review_kit_observation::operations]
#[helper]
fn status(code: u16) -> Result<(), GitLabError> {
    match code {
        200..=299 => Ok(()),
        300..=399 => Err(GitLabError::Redirect),
        401 => Err(GitLabError::Authentication),
        403 => Err(GitLabError::Denied),
        404 => Err(GitLabError::NotFound),
        429 => Err(GitLabError::RateLimited),
        _ => Err(GitLabError::Rejected(code)),
    }
}

#[cfg(test)]
mod tests;
