use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};
use percent_encoding::percent_decode_str;
use review_kit_gitlab_adapter::{Credential, GitLab, GitLabConfig, Limits};
use serde_json::Value;
use url::Url;

use super::TOKEN;
mod state;
pub use state::State;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub payload: Value,
}

#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&value).unwrap(),
        }
    }
    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: vec![],
            body: b"fixture-private-secret private provider detail".to_vec(),
        }
    }
}

pub enum Fault {
    Respond {
        method: String,
        target: String,
        response: Response,
        skip_matches: usize,
    },
    LoseWrite {
        method: String,
        target: String,
        commit: bool,
    },
    CommitThenDrift {
        target: String,
    },
    LateCreation {
        target: String,
        original: Request,
    },
    HoldRead {
        target: String,
        ready: SyncSender<()>,
        release: Receiver<()>,
    },
    /// Hold a read until released, then answer it normally.
    PauseRead {
        target: String,
        ready: SyncSender<()>,
        release: Receiver<()>,
    },
    /// Answer a read normally after `skip_matches` earlier matches, then
    /// apply `change`, as another client acting between two reads would.
    ChangeAfterRead {
        target: String,
        skip_matches: usize,
        change: Box<dyn FnOnce(&mut State) + Send>,
    },
}

impl Fault {
    pub fn response(method: &str, target: &str, response: Response) -> Self {
        Self::Respond {
            method: method.into(),
            target: target.into(),
            response,
            skip_matches: 0,
        }
    }
    #[allow(
        dead_code,
        reason = "The CLI integration suite shares this HTTP fixture."
    )]
    pub fn response_after(
        method: &str,
        target: &str,
        skip_matches: usize,
        response: Response,
    ) -> Self {
        Self::Respond {
            method: method.into(),
            target: target.into(),
            response,
            skip_matches,
        }
    }
    pub fn lost(target: &str, commit: bool) -> Self {
        Self::LoseWrite {
            method: "POST".into(),
            target: target.into(),
            commit,
        }
    }
    pub fn lost_method(method: &str, target: &str, commit: bool) -> Self {
        Self::LoseWrite {
            method: method.into(),
            target: target.into(),
            commit,
        }
    }
    pub fn drift(target: &str) -> Self {
        Self::CommitThenDrift {
            target: target.into(),
        }
    }
    #[allow(
        dead_code,
        reason = "The CLI restart suite exercises delayed original creations."
    )]
    pub fn late_creation(target: &str, original: Request) -> Self {
        Self::LateCreation {
            target: target.into(),
            original,
        }
    }
    pub fn hold(target: &str, ready: SyncSender<()>, release: Receiver<()>) -> Self {
        Self::HoldRead {
            target: target.into(),
            ready,
            release,
        }
    }
    #[allow(
        dead_code,
        reason = "The CLI integration suite pauses a command between two of its reads."
    )]
    pub fn pause(target: &str, ready: SyncSender<()>, release: Receiver<()>) -> Self {
        Self::PauseRead {
            target: target.into(),
            ready,
            release,
        }
    }
    #[allow(
        dead_code,
        reason = "The CLI integration suite changes the merge request between two reads of one command."
    )]
    pub fn change_after_read(
        target: &str,
        skip_matches: usize,
        change: impl FnOnce(&mut State) + Send + 'static,
    ) -> Self {
        Self::ChangeAfterRead {
            target: target.into(),
            skip_matches,
            change: Box::new(change),
        }
    }
    fn matches(&self, request: &Request) -> bool {
        match self {
            Self::Respond { method, target, .. } => {
                request.method == *method && (request.path == *target || request.target == *target)
            }
            Self::LoseWrite { method, target, .. } => {
                request.method == *method && request.path == *target
            }
            Self::CommitThenDrift { target } => request.method == "POST" && request.path == *target,
            Self::LateCreation { target, .. } => {
                request.method == "POST" && request.path == *target
            }
            Self::HoldRead { target, .. }
            | Self::PauseRead { target, .. }
            | Self::ChangeAfterRead { target, .. } => {
                request.method == "GET" && request.path == *target
            }
        }
    }
}

pub struct Fixture {
    pub state: Arc<Mutex<State>>,
    pub server: String,
    transport: Transport,
}

enum Transport {
    Mock {
        _server: MockServer,
    },
    LostResponses {
        address: SocketAddr,
        worker: Option<JoinHandle<()>>,
    },
}

