//! Offline checks for live-campaign fixture tooling: the forwarding proxy,
//! the skill file server and the fixture family data. None of these tests
//! needs Docker, GitLab or network access beyond loopback sockets.

#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code, reason = "the campaign example uses the complete digest")]
#[path = "e2e/campaign/digest.rs"]
mod digest;
#[allow(dead_code, reason = "the campaign example uses the complete loader")]
#[path = "e2e/campaign/family.rs"]
mod family;
#[allow(dead_code, reason = "the campaign example uses the complete harness")]
#[path = "e2e/harness.rs"]
mod harness;

use harness::{
    files::FileServer,
    proxy::{PathPattern, Proxy, ProxyConfig, WithheldResponse, Withhold, summarize},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
struct Received {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// A scripted upstream with its own minimal HTTP parser, independent of the
/// proxy's framing code. It records every request it receives.
struct Upstream {
    address: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Upstream {
    fn start(respond: fn(&Received) -> Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let log = Arc::clone(&log);
                thread::spawn(move || {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap().to_owned();
                    let target = parts.next().unwrap().to_owned();
                    let mut headers = BTreeMap::new();
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        let header = header.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        let (key, value) = header.split_once(':').unwrap();
                        headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
                    }
                    let length = headers
                        .get("content-length")
                        .map_or(0, |value| value.parse::<usize>().unwrap());
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let request = Received {
                        method,
                        target,
                        headers,
                        body,
                    };
                    log.lock().unwrap().push(request.clone());
                    stream.write_all(&respond(&request)).unwrap();
                    stream.flush().unwrap();
                });
            }
        });
        Self { address, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn gitlab_like(request: &Received) -> Vec<u8> {
    let path = request.target.split('?').next().unwrap();
    match (request.method.as_str(), path) {
        ("GET", "/api/v4/projects/7/merge_requests") => {
            let body = r#"[{"iid":1}]"#;
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}", body.len()).into_bytes()
        }
        ("POST", _) => b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"id\r\n4\r\n\":5}\r\n0\r\n\r\n".to_vec(),
        ("PUT", _) => b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"updated\":true}".to_vec(),
        ("DELETE", _) => b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
        ("HEAD", _) => b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n".to_vec(),
        _ => b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
    }
}

fn proxy(upstream: &Upstream, log: &Path, withhold: Option<Withhold>) -> Proxy {
    Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: upstream.address.to_string(),
        log: log.to_owned(),
        withhold,
    })
    .unwrap()
}

fn client(timeout: Duration) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
        .unwrap()
}

