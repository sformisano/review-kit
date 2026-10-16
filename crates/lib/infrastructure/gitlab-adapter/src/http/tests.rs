use super::*;
use crate::Credential;
use httpmock::MockServer;
use serde_json::json;
use std::{
    error::Error,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
};

type TestResult = Result<(), Box<dyn Error>>;

fn api(server: &str, limits: Limits) -> Result<Api, GitLabError> {
    Api::new(
        GitLabConfig::new(
            &format!("{server}/prefix/"),
            Credential::private_token("fixture-secret")?,
        )?
        .with_limits(limits)?,
    )
}

/// Raw responses exercise framing that a valid HTTP mock response cannot express.
struct RawResponse {
    url: String,
    thread: thread::JoinHandle<std::io::Result<()>>,
}

impl RawResponse {
    fn start(response: &'static str) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
            let mut reader = BufReader::new(&mut stream);
            let mut length = 0;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 {
                    return Err(std::io::Error::other("request closed before headers"));
                }
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().map_err(std::io::Error::other)?;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            stream.write_all(response.as_bytes())
        });
        Ok(Self { url, thread })
    }

    fn finish(self) -> TestResult {
        self.thread
            .join()
            .map_err(|_| "raw response worker panicked")??;
        Ok(())
    }
}

#[test]
fn real_http_encodes_parameters_and_preserves_authenticated_prefix() -> TestResult {
    let server = MockServer::start();
    let request = server.mock(|when, then| {
        when.method("GET")
            .is_true(|request| {
                request.uri().path_and_query().map(|value| value.as_str())
                    == Some("/prefix/api/v4/projects/group%2Fsub%20group%2F%C3%A9?ref=a%26b")
            })
            .header("private-token", "fixture-secret");
        then.status(200).body(r#"{"id":7}"#);
    });
    let value: Value = api(&server.base_url(), Limits::default())?.get(
        &format!("projects/{}", segment("group/sub group/é")),
        &[("ref", "a&b".into())],
    )?;
    assert_eq!(value, json!({"id":7}));
    request.assert_calls(1);
    Ok(())
}

#[test]
fn pagination_reads_all_pages_and_rejects_incomplete_tail() -> TestResult {
    for (tail_status, tail_body, expected) in [
        (200, "[2]", Ok(vec![1, 2])),
        (
            503,
            "{\"token\":\"fixture-secret\"}",
            Err(GitLabError::Rejected(503)),
        ),
    ] {
        let server = MockServer::start();
        let first = server.mock(|when, then| {
            when.method("GET")
                .path("/prefix/api/v4/notes")
                .query_param("page", "1")
                .query_param("per_page", "100");
            then.status(200).header("x-next-page", "2").body("[1]");
        });
        let second = server.mock(|when, then| {
            when.method("GET")
                .path("/prefix/api/v4/notes")
                .query_param("page", "2")
                .query_param("per_page", "100");
            then.status(tail_status)
                .header("x-next-page", "")
                .body(tail_body);
        });
        assert_eq!(
            api(&server.base_url(), Limits::default())?.pages::<u64>("notes", &[]),
            expected
        );
        first.assert_calls(1);
        second.assert_calls(1);
    }
    Ok(())
}

#[test]
fn pagination_bounds_and_malformed_continuations_fail() -> TestResult {
    for header in ["1", "3", "https://other/"] {
        let server = MockServer::start();
        let page = server.mock(|when, then| {
            when.method("GET").path("/prefix/api/v4/notes");
            then.status(200).header("x-next-page", header).body("[1]");
        });
        assert_eq!(
            api(&server.base_url(), Limits::default())?.pages::<u64>("notes", &[]),
            Err(GitLabError::Protocol)
        );
        page.assert_calls(1);
    }
    let server = MockServer::start();
    let page = server.mock(|when, then| {
        when.method("GET").path("/prefix/api/v4/notes");
        then.status(200).header("x-next-page", "2").body("[1]");
    });
    assert_eq!(
        api(
            &server.base_url(),
            Limits {
                pages: 1,
                ..Limits::default()
            }
        )?
        .pages::<u64>("notes", &[]),
        Err(GitLabError::Limit)
    );
    page.assert_calls(1);
    Ok(())
}

#[test]
fn missing_headers_on_a_full_page_require_following_read() -> TestResult {
    let server = MockServer::start();
    let full = serde_json::to_string(&vec![1u64; 100])?;
    let first = server.mock(|when, then| {
        when.method("GET")
            .path("/prefix/api/v4/notes")
            .query_param("page", "1");
        then.status(200).body(full);
    });
    let second = server.mock(|when, then| {
        when.method("GET")
            .path("/prefix/api/v4/notes")
            .query_param("page", "2");
        then.status(200).body("[]");
    });
    assert_eq!(
        api(&server.base_url(), Limits::default())?
            .pages::<u64>("notes", &[])?
            .len(),
        100
    );
    first.assert_calls(1);
    second.assert_calls(1);
    Ok(())
}

#[test]
fn redirects_never_forward_credentials_to_destination() -> TestResult {
    let destination = MockServer::start();
    let capture = destination.mock(|when, then| {
        when.any_request();
        then.status(200);
    });
    let server = MockServer::start();
    let redirect = server.mock(|when, then| {
        when.method("GET")
            .path("/prefix/api/v4/user")
            .header("private-token", "fixture-secret");
        then.status(302)
            .header("location", destination.url("/capture"));
    });
    assert_eq!(
        api(&server.base_url(), Limits::default())?.bytes("user", &[]),
        Err(GitLabError::Redirect)
    );
    redirect.assert_calls(1);
    capture.assert_calls(0);
    Ok(())
}

#[test]
fn read_size_limits_include_streamed_bodies() -> TestResult {
    let limits = Limits {
        response_bytes: 4,
        ..Limits::default()
    };
    let server = MockServer::start();
    let oversized = server.mock(|when, then| {
        when.method("GET").path("/prefix/api/v4/user");
        then.status(200).body("oversized");
    });
    assert_eq!(
        api(&server.base_url(), limits.clone())?.bytes("user", &[]),
        Err(GitLabError::Limit)
    );
    oversized.assert_calls(1);
    let streamed =
        RawResponse::start("HTTP/1.1 200 Fixture\r\nConnection: close\r\n\r\noversized")?;
    assert_eq!(
        api(&streamed.url, limits)?.bytes("user", &[]),
        Err(GitLabError::Limit)
    );
    streamed.finish()
}

#[test]
fn transport_errors_and_statuses_are_safe_and_writes_preserve_uncertainty() -> TestResult {
    for (code, expected) in [
        (401, GitLabError::Authentication),
        (403, GitLabError::Denied),
        (404, GitLabError::NotFound),
        (429, GitLabError::RateLimited),
        (400, GitLabError::Rejected(400)),
        (503, GitLabError::Uncertain),
        (408, GitLabError::Uncertain),
        (302, GitLabError::Uncertain),
        (202, GitLabError::Uncertain),
    ] {
        let server = MockServer::start();
        let write = server.mock(|when, then| {
            when.method("POST")
                .path("/prefix/api/v4/notes")
                .json_body(json!({"body":"public"}));
            then.status(code).body("fixture-secret private/path");
        });
        let error = api(&server.base_url(), Limits::default())?
            .mutate(Method::POST, "notes", &json!({"body":"public"}))
            .err()
            .ok_or("write unexpectedly passed")?;
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("fixture-secret"));
        write.assert_calls(1);
    }
    let truncated = RawResponse::start(
        "HTTP/1.1 201 Fixture\r\nContent-Length: 80\r\nConnection: close\r\n\r\nshort",
    )?;
    assert_eq!(
        api(&truncated.url, Limits::default())?.mutate(
            Method::POST,
            "notes",
            &json!({"body":"public"})
        ),
        Err(GitLabError::Uncertain)
    );
    truncated.finish()
}

