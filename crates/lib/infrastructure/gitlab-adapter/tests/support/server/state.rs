use super::super::{MR_PATH, TOKEN};
use super::{Fault, Request, Response};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

pub struct State {
    pub server: String,
    pub authenticated_actor: u64,
    pub notes: Vec<Value>,
    pub discussions: Vec<Value>,
    pub requests: Vec<Request>,
    pub faults: Vec<Fault>,
    pub page_size: usize,
    pub broken_pagination: bool,
    pub head: String,
    pub historical_head: Option<String>,
    pub discussion_locked: Option<bool>,
    /// `None` omits the field; `Some(Value::Null)` sends `null`.
    pub title: Option<Value>,
    /// `None` omits the field; `Some(Value::Null)` sends `null`.
    pub description: Option<Value>,
    pub source_project: Option<u64>,
    pub version_state: String,
    pub diffs: Vec<Value>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub revision_diffs: BTreeMap<String, Vec<Value>>,
    pub revision_files: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    pub modes: BTreeMap<String, String>,
    next_note: u64,
}

impl State {
    pub(super) fn new(server: String) -> Self {
        let diffs = vec![
            json!({
                "old_path":"src/old.rs", "new_path":"src/lib.rs", "a_mode":"100644", "b_mode":"100644",
                "new_file":false,"renamed_file":true,"deleted_file":false,"collapsed":false,"too_large":false,
                "diff":"@@ -1,2 +1,3 @@\n same\n+added\n last\n"
            }),
            json!({
                "old_path":"src/new.rs", "new_path":"src/new.rs", "a_mode":"0", "b_mode":"100644",
                "new_file":true,"renamed_file":false,"deleted_file":false,"collapsed":false,"too_large":false,
                "diff":"@@ -0,0 +1,2 @@\n+first\n+second\n"
            }),
        ];
        Self {
            server,
            authenticated_actor: 42,
            notes: vec![],
            discussions: vec![],
            requests: vec![],
            faults: vec![],
            page_size: 100,
            broken_pagination: false,
            head: "1".repeat(40),
            historical_head: None,
            discussion_locked: Some(false),
            title: Some("Fixture merge request".into()),
            description: Some("Fixture description".into()),
            source_project: Some(22),
            version_state: "collected".into(),
            diffs,
            files: BTreeMap::from([
                ("src/lib.rs".into(), b"same\nadded\nlast\n".to_vec()),
                ("src/new.rs".into(), b"first\nsecond\n".to_vec()),
            ]),
            revision_diffs: BTreeMap::new(),
            revision_files: BTreeMap::new(),
            modes: BTreeMap::new(),
            next_note: 100,
        }
    }

    pub fn writes(&self) -> Vec<&Request> {
        self.requests
            .iter()
            .filter(|request| request.method != "GET")
            .collect()
    }
    pub fn commit(&mut self, request: &Request) {
        self.route(request);
    }
    pub fn retain_revision(&mut self) {
        self.revision_diffs
            .insert(self.head.clone(), self.diffs.clone());
        self.revision_files
            .insert(self.head.clone(), self.files.clone());
    }
    fn diffs_at(&self, head: &str) -> &[Value] {
        self.revision_diffs
            .get(head)
            .map(Vec::as_slice)
            .unwrap_or(&self.diffs)
    }
    fn files_at(&self, head: &str) -> &BTreeMap<String, Vec<u8>> {
        self.revision_files.get(head).unwrap_or(&self.files)
    }
    /// A merge-request note; its creation time advances one second per note.
    pub fn note(&mut self, body: &str, author: u64) -> Value {
        self.next_note += 1;
        let seconds = self.next_note;
        let created_at = format!(
            "2026-01-01T{:02}:{:02}:{:02}.000Z",
            seconds / 3600 % 24,
            seconds / 60 % 60,
            seconds % 60
        );
        json!({"id":self.next_note,"body":body,"author":{"id":author},"system":false,
            "noteable_id":55,"noteable_type":"MergeRequest","project_id":11,"noteable_iid":7,
            "type":null,"position":null,"resolvable":false,"resolved":false,"suggestions":[],
            "created_at":created_at})
    }
    pub fn question(&mut self, discussion: &str, body: &str) -> u64 {
        let mut note = self.note(body, 77);
        note["type"] = "DiscussionNote".into();
        let id = note["id"].as_u64().unwrap();
        let thread = self
            .discussions
            .iter_mut()
            .find(|value| value["id"] == discussion)
            .unwrap();
        thread["notes"].as_array_mut().unwrap().push(note);
        id
    }

