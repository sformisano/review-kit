use std::{
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::{Method, blocking::Client};
use review_kit_application::ports::ReviewReader;
use review_kit_domain::Revision;
use review_kit_gitlab_adapter::{Credential, GitLab, GitLabConfig, Limits};
use serde_json::{Value, json};

use super::service::GitLabService;

pub const BASE: &str = "first\nsame\nlast\n";
pub const HEAD: &str = "first\nsame\nadded\nlast\n";
pub const FILES: [&str; 4] = ["src/lib.rs", "single.rs", "multi.rs", "delete.rs"];

// This client creates fixtures and independently inspects server state.
// It does not reuse adapter DTOs or publication readbacks.
#[derive(Clone)]
pub struct Api {
    pub url: String,
    token: String,
    client: Client,
}

pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Api {
    fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
        }
    }
    pub fn call(&self, method: Method, path: &str, body: Value) -> Value {
        let response = self.raw(method.clone(), path, body);
        assert!(
            (200..300).contains(&response.status),
            "fixture API {method} {path}: {}",
            response.status
        );
        if response.status == 204 {
            Value::Null
        } else {
            serde_json::from_slice(&response.body).unwrap()
        }
    }
    pub fn raw(&self, method: Method, path: &str, body: Value) -> RawResponse {
        let response = self
            .client
            .request(method.clone(), format!("{}/api/v4/{path}", self.url))
            .header("PRIVATE-TOKEN", &self.token)
            .json(&body)
            .send()
            .unwrap();
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "set-cookie" | "authorization" | "private-token"
                )
            })
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        RawResponse {
            status,
            headers,
            body: response.bytes().unwrap().to_vec(),
        }
    }
    pub fn get(&self, path: &str) -> Value {
        self.call(Method::GET, path, Value::Null)
    }
    pub fn post(&self, path: &str, body: Value) -> Value {
        self.call(Method::POST, path, body)
    }
    pub fn delete(&self, path: &str) {
        self.call(Method::DELETE, path, Value::Null);
    }
    fn wait_for_branch(&self, project: u64) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let response = self
                .client
                .get(format!(
                    "{}/api/v4/projects/{project}/repository/branches/main",
                    self.url
                ))
                .header("PRIVATE-TOKEN", &self.token)
                .send()
                .unwrap();
            if response.status().is_success() {
                let branch: Value = response.json().unwrap();
                assert_eq!(branch["name"], "main");
                return;
            }
            assert_eq!(
                response.status(),
                reqwest::StatusCode::NOT_FOUND,
                "fixture author cannot read the initialized branch"
            );
            assert!(
                Instant::now() < deadline,
                "fixture repository did not become readable"
            );
            thread::sleep(Duration::from_millis(250));
        }
    }
}

pub struct User {
    pub id: u64,
    pub api: Api,
}

