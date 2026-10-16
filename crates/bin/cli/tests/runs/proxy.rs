//! One GitLab base URL serving merge request 7 from one fixture and merge request 8
//! from another, so several merge requests of one repository can share a worktree.
//!
//! Requests and responses are rewritten textually: the second fixture's
//! `merge_requests/7` and identifiers appear as merge request 8, and both
//! fixtures' base URLs appear as the proxy's.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use reqwest::{Method, blocking::Client, redirect::Policy};

use super::server;

pub struct Proxy {
    pub url: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub const SECOND_HEAD: &str = "5555555555555555555555555555555555555555";

impl Proxy {
    /// Serve `first` as merge request 7 and `second` as merge request 8.
    pub fn start(first: &server::Fixture, second: &server::Fixture) -> Self {
        second.state.lock().unwrap().head = SECOND_HEAD.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let routes = Routes {
            url: url.clone(),
            first: first.server.clone(),
            second: second.server.clone(),
            client: Client::builder().redirect(Policy::none()).build().unwrap(),
        };
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            for connection in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = connection else { continue };
                routes.serve(stream);
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Routes {
    url: String,
    first: String,
    second: String,
    client: Client,
}

impl Routes {
    fn serve(&self, mut stream: TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let Some((method, target, headers, body)) = read(&mut stream) else {
            return;
        };
        let second = target.contains("/merge_requests/8")
            || (target.contains("/repository/") && target.contains(SECOND_HEAD));
        let (base, target) = if second {
            (
                &self.second,
                target.replace("/merge_requests/8", "/merge_requests/7"),
            )
        } else {
            (&self.first, target)
        };
        let mut request = self.client.request(
            Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{base}{target}"),
        );
        for (name, value) in headers {
            if !matches!(name.as_str(), "host" | "content-length" | "connection") {
                request = request.header(name, value);
            }
        }
        let response = request.body(body).send().unwrap();
        let status = response.status().as_u16();
        let next = response
            .headers()
            .get("x-next-page")
            .map(|value| value.to_str().unwrap().to_owned());
        let mut text = String::from_utf8_lossy(&response.bytes().unwrap()).into_owned();
        text = text.replace(base.as_str(), &self.url);
        if second {
            for (from, to) in [
                ("merge_requests/7", "merge_requests/8"),
                ("\"iid\":7", "\"iid\":8"),
                ("\"noteable_iid\":7", "\"noteable_iid\":8"),
                ("\"noteable_id\":55", "\"noteable_id\":56"),
                ("\"id\":55,", "\"id\":56,"),
                ("\"merge_request_id\":55", "\"merge_request_id\":56"),
            ] {
                text = text.replace(from, to);
            }
        }
        let mut head = format!(
            "HTTP/1.1 {status} Proxy\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
            text.len()
        );
        if let Some(next) = next {
            head.push_str(&format!("x-next-page: {next}\r\n"));
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(text.as_bytes());
    }
}

type Request = (String, String, Vec<(String, String)>, Vec<u8>);

fn read(stream: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let offset = loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break offset + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..offset - 4]).into_owned();
    let mut lines = head.lines();
    let mut start = lines.next()?.split_whitespace();
    let method = start.next()?.to_owned();
    let target = start.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map_or(0, |(_, value)| value.parse().unwrap());
    while bytes.len() < offset + length {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Some((
        method,
        target,
        headers,
        bytes[offset..offset + length].to_vec(),
    ))
}