    /// Discussions as GitLab lists them: every thread, and every merge-request
    /// note, such as a review summary, as its own individual-note discussion.
    /// Notes join the threads in creation order; threads keep their stored
    /// order, so tests can still reorder them.
    pub fn listed_discussions(&self) -> Vec<Value> {
        let mut notes = self
            .notes
            .iter()
            .map(|note| {
                json!({"id": format!("note-{}", note["id"]), "individual_note": true, "notes": [note]})
            })
            .peekable();
        let mut listed = Vec::new();
        for thread in &self.discussions {
            let created = thread["notes"][0]["id"].as_u64();
            while let Some(note) = notes.next_if(|note| note["notes"][0]["id"].as_u64() < created) {
                listed.push(note);
            }
            listed.push(thread.clone());
        }
        listed.extend(notes);
        listed
    }

    fn version(&self) -> Value {
        self.version_for(
            if self.historical_head.is_some() {
                13
            } else {
                12
            },
            &self.head,
        )
    }
    fn version_for(&self, id: u64, head: &str) -> Value {
        json!({"id":id,"merge_request_id":55,"head_commit_sha":head,
            "base_commit_sha":"2".repeat(40),"start_commit_sha":"3".repeat(40),
            "state":self.version_state,"real_size":self.diffs_at(head).len().to_string()})
    }
    fn merge_request(&self) -> Value {
        let mut request = json!({"id":55,"iid":7,"project_id":11,"target_project_id":11,"source_project_id":self.source_project,
            "web_url":format!("{}/group/sub/repo/-/merge_requests/7",self.server),
            "diff_refs":{"head_sha":self.head,"base_sha":"2".repeat(40),"start_sha":"3".repeat(40)},
            "discussion_locked":self.discussion_locked});
        if let Some(title) = &self.title {
            request["title"] = title.clone();
        }
        if let Some(description) = &self.description {
            request["description"] = description.clone();
        }
        request
    }
    fn page(&self, request: &Request, values: Vec<Value>) -> Response {
        assert_eq!(
            request.query.get("per_page").map(String::as_str),
            Some("100")
        );
        let page: usize = request.query["page"].parse().unwrap();
        let start = (page - 1) * self.page_size;
        let output = values
            .into_iter()
            .skip(start)
            .take(self.page_size)
            .collect::<Vec<_>>();
        let next = if self.broken_pagination {
            (page + 2).to_string()
        } else if output.len() == self.page_size {
            (page + 1).to_string()
        } else {
            String::new()
        };
        let mut response = Response::json(Value::Array(output));
        response.headers.push(("x-next-page".into(), next));
        response
    }

    pub(super) fn serve(&mut self, request: Request) -> Option<Response> {
        self.requests.push(request.clone());
        assert_eq!(
            request.headers.get("private-token").map(String::as_str),
            Some(TOKEN)
        );
        if let Some(index) = self.faults.iter().position(|fault| fault.matches(&request)) {
            if let Fault::Respond { skip_matches, .. } = &mut self.faults[index]
                && *skip_matches > 0
            {
                *skip_matches -= 1;
                return Some(self.route(&request));
            }
            match self.faults.remove(index) {
                Fault::Respond { response, .. } => return Some(response),
                Fault::LoseWrite { commit, .. } => {
                    if commit {
                        self.route(&request);
                    }
                    return None;
                }
                Fault::CommitThenDrift { .. } => {
                    let response = self.route(&request);
                    self.head = "4".repeat(40);
                    return Some(response);
                }
                Fault::LateCreation { original, .. } => {
                    let response = self.route(&request);
                    self.commit(&original);
                    return Some(response);
                }
                Fault::HoldRead { ready, release, .. } => {
                    ready.send(()).unwrap();
                    release.recv_timeout(Duration::from_secs(20)).unwrap();
                    return Some(Response::json(Value::Null));
                }
                Fault::PauseRead { ready, release, .. } => {
                    ready.send(()).unwrap();
                    release.recv_timeout(Duration::from_secs(20)).unwrap();
                }
            }
        }
        Some(self.route(&request))
    }