impl Fixture {
    pub fn new() -> Self {
        let mock = MockServer::start();
        let server = mock.base_url();
        let state = Arc::new(Mutex::new(State::new(server.clone())));
        let shared = state.clone();
        mock.mock(|when, then| {
            when.any_request();
            then.respond_with(move |request: &HttpMockRequest| {
                let response = shared
                    .lock()
                    .unwrap()
                    .serve(Request::from(request))
                    .expect("lost-response faults require Fixture::with_lost_responses");
                HttpMockResponse {
                    status: Some(response.status),
                    headers: Some(response.headers),
                    body: Some(response.body.into()),
                }
            });
        });
        Self {
            state,
            server,
            transport: Transport::Mock { _server: mock },
        }
    }

    /// A real connection close after optional commit proves lost-response recovery.
    pub fn with_lost_responses() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = format!("http://{address}");
        let state = Arc::new(Mutex::new(State::new(server.clone())));
        let shared = state.clone();
        let worker = thread::spawn(move || {
            for connection in listener.incoming() {
                let mut stream = connection.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = read_request(&mut stream);
                if request.target == "/__stop" {
                    return;
                }
                let response = shared.lock().unwrap().serve(request);
                if let Some(response) = response {
                    let mut headers = format!(
                        "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
                        response.status,
                        response.body.len()
                    );
                    for (name, value) in response.headers {
                        headers.push_str(&format!("{name}: {value}\r\n"));
                    }
                    headers.push_str("\r\n");
                    stream.write_all(headers.as_bytes()).unwrap();
                    stream.write_all(&response.body).unwrap();
                }
            }
        });
        Self {
            state,
            server,
            transport: Transport::LostResponses {
                address,
                worker: Some(worker),
            },
        }
    }
    pub fn connect(&self) -> GitLab {
        self.connect_with_limits(Limits::default()).unwrap()
    }
    pub fn connect_with_limits(
        &self,
        limits: Limits,
    ) -> Result<GitLab, review_kit_gitlab_adapter::GitLabError> {
        let config = GitLabConfig::new(&self.server, Credential::private_token(TOKEN).unwrap())
            .unwrap()
            .with_limits(limits)
            .unwrap();
        GitLab::connect(
            config,
            &format!("{}/group/sub/repo/-/merge_requests/7", self.server),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let (address, worker) = match &mut self.transport {
            Transport::Mock { .. } => return,
            Transport::LostResponses { address, worker } => (address, worker),
        };
        if let Ok(mut stream) = TcpStream::connect_timeout(address, Duration::from_secs(1)) {
            let _ = stream
                .write_all(b"GET /__stop HTTP/1.1\r\nHost: fixture\r\nContent-Length: 0\r\n\r\n");
        }
        let outcome = worker.take().unwrap().join();
        if !thread::panicking() {
            assert!(outcome.is_ok(), "HTTP fixture worker failed");
        }
    }
}

impl From<&HttpMockRequest> for Request {
    fn from(request: &HttpMockRequest) -> Self {
        let uri = request.uri();
        let path = percent_decode_str(uri.path().strip_prefix("/api/v4").unwrap_or(uri.path()))
            .decode_utf8()
            .unwrap()
            .into_owned();
        Self {
            method: request.method_str().into(),
            target: uri
                .path_and_query()
                .expect("HTTP request target")
                .as_str()
                .into(),
            path,
            query: request.query_params().into_iter().collect(),
            headers: request
                .headers_vec()
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
                .collect(),
            payload: if request.body_ref().is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(request.body_ref()).unwrap()
            },
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let (offset, length) = loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "request closed before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(offset) = bytes.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..offset]).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (offset + 4, length);
        }
        assert!(bytes.len() < 1024 * 1024);
    };
    while bytes.len() < offset + length {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
    }
    let headers = std::str::from_utf8(&bytes[..offset - 4]).unwrap();
    let mut lines = headers.lines();
    let mut start = lines.next().unwrap().split_whitespace();
    let method = start.next().unwrap().to_owned();
    let target = start.next().unwrap().to_owned();
    let url = Url::parse(&format!("http://fixture{target}")).unwrap();
    let path = percent_decode_str(url.path().strip_prefix("/api/v4").unwrap_or(url.path()))
        .decode_utf8()
        .unwrap()
        .into_owned();
    Request {
        method,
        target,
        path,
        query: url.query_pairs().into_owned().collect(),
        headers: lines
            .map(|line| {
                let (key, value) = line.split_once(':').unwrap();
                (key.to_ascii_lowercase(), value.trim().into())
            })
            .collect(),
        payload: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[offset..offset + length]).unwrap()
        },
    }
}