pub struct Fixture {
    pub admin: Api,
    pub author: User,
    pub reviewer: User,
    pub restricted: Api,
    pub project: u64,
    pub namespace: u64,
    pub mr: u64,
    pub mr_url: String,
    name: String,
    group: u64,
    groups: Vec<u64>,
    projects: Vec<u64>,
    users: Vec<u64>,
}
impl Fixture {
    pub fn new(service: &GitLabService) -> Self {
        let url = service.endpoint().to_owned();
        let admin = Api::new(url.clone(), service.admin_token().into());
        let version = admin.get("version");
        println!("Testing GitLab {}", version["version"].as_str().unwrap());
        let name = format!(
            "rk-live-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let group = admin.post(
            "groups",
            json!({"name":name,"path":name,"visibility":"private"}),
        );
        let group_id = group["id"].as_u64().unwrap();
        println!("Fixture namespace: {name}/reviews");
        let subgroup = admin.post(
            "groups",
            json!({"name":"reviews","path":"reviews","parent_id":group_id}),
        );
        let namespace = subgroup["id"].as_u64().unwrap();
        let author = create_user(&admin, &format!("{name}-author"));
        let reviewer = create_user(&admin, &format!("{name}-reviewer"));
        for (user, access) in [(&author, 40), (&reviewer, 30)] {
            admin.post(
                &format!("groups/{group_id}/members"),
                json!({"user_id":user.id,"access_level":access}),
            );
        }
        let read_token = create_token(&admin, reviewer.id, "read_api");
        let restricted = Api::new(url, read_token);
        let project = admin.post(
            "projects",
            json!({"name":"repository","path":"repository",
            "namespace_id":namespace,"initialize_with_readme":true,"default_branch":"main"}),
        );
        let project_id = project["id"].as_u64().unwrap();
        author.api.wait_for_branch(project_id);
        commit(
            &author.api,
            project_id,
            "main",
            None,
            "Base source",
            &FILES.map(|path| (path, BASE)),
            "create",
        );
        let head = commit(
            &author.api,
            project_id,
            "review",
            Some("main"),
            "Review source",
            &FILES.map(|path| (path, HEAD)),
            "update",
        );
        let mr = author.api.post(
            &format!("projects/{project_id}/merge_requests"),
            json!({
            "source_branch":"review","target_branch":"main","title":"Live adapter verification"}),
        );
        let iid = mr["iid"].as_u64().unwrap();
        wait_for_diff(&author.api, project_id, iid, &head);
        Self {
            admin,
            author,
            reviewer,
            restricted,
            project: project_id,
            namespace,
            mr: iid,
            mr_url: mr["web_url"].as_str().unwrap().into(),
            name,
            group: group_id,
            groups: vec![group_id],
            projects: vec![],
            users: vec![],
        }
    }
    /// Another group member whose user this fixture deletes on cleanup.
    ///
    /// Returns once the member can read the project.
    pub fn add_user(&mut self, role: &str, access_level: u64) -> User {
        let user = create_user(&self.admin, &format!("{}-{role}", self.name));
        self.admin.post(
            &format!("groups/{}/members", self.group),
            json!({"user_id":user.id,"access_level":access_level}),
        );
        self.users.push(user.id);
        self.wait_for_access(&user.api);
        user
    }
    /// Waits until `api` can read the project; GitLab can grant project
    /// access to a new member after the membership request returns.
    fn wait_for_access(&self, api: &Api) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = api
                .raw(
                    Method::GET,
                    &format!("projects/{}", self.project),
                    Value::Null,
                )
                .status;
            if status == 200 {
                return;
            }
            assert_eq!(status, 404, "the new member cannot read the project");
            assert!(
                Instant::now() < deadline,
                "project access did not reach the new member"
            );
            thread::sleep(Duration::from_millis(250));
        }
    }
    /// A project access token's bot user, or `None` when the server refuses
    /// to create one. Project deletion removes the bot with its token.
    pub fn add_project_bot(&self, expires_at: &str) -> Option<User> {
        let response = self
            .admin
            .client
            .post(format!(
                "{}/api/v4/projects/{}/access_tokens",
                self.admin.url, self.project
            ))
            .header("PRIVATE-TOKEN", &self.admin.token)
            .json(&json!({"name":"conversation-bot","scopes":["api"],
                "access_level":30,"expires_at":expires_at}))
            .send()
            .unwrap();
        if !response.status().is_success() {
            println!("Project access token unavailable: {}", response.status());
            return None;
        }
        let token: Value = response.json().unwrap();
        let bot = User {
            id: token["user_id"].as_u64().unwrap(),
            api: Api::new(
                self.admin.url.clone(),
                token["token"].as_str().unwrap().into(),
            ),
        };
        self.wait_for_access(&bot.api);
        Some(bot)
    }
    pub fn connect(&self) -> GitLab {
        self.connect_as(&self.reviewer.api)
    }
    pub fn connect_as(&self, api: &Api) -> GitLab {
        GitLab::connect(
            GitLabConfig::new(&api.url, Credential::private_token(&api.token).unwrap()).unwrap(),
            &self.mr_url,
        )
        .unwrap()
    }
    pub fn connect_with_limits(&self, limits: Limits) -> GitLab {
        let api = &self.reviewer.api;
        let config = GitLabConfig::new(&api.url, Credential::private_token(&api.token).unwrap())
            .unwrap()
            .with_limits(limits)
            .unwrap();
        GitLab::connect(config, &self.mr_url).unwrap()
    }
    pub fn retained_resources(&self) -> Value {
        json!({"mr_url":self.mr_url,"project":self.project,"mr":self.mr,
            "groups":self.groups,"extra_projects":self.projects,
            "author":self.author.id,"reviewer":self.reviewer.id,"extra_users":self.users})
    }
    pub fn browser_author(&self, password: &str) -> String {
        let path = format!("users/{}", self.author.id);
        self.admin
            .call(Method::PUT, &path, json!({"password":password}))["username"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub fn revision(&self) -> Revision {
        let client = self.connect();
        client.clone().discover(client.subject()).unwrap().revision
    }
    pub fn mr_path(&self) -> String {
        format!("projects/{}/merge_requests/{}", self.project, self.mr)
    }
    pub fn notes(&self) -> Vec<Value> {
        pages(&self.admin, &format!("{}/notes", self.mr_path()))
    }
    pub fn discussions(&self) -> Vec<Value> {
        pages(&self.admin, &format!("{}/discussions", self.mr_path()))
    }
    pub fn push_prefix(&self) {
        let new = format!("prefix\n{HEAD}");
        let head = commit(
            &self.author.api,
            self.project,
            "review",
            None,
            "Follow-up source",
            &FILES.map(|path| (path, new.as_str())),
            "update",
        );
        wait_for_diff(&self.author.api, self.project, self.mr, &head);
    }
    pub fn fork(&mut self) {
        let fork = self.admin.post(
            &format!("projects/{}/fork", self.project),
            json!({"namespace_id":self.namespace,"name":"fork","path":"fork"}),
        );
        let source = fork["id"].as_u64().unwrap();
        self.projects.push(source);
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let state = self.admin.get(&format!("projects/{source}"));
            if state["import_status"] == "finished" {
                break;
            }
            assert!(Instant::now() < deadline, "fork import did not finish");
            thread::sleep(Duration::from_millis(250));
        }
        self.author.api.wait_for_branch(source);
        let content = "first\nsame\nfork-only\nlast\n";
        let head = commit(
            &self.author.api,
            source,
            "fork-review",
            Some("main"),
            "Fork source",
            &[("src/lib.rs", content)],
            "update",
        );
        let mr = self.author.api.post(
            &format!("projects/{source}/merge_requests"),
            json!({
            "source_branch":"fork-review","target_branch":"main","target_project_id":self.project,
            "title":"Fork adapter verification"}),
        );
        self.mr = mr["iid"].as_u64().unwrap();
        self.mr_url = mr["web_url"].as_str().unwrap().into();
        wait_for_diff(&self.author.api, self.project, self.mr, &head);
    }
    // Failed scenarios preserve their fixture for inspection. Successful ones
    // request deletion only for resources this fixture created.
    pub fn cleanup(self) {
        for project in self.projects {
            self.admin.delete(&format!("projects/{project}"));
        }
        for group in self.groups {
            self.admin.delete(&format!("groups/{group}"));
        }
        for user in [self.author.id, self.reviewer.id]
            .into_iter()
            .chain(self.users)
        {
            self.admin.delete(&format!("users/{user}?hard_delete=true"));
        }
    }
}