fn entries(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn notes_rule(occurrence: u64, response: WithheldResponse) -> Withhold {
    Withhold {
        method: "POST".to_owned(),
        path: PathPattern::parse("/api/v4/projects/*/merge_requests/*/notes").unwrap(),
        occurrence,
        response,
    }
}

#[test]
fn proxy_forwards_exchanges_and_records_paths_without_query_values() {
    let upstream = Upstream::start(gitlab_like);
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("proxy.jsonl");
    let proxy = proxy(&upstream, &log, None);
    let base = format!("http://{}", proxy.address());
    let http = client(Duration::from_secs(10));

    let listed = http
        .get(format!(
            "{base}/api/v4/projects/7/merge_requests?private_token=query-secret&page=2"
        ))
        .header("PRIVATE-TOKEN", "header-secret")
        .send()
        .unwrap();
    assert_eq!(listed.status(), 200);
    assert_eq!(listed.text().unwrap(), r#"[{"iid":1}]"#);
    let created = http
        .post(format!("{base}/api/v4/projects/7/merge_requests/1/notes"))
        .json(&json!({"body": "first"}))
        .send()
        .unwrap();
    assert_eq!(created.status(), 201);
    assert_eq!(created.json::<Value>().unwrap(), json!({"id": 5}));
    let updated = http
        .put(format!("{base}/api/v4/projects/7/merge_requests/1"))
        .json(&json!({"description": "edited"}))
        .send()
        .unwrap();
    assert_eq!(updated.status(), 200);
    assert_eq!(updated.text().unwrap(), r#"{"updated":true}"#);
    let deleted = http
        .delete(format!("{base}/api/v4/projects/7/merge_requests/1/notes/5"))
        .send()
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let head = http.head(format!("{base}/api/v4/version")).send().unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], "17");
    proxy.stop(Duration::from_secs(10)).unwrap();

    let received = upstream.received();
    assert_eq!(
        received
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["GET", "POST", "PUT", "DELETE", "HEAD"]
    );
    assert_eq!(
        received[0].target,
        "/api/v4/projects/7/merge_requests?private_token=query-secret&page=2"
    );
    assert_eq!(received[0].headers["private-token"], "header-secret");
    assert_eq!(
        received[0].headers["host"],
        base.trim_start_matches("http://")
    );
    assert!(
        received
            .iter()
            .all(|request| request.headers["connection"] == "close")
    );
    assert_eq!(received[1].body, br#"{"body":"first"}"#);

    let recorded = entries(&log);
    assert_eq!(
        recorded
            .iter()
            .map(|entry| (
                entry["sequence"].as_u64().unwrap(),
                entry["method"].as_str().unwrap(),
                entry["path"].as_str().unwrap(),
                entry["status"].as_u64().unwrap(),
                entry["mutation"].as_bool().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            (1, "GET", "/api/v4/projects/7/merge_requests", 200, false),
            (
                2,
                "POST",
                "/api/v4/projects/7/merge_requests/1/notes",
                201,
                true
            ),
            (3, "PUT", "/api/v4/projects/7/merge_requests/1", 200, true),
            (
                4,
                "DELETE",
                "/api/v4/projects/7/merge_requests/1/notes/5",
                204,
                true
            ),
            (5, "HEAD", "/api/v4/version", 200, false),
        ]
    );
    assert_eq!(recorded[0]["query_keys"], json!(["private_token", "page"]));
    assert!(recorded.iter().all(|entry| entry["withheld"] == false));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        !text.contains("query-secret")
            && !text.contains("header-secret")
            && !text.contains("first")
    );
    let summary = summarize(&log).unwrap();
    assert_eq!(summary["exchanges"], 5);
    assert_eq!(summary["mutation_requests"], 3);
    assert_eq!(summary["withheld"], json!([]));
}

#[test]
fn proxy_withholds_the_selected_mutation_after_upstream_completes() {
    let upstream = Upstream::start(gitlab_like);
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("proxy.jsonl");
    let proxy = proxy(
        &upstream,
        &log,
        Some(notes_rule(2, WithheldResponse::Close)),
    );
    let base = format!("http://{}", proxy.address());
    let http = client(Duration::from_secs(10));
    let notes = format!("{base}/api/v4/projects/7/merge_requests/1/notes");

    assert_eq!(
        http.post(&notes)
            .json(&json!({"body": "one"}))
            .send()
            .unwrap()
            .status(),
        201
    );
    let other = http
        .post(format!(
            "{base}/api/v4/projects/7/merge_requests/1/discussions"
        ))
        .json(&json!({"body": "thread"}))
        .send()
        .unwrap();
    assert_eq!(other.status(), 201);
    let lost = http.post(&notes).json(&json!({"body": "two"})).send();
    let error = lost.expect_err("the withheld response must not reach the client");
    assert!(!error.is_timeout(), "{error}");
    let after = http
        .get(format!("{base}/api/v4/projects/7/merge_requests"))
        .send()
        .unwrap();
    assert_eq!(after.status(), 200);
    proxy.stop(Duration::from_secs(10)).unwrap();

    let received = upstream.received();
    assert_eq!(received.len(), 4);
    assert_eq!(
        received[2].body, br#"{"body":"two"}"#,
        "the withheld request reached upstream"
    );
    let recorded = entries(&log);
    assert_eq!(
        recorded
            .iter()
            .map(|entry| (entry["matched"] == true, entry["withheld"] == true))
            .collect::<Vec<_>>(),
        [(true, false), (false, false), (true, true), (false, false)]
    );
    assert_eq!(
        recorded[2]["status"], 201,
        "upstream completed the withheld write"
    );
    let summary = summarize(&log).unwrap();
    assert_eq!(summary["mutation_requests"], 3);
    assert_eq!(
        summary["withheld"],
        json!([{"sequence": 3, "method": "POST", "path": "/api/v4/projects/7/merge_requests/1/notes", "status": 201}])
    );
}

#[test]
fn proxy_holds_a_withheld_response_past_the_client_timeout() {
    let upstream = Upstream::start(gitlab_like);
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("proxy.jsonl");
    let proxy = proxy(
        &upstream,
        &log,
        Some(notes_rule(
            1,
            WithheldResponse::Hold(Duration::from_secs(3)),
        )),
    );
    let started = Instant::now();
    let error = client(Duration::from_secs(1))
        .post(format!(
            "http://{}/api/v4/projects/7/merge_requests/1/notes",
            proxy.address()
        ))
        .json(&json!({"body": "held"}))
        .send()
        .expect_err("the held response must not reach the client");
    assert!(error.is_timeout(), "{error}");
    assert!(started.elapsed() < Duration::from_millis(2500));
    proxy.stop(Duration::from_secs(10)).unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "stop waits for the held exchange"
    );
    assert_eq!(upstream.received()[0].body, br#"{"body":"held"}"#);
    let recorded = entries(&log);
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        (
            recorded[0]["withheld"].clone(),
            recorded[0]["status"].clone()
        ),
        (json!(true), json!(201))
    );
}

#[test]
fn proxy_delays_a_withheld_request_until_after_the_client_gave_up() {
    let upstream = Upstream::start(gitlab_like);
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("proxy.jsonl");
    let proxy = proxy(
        &upstream,
        &log,
        Some(notes_rule(
            1,
            WithheldResponse::Delay(Duration::from_secs(3)),
        )),
    );
    let started = Instant::now();
    let error = client(Duration::from_secs(1))
        .post(format!(
            "http://{}/api/v4/projects/7/merge_requests/1/notes",
            proxy.address()
        ))
        .json(&json!({"body": "late"}))
        .send()
        .expect_err("the delayed request gets no response");
    assert!(error.is_timeout(), "{error}");
    assert!(
        upstream.received().is_empty(),
        "the request has not reached upstream when the client gives up"
    );
    proxy.stop(Duration::from_secs(10)).unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert_eq!(upstream.received()[0].body, br#"{"body":"late"}"#);
    let recorded = entries(&log);
    assert_eq!(
        (
            recorded[0]["withheld"].clone(),
            recorded[0]["status"].clone()
        ),
        (json!(true), json!(201))
    );
}

#[test]
fn proxy_refuses_to_listen_beyond_loopback() {
    let directory = tempfile::tempdir().unwrap();
    let started = Proxy::start(ProxyConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        upstream: "127.0.0.1:9".to_owned(),
        log: directory.path().join("proxy.jsonl"),
        withhold: None,
    });
    assert!(started.is_err());
}

#[test]
fn proxy_reports_an_unavailable_upstream() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("proxy.jsonl");
    let proxy = Proxy::start(ProxyConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: address.to_string(),
        log: log.clone(),
        withhold: None,
    })
    .unwrap();
    let response = client(Duration::from_secs(10))
        .get(format!("http://{}/api/v4/version", proxy.address()))
        .send()
        .unwrap();
    assert_eq!(response.status(), 502);
    proxy.stop(Duration::from_secs(10)).unwrap();
    let recorded = entries(&log);
    assert!(recorded[0]["status"].is_null() && recorded[0]["error"].is_string());
    assert_eq!(summarize(&log).unwrap()["upstream_errors"], 1);
}

