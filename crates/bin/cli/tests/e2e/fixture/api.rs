use anyhow::{Context, Result, ensure};
use reqwest::{Method, blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

pub struct Api {
    server: String,
    token: String,
    client: Client,
}
impl Api {
    pub fn new(server: &str, token: &str) -> Result<Self> {
        let token = token.trim();
        ensure!(
            !token.is_empty() && !token.contains(['\r', '\n']),
            "fixture credential is missing or invalid"
        );
        let url = url::Url::parse(server)?;
        ensure!(
            matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
                && url.username().is_empty()
                && url.password().is_none(),
            "fixture server must be a loopback URL without credentials"
        );
        Ok(Self {
            server: server.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            client: Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(30))
                .build()?,
        })
    }
    fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<(Value, String)> {
        ensure!(
            !path.starts_with('/') && !path.split('/').any(|part| part == ".."),
            "invalid fixture API path"
        );
        let mut request = self
            .client
            .request(method.clone(), format!("{}/api/v4/{path}", self.server))
            .header("PRIVATE-TOKEN", &self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .with_context(|| format!("fixture API {method} {path} unavailable"))?;
        response
            .error_for_status_ref()
            .with_context(|| format!("fixture API {method} {path} rejected the request"))?;
        ensure!(
            response.status().is_success(),
            "fixture API {method} {path} returned HTTP {}",
            response.status()
        );
        let next = response
            .headers()
            .get("X-Next-Page")
            .map(|value| value.to_str())
            .transpose()?
            .unwrap_or("")
            .to_owned();
        let mut raw = Vec::new();
        response.take(16 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
        ensure!(
            raw.len() <= 16 * 1024 * 1024,
            "fixture API response exceeds its size limit"
        );
        Ok((
            if raw.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&raw)?
            },
            next,
        ))
    }
    pub fn get(&self, path: &str) -> Result<Value> {
        Ok(self.call(Method::GET, path, None)?.0)
    }
    pub fn get_if_exists(&self, path: &str) -> Result<Option<Value>> {
        match self.get(path) {
            Ok(value) => Ok(Some(value)),
            Err(error)
                if error
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
                    == Some(reqwest::StatusCode::NOT_FOUND) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        Ok(self.call(Method::POST, path, Some(body))?.0)
    }
    #[allow(dead_code, reason = "campaign mutations edit provider state")]
    pub fn put(&self, path: &str, body: &Value) -> Result<Value> {
        Ok(self.call(Method::PUT, path, Some(body))?.0)
    }
    pub fn delete(&self, path: &str) -> Result<()> {
        self.call(Method::DELETE, path, None)?;
        Ok(())
    }
    pub fn pages(&self, path: &str) -> Result<Value> {
        let mut rows = Vec::new();
        let mut page = "1".to_owned();
        while !page.is_empty() {
            let (value, next) = self.call(
                Method::GET,
                &format!("{path}?per_page=100&page={page}"),
                None,
            )?;
            rows.extend(
                value
                    .as_array()
                    .context("fixture API page must be an array")?
                    .iter()
                    .cloned(),
            );
            ensure!(
                next.is_empty()
                    || next
                        .parse::<u32>()
                        .is_ok_and(|number| number <= 100 && number > page.parse().unwrap_or(0)),
                "fixture pagination exceeded its bound"
            );
            page = next;
        }
        Ok(json!(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::Api;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    fn project_response(status: &str, body: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream.write_all(response.as_bytes()).unwrap();
        });
        let result = Api::new(&format!("http://{address}"), "fixture-token")
            .unwrap()
            .get_if_exists("projects/2");
        server.join().unwrap();
        result
    }

    #[test]
    fn project_visibility_wait_accepts_only_not_found_as_pending() {
        assert_eq!(
            project_response("404 Not Found", r#"{"message":"404 Project Not Found"}"#).unwrap(),
            None
        );
        assert_eq!(
            project_response("200 OK", r#"{"id":2,"empty_repo":false}"#).unwrap(),
            Some(json!({"id":2,"empty_repo":false}))
        );
        for status in [
            "401 Unauthorized",
            "403 Forbidden",
            "429 Too Many Requests",
            "500 Internal Server Error",
        ] {
            assert!(project_response(status, "{}").is_err(), "{status}");
        }
        assert!(project_response("200 OK", "invalid json").is_err());
    }
}