    #[allow(
        clippy::panic,
        reason = "Missing fixture routes are test setup failures."
    )]
    fn route(&mut self, request: &Request) -> Response {
        let path = request.path.as_str();
        if request.method == "GET" {
            if path == "/user" {
                return Response::json(json!({"id":self.authenticated_actor}));
            }
            if path == "/projects/group/sub/repo" {
                return Response::json(
                    json!({"id":11,"path_with_namespace":"group/sub/repo","web_url":format!("{}/group/sub/repo",self.server)}),
                );
            }
            if path == MR_PATH {
                return Response::json(self.merge_request());
            }
            if path == format!("{MR_PATH}/notes") {
                return self.page(request, self.notes.clone());
            }
            if let Some(id) = path.strip_prefix(&format!("{MR_PATH}/notes/")) {
                let note = self
                    .notes
                    .iter()
                    .find(|value| value["id"].as_u64().unwrap().to_string() == id)
                    .unwrap();
                return Response::json(note.clone());
            }
            if path == format!("{MR_PATH}/discussions") {
                return self.page(request, self.listed_discussions());
            }
            if let Some(id) = path.strip_prefix(&format!("{MR_PATH}/discussions/")) {
                let discussion = self
                    .listed_discussions()
                    .into_iter()
                    .find(|value| value["id"] == id)
                    .unwrap();
                return Response::json(discussion);
            }
            if path == format!("{MR_PATH}/versions") {
                let mut versions = vec![self.version()];
                if let Some(head) = &self.historical_head {
                    versions.push(self.version_for(12, head));
                }
                return self.page(request, versions);
            }
            if path == format!("{MR_PATH}/versions/12") {
                let mut version =
                    self.version_for(12, self.historical_head.as_ref().unwrap_or(&self.head));
                version["diffs"] = self
                    .diffs_at(self.historical_head.as_ref().unwrap_or(&self.head))
                    .to_vec()
                    .into();
                return Response::json(version);
            }
            if path == format!("{MR_PATH}/versions/13") {
                let mut version = self.version_for(13, &self.head);
                version["diffs"] = self.diffs_at(&self.head).to_vec().into();
                return Response::json(version);
            }
            if path == "/projects/22/repository/tree" {
                assert!(
                    request.query["ref"] == self.head
                        || self.historical_head.as_ref() == Some(&request.query["ref"])
                );
                assert_eq!(request.query["recursive"], "false");
                let parent = &request.query["path"];
                let entries = self.files_at(&request.query["ref"]).keys().filter(|path| path.rsplit_once('/').map_or("", |(parent, _)| parent) == parent).map(|path| json!({"path":path,"type":"blob","mode":self.modes.get(path).map(String::as_str).unwrap_or("100644")})).collect();
                return self.page(request, entries);
            }
            if let Some(file) = path
                .strip_prefix("/projects/22/repository/files/")
                .and_then(|tail| tail.strip_suffix("/raw"))
            {
                assert!(
                    request.query["ref"] == self.head
                        || self.historical_head.as_ref() == Some(&request.query["ref"])
                );
                assert_eq!(request.query["lfs"], "false");
                return Response {
                    status: 200,
                    headers: vec![],
                    body: self.files_at(&request.query["ref"])[file].clone(),
                };
            }
        }
        if request.method == "POST" && path == format!("{MR_PATH}/notes") {
            let note = self.note(
                &request.payload["body"].as_str().unwrap().replace('\r', ""),
                self.authenticated_actor,
            );
            self.notes.push(note.clone());
            return Response::json(note);
        }
        if request.method == "PUT" && path.starts_with(&format!("{MR_PATH}/notes/")) {
            let id: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
            let note = self
                .notes
                .iter_mut()
                .find(|value| value["id"] == id)
                .unwrap();
            note["body"] = request.payload["body"]
                .as_str()
                .unwrap()
                .replace('\r', "")
                .into();
            return Response::json(note.clone());
        }
        if request.method == "POST" && path == format!("{MR_PATH}/discussions") {
            let mut note = self.note(
                &request.payload["body"].as_str().unwrap().replace('\r', ""),
                self.authenticated_actor,
            );
            note["type"] = "DiffNote".into();
            note["position"] = request.payload["position"].clone();
            note["commit_id"] = request.payload["position"]["head_sha"].clone();
            note["resolvable"] = true.into();
            let position = &request.payload["position"];
            let rendered = note["body"].as_str().unwrap();
            if let Some((above, below, replacement)) = suggestion_replacement(rendered) {
                let anchor = position["new_line"].as_u64().unwrap();
                let start = anchor - above;
                let end = anchor + below;
                let source = std::str::from_utf8(
                    &self.files_at(position["head_sha"].as_str().unwrap())
                        [position["new_path"].as_str().unwrap()],
                )
                .unwrap()
                .split_inclusive('\n')
                .collect::<Vec<_>>();
                let original = source[(start - 1) as usize..end as usize].concat();
                let ending = if original.ends_with("\r\n") {
                    "\r\n"
                } else if original.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                let replacement = if replacement.is_empty() {
                    String::new()
                } else {
                    format!("{replacement}{ending}")
                };
                note["suggestions"] = json!([{"from_line":start,"to_line":end,"from_content":original,"to_content":replacement,"applied":false}]);
            }
            let discussion = json!({"id":format!("discussion-{}",self.discussions.len()+1),"individual_note":false,"notes":[note]});
            self.discussions.push(discussion.clone());
            return Response::json(discussion);
        }
        if request.method == "POST" && path.ends_with("/notes") {
            let id = path
                .strip_prefix(&format!("{MR_PATH}/discussions/"))
                .unwrap()
                .strip_suffix("/notes")
                .unwrap();
            let mut note = self.note(
                &request.payload["body"].as_str().unwrap().replace('\r', ""),
                self.authenticated_actor,
            );
            note["type"] = "DiscussionNote".into();
            let discussion = self
                .discussions
                .iter_mut()
                .find(|value| value["id"] == id)
                .unwrap();
            discussion["notes"]
                .as_array_mut()
                .unwrap()
                .push(note.clone());
            return Response::json(note);
        }
        if request.method == "PUT" && path.starts_with(&format!("{MR_PATH}/discussions/")) {
            let id = path.rsplit('/').next().unwrap();
            let discussion = self
                .discussions
                .iter_mut()
                .find(|value| value["id"] == id)
                .unwrap();
            for note in discussion["notes"].as_array_mut().unwrap() {
                if note["resolvable"] == true {
                    note["resolved"] = request.payload["resolved"].clone();
                }
            }
            return Response::json(discussion.clone());
        }
        panic!(
            "unhandled fixture request {} {}",
            request.method, request.target
        );
    }
}

fn suggestion_replacement(body: &str) -> Option<(u64, u64, String)> {
    let mut fence: Option<String> = None;
    let mut offsets = (0, 0);
    let mut replacement = String::new();
    for line in body.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        if let Some(opening) = &fence {
            if text == opening {
                return Some((
                    offsets.0,
                    offsets.1,
                    replacement
                        .strip_suffix('\n')
                        .unwrap_or(&replacement)
                        .to_owned(),
                ));
            }
            replacement.push_str(line);
        } else {
            let count = text.bytes().take_while(|byte| *byte == b'`').count();
            if count >= 3 && text[count..].starts_with("suggestion") {
                let info = &text[count..];
                offsets = if info == "suggestion" {
                    (0, 0)
                } else {
                    let (above, below) = info.strip_prefix("suggestion:-")?.split_once('+')?;
                    (above.parse().ok()?, below.parse().ok()?)
                };
                fence = Some("`".repeat(count));
            }
        }
    }
    None
}
