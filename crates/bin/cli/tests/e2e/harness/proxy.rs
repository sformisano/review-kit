//! Transparent forwarding proxy that records every exchange and can withhold
//! the response to one selected mutation.
//!
//! Each client connection carries one exchange: the proxy reads the complete
//! request, forwards it on a new upstream connection, reads the complete
//! response, and replies with `Connection: close`. The request head is
//! forwarded unchanged apart from hop-by-hop headers, so `Host` and
//! credentials reach the upstream exactly as the client sent them.
//!
//! The log is newline-delimited JSON with one entry per exchange. Entries
//! contain the method, the path without its query values, query parameter
//! names, status and sizes. They never contain headers or bodies.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufReader, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::http::{self, Head};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(300);

/// A path pattern where `*` matches within one path segment and `**` matches
/// across segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern(String);

impl PathPattern {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.starts_with('/') && !text.contains(['?', '#']),
            "path pattern must be an absolute path without query or fragment"
        );
        Ok(Self(text.to_owned()))
    }

    pub fn matches(&self, path: &str) -> bool {
        glob(self.0.as_bytes(), path.as_bytes())
    }
}

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) if rest.first() == Some(&b'*') => {
            (0..=text.len()).any(|index| glob(&rest[1..], &text[index..]))
        }
        Some((b'*', rest)) => {
            let limit = text
                .iter()
                .position(|byte| *byte == b'/')
                .unwrap_or(text.len());
            (0..=limit).any(|index| glob(rest, &text[index..]))
        }
        Some((byte, rest)) => text.first() == Some(byte) && glob(rest, &text[1..]),
    }
}

/// What the client observes for the withheld exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithheldResponse {
    /// Close the client connection as soon as the upstream response is complete.
    Close,
    /// Keep the client connection open without data, then close it.
    Hold(Duration),
    /// Forward the request upstream only after this delay, as a late original
    /// request, then close the client connection without a response.
    Delay(Duration),
}

/// Selects the `occurrence`-th mutation whose method and path match.
#[derive(Debug, Clone)]
pub struct Withhold {
    /// An HTTP method, or `*` for every mutation method.
    pub method: String,
    pub path: PathPattern,
    /// One-based count among matching mutations.
    pub occurrence: u64,
    pub response: WithheldResponse,
}

impl Withhold {
    fn matches(&self, method: &str, path: &str) -> bool {
        http::is_mutation(method)
            && (self.method == "*" || self.method.eq_ignore_ascii_case(method))
            && self.path.matches(path)
    }
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub listen: SocketAddr,
    /// Upstream `host:port`, resolved for each exchange.
    pub upstream: String,
    pub log: PathBuf,
    pub withhold: Option<Withhold>,
}

#[derive(Serialize)]
struct Exchange<'a> {
    sequence: u64,
    method: &'a str,
    path: &'a str,
    query_keys: &'a [String],
    mutation: bool,
    matched: bool,
    withheld: bool,
    status: Option<u16>,
    request_body_bytes: usize,
    response_body_bytes: Option<usize>,
    upstream_millis: u128,
    error: Option<String>,
}

struct Recorder {
    log: fs::File,
    sequence: u64,
    matched: u64,
}

struct Shared {
    upstream: String,
    withhold: Option<Withhold>,
    recorder: Mutex<Recorder>,
    active: AtomicUsize,
}

impl Shared {
    /// Assigns the exchange sequence and decides withholding in arrival order.
    fn admit(&self, method: &str, path: &str) -> io::Result<(u64, bool, bool)> {
        let mut recorder = self.recorder.lock().map_err(|_| poisoned())?;
        recorder.sequence += 1;
        let matched = self
            .withhold
            .as_ref()
            .is_some_and(|rule| rule.matches(method, path));
        if matched {
            recorder.matched += 1;
        }
        let withheld = matched
            && self
                .withhold
                .as_ref()
                .is_some_and(|rule| rule.occurrence == recorder.matched);
        Ok((recorder.sequence, matched, withheld))
    }

    fn record(&self, exchange: &Exchange<'_>) -> io::Result<()> {
        let mut line = serde_json::to_vec(exchange).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut recorder = self.recorder.lock().map_err(|_| poisoned())?;
        recorder.log.write_all(&line)?;
        recorder.log.flush()
    }
}

fn poisoned() -> io::Error {
    io::Error::other("proxy recorder lock is poisoned")
}