fn raw_get(address: SocketAddr, target: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "GET {target} HTTP/1.1\r\nHost: skills\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn file_server_serves_only_files_below_its_root_and_records_requests() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("served");
    std::fs::create_dir_all(root.join("team-policy")).unwrap();
    std::fs::write(root.join("team-policy/SKILL.md"), "# Team policy\n").unwrap();
    std::fs::write(directory.path().join("outside.md"), "private\n").unwrap();
    std::os::unix::fs::symlink(directory.path().join("outside.md"), root.join("escape.md"))
        .unwrap();
    let log = directory.path().join("requests.jsonl");
    let server = FileServer::start(&root, "127.0.0.1:0".parse().unwrap(), &log).unwrap();
    let base = format!("http://{}", server.address());
    let http = client(Duration::from_secs(10));

    let found = http
        .get(format!("{base}/team-policy/SKILL.md"))
        .send()
        .unwrap();
    assert_eq!(found.status(), 200);
    assert_eq!(
        found.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(found.text().unwrap(), "# Team policy\n");
    assert_eq!(
        http.get(format!("{base}/missing/SKILL.md"))
            .send()
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.get(format!("{base}/escape.md"))
            .send()
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.post(format!("{base}/team-policy/SKILL.md"))
            .send()
            .unwrap()
            .status(),
        405
    );
    assert!(raw_get(server.address(), "/../outside.md").starts_with("HTTP/1.1 404"));
    assert!(raw_get(server.address(), "/team-policy/%53KILL.md").starts_with("HTTP/1.1 404"));
    std::fs::remove_file(root.join("team-policy/SKILL.md")).unwrap();
    assert_eq!(
        http.get(format!("{base}/team-policy/SKILL.md"))
            .send()
            .unwrap()
            .status(),
        404
    );
    server.stop().unwrap();

    let recorded = entries(&log);
    assert_eq!(
        recorded
            .iter()
            .map(|entry| (
                entry["path"].as_str().unwrap(),
                entry["status"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("/team-policy/SKILL.md", 200),
            ("/missing/SKILL.md", 404),
            ("/escape.md", 404),
            ("/team-policy/SKILL.md", 405),
            ("/../outside.md", 404),
            ("/team-policy/%53KILL.md", 404),
            ("/team-policy/SKILL.md", 404),
        ]
    );
}
