//! Read-only loopback file server for trusted skill URLs.
//!
//! It serves regular files below one root directory with `GET` and `HEAD`,
//! answers 404 for anything else below the root, and records every request
//! so that a campaign can retain which sources a reviewer actually read.
//! Removing a file from the root makes its URL unreadable until it returns.

use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    io::{self, BufReader, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::http;

struct Shared {
    root: PathBuf,
    log: Mutex<(fs::File, u64)>,
}

pub struct FileServer {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl FileServer {
    pub fn start(root: &Path, listen: SocketAddr, log: &Path) -> Result<Self> {
        ensure!(
            listen.ip().is_loopback(),
            "the file server listens on loopback only"
        );
        let root = root
            .canonicalize()
            .with_context(|| format!("resolve file server root {}", root.display()))?;
        ensure!(root.is_dir(), "file server root must be a directory");
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new().create(true).append(true).open(log)?;
        let listener = TcpListener::bind(listen)?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            root,
            log: Mutex::new((file, 0)),
        });
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            thread::Builder::new()
                .name("fixture-file-server".to_owned())
                .spawn(move || {
                    for connection in listener.incoming() {
                        if stopping.load(Ordering::SeqCst) {
                            break;
                        }
                        if let Ok(client) = connection {
                            let shared = Arc::clone(&shared);
                            let _ = thread::Builder::new()
                                .name("fixture-file-request".to_owned())
                                .spawn(move || respond(client, &shared));
                        }
                    }
                })?
        };
        Ok(Self {
            address,
            stopping,
            acceptor: Some(acceptor),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn stop(mut self) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(5));
        if let Some(acceptor) = self.acceptor.take() {
            acceptor
                .join()
                .map_err(|_| anyhow::anyhow!("file server thread panicked"))?;
        }
        Ok(())
    }
}

fn respond(client: TcpStream, shared: &Shared) -> io::Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(client.try_clone()?);
    let Some(head) = http::read_head(&mut reader)? else {
        return Ok(());
    };
    let (method, target) = head.request_line()?;
    let (path, _) = http::path_and_query_keys(&target);
    let (status, body, kind) = match method.as_str() {
        "GET" | "HEAD" => match resolve(&shared.root, &path) {
            Some(file) => match fs::read(&file) {
                Ok(body) => (200, body, content_type(&file)),
                Err(_) => (404, Vec::new(), "text/plain"),
            },
            None => (404, Vec::new(), "text/plain"),
        },
        _ => (405, Vec::new(), "text/plain"),
    };
    {
        let mut log = shared
            .log
            .lock()
            .map_err(|_| io::Error::other("log lock poisoned"))?;
        log.1 += 1;
        let time = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(io::Error::other)?;
        let mut line = serde_json::to_vec(&json!({"sequence": log.1, "time": time, "method": method, "path": path, "status": status, "bytes": body.len()}))
            .map_err(io::Error::other)?;
        line.push(b'\n');
        log.0.write_all(&line)?;
        log.0.flush()?;
    }
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let mut output = io::BufWriter::new(&client);
    write!(
        output,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if method != "HEAD" {
        output.write_all(&body)?;
    }
    output.flush()?;
    drop(output);
    client.shutdown(Shutdown::Write)
}

/// Maps a URL path to a regular file below `root`; symbolic links that leave
/// the root, encoded characters and dot segments resolve to nothing.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    if path.contains(['%', '\\']) {
        return None;
    }
    let relative = Path::new(path.trim_start_matches('/'));
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let file = root.join(relative).canonicalize().ok()?;
    (file.starts_with(root) && file.is_file()).then_some(file)
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("md") => "text/markdown; charset=utf-8",
        Some("json") => "application/json",
        _ => "text/plain; charset=utf-8",
    }
}