#[test]
fn local_preflight_rejections_emit_no_write_attempt() -> TestResult {
    use review_kit_observation::{Observer, with_observer};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);

    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("captured log poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let server = MockServer::start();
    let write = server.mock(|when, then| {
        when.any_request();
        then.status(201);
    });
    let limited = api(
        &server.base_url(),
        Limits {
            body_bytes: 2,
            ..Limits::default()
        },
    )?;
    let client = api(&server.base_url(), Limits::default())?;
    let log = Log::default();
    with_observer(&Observer::json(log.clone()), || -> TestResult {
        assert_eq!(
            limited.mutate(Method::POST, "notes", &json!({"body":"private-body"})),
            Err(GitLabError::Limit)
        );
        assert_eq!(
            client.mutate(Method::POST, "../private-path", &json!({})),
            Err(GitLabError::Protocol)
        );
        write.assert_calls(0);
        client.mutate(Method::POST, "notes", &json!({"body":"private-body"}))?;
        Ok(())
    })?;
    write.assert_calls(1);
    let text = String::from_utf8(log.0.lock().map_err(|_| "captured log poisoned")?.clone())?;
    let records = text
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(records.len(), 2);
    let started = &records[0]["execution"];
    let completed = &records[1]["execution"];
    assert_eq!(started["operation"], "gitlab.http.write");
    assert_eq!(started["phase"], "started");
    assert_eq!(started["input"]["effect"], "attempted");
    assert_eq!(started["input"]["method"], "POST");
    assert!(started["input"]["bytes"].is_u64());
    assert_eq!(completed["phase"], "completed");
    assert_eq!(completed["output"]["effect"], "acknowledged");
    assert_eq!(started["execution_id"], completed["execution_id"]);
    for secret in ["private-body", "private-path", "fixture-secret"] {
        assert!(!text.contains(secret));
    }
    Ok(())
}