/// A running proxy. Dropping it without [`Proxy::stop`] leaves the accept
/// thread running until the process exits.
pub struct Proxy {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    shared: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

impl Proxy {
    /// Starts listening on a loopback address; clients send their provider
    /// credentials through the proxy, so it never listens beyond the host.
    pub fn start(config: ProxyConfig) -> Result<Self> {
        ensure!(
            config.listen.ip().is_loopback(),
            "the proxy listens on loopback only"
        );
        if let Some(rule) = &config.withhold {
            ensure!(rule.occurrence >= 1, "withheld occurrence starts at 1");
        }
        if let Some(parent) = config.log.parent() {
            fs::create_dir_all(parent)?;
        }
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config.log)
            .with_context(|| format!("open proxy log {}", config.log.display()))?;
        let listener = TcpListener::bind(config.listen)
            .with_context(|| format!("bind proxy listener {}", config.listen))?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            upstream: config.upstream,
            withhold: config.withhold,
            recorder: Mutex::new(Recorder {
                log,
                sequence: 0,
                matched: 0,
            }),
            active: AtomicUsize::new(0),
        });
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let shared = Arc::clone(&shared);
            let stopping = Arc::clone(&stopping);
            thread::Builder::new()
                .name("fixture-proxy-accept".to_owned())
                .spawn(move || accept(listener, &shared, &stopping))?
        };
        Ok(Self {
            address,
            stopping,
            shared,
            acceptor: Some(acceptor),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stops accepting connections and waits up to `grace` for in-flight
    /// exchanges, including held responses, to finish writing their entries.
    pub fn stop(mut self, grace: Duration) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        // Wake the blocking accept call.
        let _ = TcpStream::connect_timeout(&self.address, CONNECT_TIMEOUT);
        if let Some(acceptor) = self.acceptor.take() {
            acceptor
                .join()
                .map_err(|_| anyhow::anyhow!("proxy accept thread panicked"))?;
        }
        let deadline = Instant::now() + grace;
        while self.shared.active.load(Ordering::SeqCst) > 0 {
            ensure!(
                Instant::now() < deadline,
                "proxy exchanges did not finish within the shutdown grace period"
            );
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

fn accept(listener: TcpListener, shared: &Arc<Shared>, stopping: &AtomicBool) {
    for connection in listener.incoming() {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(client) = connection else { continue };
        shared.active.fetch_add(1, Ordering::SeqCst);
        let worker = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name("fixture-proxy-exchange".to_owned())
            .spawn(move || {
                let _ = exchange(client, &worker);
                worker.active.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            shared.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn exchange(client: TcpStream, shared: &Shared) -> io::Result<()> {
    client.set_read_timeout(Some(EXCHANGE_TIMEOUT))?;
    client.set_write_timeout(Some(EXCHANGE_TIMEOUT))?;
    let mut reader = BufReader::new(client.try_clone()?);
    let Some(head) = http::read_head(&mut reader)? else {
        return Ok(());
    };
    let (method, target) = head.request_line()?;
    if head
        .header("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        (&client).write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = http::read_body(&mut reader, head.request_framing()?)?;
    let (path, query_keys) = http::path_and_query_keys(&target);
    let (sequence, matched, withheld) = shared.admit(&method, &path)?;
    if withheld
        && let Some(WithheldResponse::Delay(duration)) =
            shared.withhold.as_ref().map(|rule| rule.response)
    {
        thread::sleep(duration);
    }
    let started = Instant::now();
    let forwarded = forward(&shared.upstream, &head, &body, &method);
    let mut entry = Exchange {
        sequence,
        method: &method,
        path: &path,
        query_keys: &query_keys,
        mutation: http::is_mutation(&method),
        matched,
        withheld,
        status: None,
        request_body_bytes: body.len(),
        response_body_bytes: None,
        upstream_millis: started.elapsed().as_millis(),
        error: None,
    };
    match forwarded {
        Ok((response, response_body)) => {
            entry.status = response.status().ok();
            entry.response_body_bytes = Some(response_body.len());
            shared.record(&entry)?;
            if withheld {
                if let Some(WithheldResponse::Hold(duration)) =
                    shared.withhold.as_ref().map(|rule| rule.response)
                {
                    thread::sleep(duration);
                }
                return client.shutdown(Shutdown::Both);
            }
            let mut output = io::BufWriter::new(&client);
            response.for_single_exchange().write_to(&mut output)?;
            output.write_all(&response_body)?;
            output.flush()?;
        }
        Err(error) => {
            entry.error = Some(error.to_string());
            shared.record(&entry)?;
            let message = b"proxy upstream unavailable\n";
            write!(
                &client,
                "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                message.len()
            )?;
            (&client).write_all(message)?;
        }
    }
    client.shutdown(Shutdown::Write)
}

fn forward(upstream: &str, head: &Head, body: &[u8], method: &str) -> io::Result<(Head, Vec<u8>)> {
    let address = upstream
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "upstream has no address"))?;
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(EXCHANGE_TIMEOUT))?;
    stream.set_write_timeout(Some(EXCHANGE_TIMEOUT))?;
    let mut output = io::BufWriter::new(&stream);
    head.for_single_exchange().write_to(&mut output)?;
    output.write_all(body)?;
    output.flush()?;
    drop(output);
    let mut reader = BufReader::new(&stream);
    loop {
        let response = http::read_head(&mut reader)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed before responding",
            )
        })?;
        let status = response.status()?;
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        let framing = response.response_framing(method)?;
        let response_body = http::read_body(&mut reader, framing)?;
        return Ok((response, response_body));
    }
}

/// Reads a proxy log and counts exchanges by method, mutation and outcome.
pub fn summarize(log: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(log).with_context(|| format!("read proxy log {}", log.display()))?;
    let mut methods = BTreeMap::<String, u64>::new();
    let mut mutations = Vec::new();
    let (mut total, mut errors, mut withheld) = (0_u64, 0_u64, Vec::new());
    for (index, line) in text.lines().enumerate() {
        let entry: Value = serde_json::from_str(line)
            .with_context(|| format!("proxy log line {} is not JSON", index + 1))?;
        let method = entry["method"].as_str().context("proxy log method")?;
        total += 1;
        *methods.entry(method.to_owned()).or_default() += 1;
        if !entry["error"].is_null() {
            errors += 1;
        }
        if entry["mutation"] == true {
            mutations.push(json!({"sequence": entry["sequence"], "method": method, "path": entry["path"], "status": entry["status"], "withheld": entry["withheld"]}));
        }
        if entry["withheld"] == true {
            withheld.push(json!({"sequence": entry["sequence"], "method": method, "path": entry["path"], "status": entry["status"]}));
        }
    }
    Ok(json!({
        "exchanges": total,
        "methods": methods,
        "mutation_requests": mutations.len(),
        "mutations": mutations,
        "withheld": withheld,
        "upstream_errors": errors,
    }))
}

/// Parses `close`, `hold:SECONDS` or `delay:SECONDS`.
pub fn parse_withheld_response(text: &str) -> Result<WithheldResponse> {
    match text.split_once(':') {
        None if text == "close" => Ok(WithheldResponse::Close),
        Some(("hold", seconds)) => Ok(WithheldResponse::Hold(Duration::from_secs(
            seconds.parse().context("hold duration in seconds")?,
        ))),
        Some(("delay", seconds)) => Ok(WithheldResponse::Delay(Duration::from_secs(
            seconds.parse().context("delay duration in seconds")?,
        ))),
        _ => bail!("withheld response must be `close`, `hold:SECONDS` or `delay:SECONDS`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_segments_and_subtrees() {
        let notes = PathPattern::parse("/api/v4/projects/*/merge_requests/*/notes").unwrap();
        assert!(notes.matches("/api/v4/projects/12/merge_requests/3/notes"));
        assert!(!notes.matches("/api/v4/projects/12/merge_requests/3/notes/9"));
        assert!(!notes.matches("/api/v4/projects/group%2Fp/x/merge_requests/3/notes"));
        let subtree = PathPattern::parse("/api/v4/**/discussions/**").unwrap();
        assert!(subtree.matches("/api/v4/projects/1/merge_requests/2/discussions/abc/notes"));
        assert!(!subtree.matches("/api/v4/projects/1/merge_requests/2/notes"));
        assert!(PathPattern::parse("api/v4").is_err());
        assert!(PathPattern::parse("/api/v4?x=1").is_err());
    }

    #[test]
    fn withheld_responses_parse() {
        assert_eq!(
            parse_withheld_response("close").unwrap(),
            WithheldResponse::Close
        );
        assert_eq!(
            parse_withheld_response("hold:90").unwrap(),
            WithheldResponse::Hold(Duration::from_secs(90))
        );
        assert_eq!(
            parse_withheld_response("delay:75").unwrap(),
            WithheldResponse::Delay(Duration::from_secs(75))
        );
        assert!(parse_withheld_response("hold").is_err());
        assert!(parse_withheld_response("drop:1").is_err());
    }

    #[test]
    fn rules_select_only_mutations() {
        let rule = Withhold {
            method: "*".to_owned(),
            path: PathPattern::parse("/api/v4/**").unwrap(),
            occurrence: 1,
            response: WithheldResponse::Close,
        };
        assert!(rule.matches("PUT", "/api/v4/projects/1"));
        assert!(!rule.matches("GET", "/api/v4/projects/1"));
        let post = Withhold {
            method: "post".to_owned(),
            ..rule
        };
        assert!(post.matches("POST", "/api/v4/projects/1"));
        assert!(!post.matches("DELETE", "/api/v4/projects/1"));
    }
}