fn create_user(admin: &Api, name: &str) -> User {
    let password = format!(
        "Aa!9{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let user = admin.post(
        "users",
        json!({"username":name,"name":name,
        "email":format!("{name}@example.test"),"password":password,
        "skip_confirmation":true}),
    );
    let id = user["id"].as_u64().unwrap();
    User {
        id,
        api: Api::new(admin.url.clone(), create_token(admin, id, "api")),
    }
}
fn create_token(admin: &Api, user: u64, scope: &str) -> String {
    admin.post(
        &format!("users/{user}/personal_access_tokens"),
        json!({"name":"live-test","scopes":[scope]}),
    )["token"]
        .as_str()
        .unwrap()
        .into()
}
pub fn commit(
    api: &Api,
    project: u64,
    branch: &str,
    start: Option<&str>,
    message: &str,
    files: &[(&str, &str)],
    action: &str,
) -> String {
    let mut body = json!({"branch":branch,"commit_message":message,
        "actions":files.iter().map(|(path,content)| json!({"action":action,"file_path":path,"content":content})).collect::<Vec<_>>()});
    if let Some(start) = start {
        body["start_branch"] = start.into();
    }
    api.post(&format!("projects/{project}/repository/commits"), body)["id"]
        .as_str()
        .unwrap()
        .into()
}
pub fn wait_for_diff(api: &Api, project: u64, mr: u64, head: &str) {
    let path = format!("projects/{project}/merge_requests/{mr}");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let request = api.get(&path);
        if request["diff_refs"]["head_sha"] == head {
            let versions = api.get(&format!("{path}/versions"));
            for version in versions.as_array().unwrap() {
                if version["head_commit_sha"] == head {
                    let exact = api.get(&format!("{path}/versions/{}", version["id"]));
                    if exact["state"] == "collected" {
                        return;
                    }
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "GitLab did not collect the expected MR diff"
        );
        thread::sleep(Duration::from_millis(250));
    }
}
fn pages(api: &Api, path: &str) -> Vec<Value> {
    let mut result = Vec::new();
    for page in 1..=100 {
        let items = api.get(&format!("{path}?per_page=100&page={page}"));
        let items = items.as_array().unwrap();
        result.extend(items.iter().cloned());
        if items.len() < 100 {
            return result;
        }
    }
    panic!("fixture observation exceeded its page bound");
}
