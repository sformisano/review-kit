use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use url::Url;

use crate::GitLabError;

/// Secrets are supplied by the host and sent only in authentication headers.
#[derive(derive_more::Debug)]
#[debug("Credential([redacted])")]
pub struct Credential {
    headers: HeaderMap,
}
impl Credential {
    pub fn private_token(value: &str) -> Result<Self, GitLabError> {
        Self::header("private-token", value)
    }
    pub fn bearer_token(value: &str) -> Result<Self, GitLabError> {
        if value.trim().is_empty() {
            return Err(GitLabError::Config);
        }
        Self::header(AUTHORIZATION.as_str(), &format!("Bearer {value}"))
    }
    fn header(name: &'static str, value: &str) -> Result<Self, GitLabError> {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(GitLabError::Config);
        }
        let mut value = HeaderValue::from_str(value).map_err(|_| GitLabError::Config)?;
        value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(name, value);
        Ok(Self { headers })
    }
}
#[derive(Debug, Clone)]
pub struct Limits {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub response_bytes: usize,
    pub body_bytes: usize,
    pub pages: u32,
    pub total_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            response_bytes: 8 * 1024 * 1024,
            body_bytes: 1024 * 1024,
            pages: 100,
            total_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(derive_more::Debug)]
pub struct GitLabConfig {
    pub(crate) server: Url,
    #[debug(skip)]
    pub(crate) headers: HeaderMap,
    pub(crate) limits: Limits,
}
impl GitLabConfig {
    /// `server` includes the deployment prefix, but excludes `/api/v4`.
    /// Plain HTTP is accepted only for numeric loopback fixture servers.
    pub fn new(server: &str, credential: Credential) -> Result<Self, GitLabError> {
        let mut server = Url::parse(server).map_err(|_| GitLabError::Config)?;
        clean_url(&server)?;
        let local = server.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if server.scheme() != "https" && !(server.scheme() == "http" && local) {
            return Err(GitLabError::Config);
        }
        if server.path().split('/').any(|part| part.contains('%')) {
            return Err(GitLabError::Config);
        }
        if !server.path().ends_with('/') {
            server.set_path(&format!("{}/", server.path()));
        }
        Ok(Self {
            server,
            headers: credential.headers,
            limits: Limits::default(),
        })
    }
    pub fn with_limits(mut self, limits: Limits) -> Result<Self, GitLabError> {
        if limits.timeout.is_zero()
            || limits.connect_timeout.is_zero()
            || limits.response_bytes == 0
            || limits.body_bytes == 0
            || limits.pages == 0
            || limits.total_bytes < limits.response_bytes
            || limits.response_bytes == usize::MAX
        {
            return Err(GitLabError::Config);
        }
        self.limits = limits;
        Ok(self)
    }
}
pub(crate) fn clean_url(url: &Url) -> Result<(), GitLabError> {
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GitLabError::Config);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_reject_invalid_headers_and_redact_debug() -> Result<(), GitLabError> {
        for secret in ["", " ", "token\r\nX-Other: value"] {
            assert!(Credential::private_token(secret).is_err());
            assert!(Credential::bearer_token(secret).is_err());
        }
        for credential in [
            Credential::private_token("secret")?,
            Credential::bearer_token("secret")?,
        ] {
            assert_eq!(format!("{credential:?}"), "Credential([redacted])");
            assert_eq!(format!("{credential:#?}"), "Credential([redacted])");
        }
        Ok(())
    }
    #[test]
    fn configuration_debug_includes_server_and_excludes_credentials() -> Result<(), GitLabError> {
        let config = GitLabConfig::new(
            "https://private-host.example/private-deployment",
            Credential::private_token("private-token-marker")?,
        )?;
        for debug in [format!("{config:?}"), format!("{config:#?}")] {
            assert!(!debug.contains("private-token-marker"));
            assert!(debug.contains("private-host"));
            assert!(debug.contains("private-deployment"));
            assert!(debug.contains("limits"));
        }
        Ok(())
    }
    #[test]
    fn configuration_rejects_credentials_queries_and_unencrypted_remote_hosts() {
        for server in [
            "https://user:secret@gitlab.example",
            "https://gitlab.example?secret=a",
            "https://gitlab.example#secret",
            "http://gitlab.example",
            "http://localhost",
            "file:///repo",
            "https://gitlab.example/a%2fb",
        ] {
            let result =
                Credential::private_token("token").and_then(|c| GitLabConfig::new(server, c));
            assert!(result.is_err(), "accepted invalid server");
        }
    }
}
