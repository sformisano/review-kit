use std::{
    env,
    error::Error,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::Method;
use review_kit_application::{
    ports::{PublicationHost, ReviewReader, WriteOutcome},
    publication::{
        Effects, PolicyName, PublicPolicyDeclaration, Round, RoundStatus, SummaryRecord, Write,
    },
    use_cases::checkpoints::ApplicabilityStatus,
};
use review_kit_domain::{
    Assessment, AssessmentDraft, PolicyEvaluation, PolicyId, PolicyJudgment, ReviewDecision,
    ReviewerId, Revision, Subject,
};
use review_kit_gitlab_adapter::{GitLab, GitLabError, Limits};
use review_kit_json_adapter::rounds::{RoundData, State};
use serde_json::{Value, json};

use super::{fixture::Fixture, service::GitLabService};

const STATE_PREFIX: &str = "\n\n<!-- review-kit:state:v3 ";
const STATE_SUFFIX: &str = " -->\n\n<!-- review-kit:v1 {\"kind\":\"summary\"} -->";
const REQUEST_LIMIT: usize = 4 * 1024 * 1024;

struct Evidence {
    root: PathBuf,
    browser: PathBuf,
    rows: Vec<Value>,
}

impl Evidence {
    fn new() -> Self {
        let root = PathBuf::from(env::var_os("REVIEW_KIT_STATE_EVIDENCE_DIR").unwrap());
        let browser = PathBuf::from(env::var_os("REVIEW_KIT_STATE_BROWSER_DIR").unwrap());
        fs::create_dir(&root).unwrap();
        fs::create_dir(&browser).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in [&root, &browser] {
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        Self {
            root,
            browser,
            rows: vec![],
        }
    }

    fn save(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        private_file(&path, bytes);
    }

    fn json(&self, relative: &str, value: &Value) {
        self.save(relative, &serde_json::to_vec_pretty(value).unwrap());
    }

    fn retain(&self, fixture: &Fixture, case: &str) {
        self.json(
            &format!("{case}/resources.json"),
            &fixture.retained_resources(),
        );
    }

    fn browser(&self, fixture: &Fixture, case: &str, note: &SummaryRecord, canary: &str) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let password = format!("Aa!9{nonce:x}");
        let username = fixture.browser_author(&password);
        let password_file = format!("{case}.password");
        let rotated_password_file = format!("{case}.rotated-password");
        private_file(&self.browser.join(&password_file), password.as_bytes());
        private_file(
            &self.browser.join(&rotated_password_file),
            format!("Bb!8{nonce:x}R").as_bytes(),
        );
        let input = json!({"username":username,"password_file":password_file,
            "rotated_password_file":rotated_password_file,
            "rows":[{"case":case,"mr_url":fixture.mr_url,"note_id":note.id.as_str(),
                "visible_label":note.body,"hidden_canary":canary}]});
        private_file(
            &self.browser.join(format!("{case}.json")),
            &serde_json::to_vec_pretty(&input).unwrap(),
        );
    }
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).unwrap().write_all(bytes).unwrap();
}

fn round(subject: &Subject, reviewer: &ReviewerId, revision: &Revision, scope: String) -> Round {
    let policy = PolicyId::try_from("correctness".to_owned()).unwrap();
    Round {
        confirmed_validation: false,
        number: 1,
        assessment: Assessment::new(AssessmentDraft {
            subject: subject.clone(),
            reviewer: reviewer.clone(),
            revision: revision.clone(),
            decision: ReviewDecision::Ready,
            depth: "Complete fixture review".into(),
            scope,
            limitation: None,
            selected_policies: vec![policy.clone()],
            evaluations: vec![
                PolicyEvaluation::new(policy.clone(), PolicyJudgment::Satisfied).unwrap(),
            ],
            findings: vec![],
        })
        .unwrap(),
        status: RoundStatus::Pending,
        abandonment: None,
        publication_limitation: None,
        answers: vec![],
        transitions: vec![],
        answer_targets: vec![],
        actions: vec![],
        review_workflow: Some("state-preservation".into()),
        policy_set: Some("live-policies".into()),
        policy_names: vec![PolicyName {
            id: policy.clone(),
            name: "Correctness".into(),
        }],
        policy_declarations: vec![
            PublicPolicyDeclaration {
                policy: PolicyName {
                    id: policy,
                    name: "Correctness".into(),
                },
                status: ApplicabilityStatus::Applicable,
                reason: "The changed source requires correctness review.".into(),
            },
            PublicPolicyDeclaration {
                policy: PolicyName {
                    id: PolicyId::try_from("async-work".to_owned()).unwrap(),
                    name: "Async Work Placement".into(),
                },
                status: ApplicabilityStatus::Excluded,
                reason: "This fixture changes synchronous source only.".into(),
            },
        ],
        renderer: None,
        communicated_findings: Some(vec![]),
        verified_resolutions: vec![],
    }
}

fn state_size(round: &Round) -> usize {
    let json = serde_json::to_string(&State::from_rounds(round, &[]).unwrap()).unwrap();
    json.len()
        + json
            .chars()
            .filter(|character| matches!(character, '<' | '>' | '&' | '`' | '~'))
            .count()
            * 5
}

fn sized_round(
    subject: &Subject,
    reviewer: &ReviewerId,
    revision: &Revision,
    target: usize,
    canary: &str,
) -> Round {
    let scope = format!("{canary}: --> <!-- nested & ``` ~ \" \\ \nUnicode λ ☃\n");
    let initial = round(subject, reviewer, revision, scope.clone());
    let padding = target.saturating_sub(state_size(&initial));
    round(subject, reviewer, revision, scope + &"x".repeat(padding))
}

fn retained_payload(body: &str) -> &str {
    let (visible, payload) = body.rsplit_once(STATE_PREFIX).unwrap();
    assert!(!visible.contains("<!-- review-kit:state:"));
    let payload = payload.strip_suffix(STATE_SUFFIX).unwrap();
    assert!(!payload.contains(['\r', '\n', '<', '>', '&', '`', '~']));
    assert_eq!(
        serde_json::from_str::<Value>(payload).unwrap()["version"],
        3
    );
    payload
}

fn write(client: &GitLab, write: &Write) -> WriteOutcome {
    let mut host = client.publication_host(Effects::review());
    let mut session = host.acquire(client.subject(), client.reviewer()).unwrap();
    session.write(client.subject(), client.reviewer(), write)
}

fn summary(client: &GitLab, intended: &Round) -> SummaryRecord {
    let mut reader = client.clone();
    let snapshot = reader.discover(client.subject()).unwrap();
    assert_eq!(snapshot.summaries.len(), 1);
    let summary = snapshot.summaries.into_iter().next().unwrap();
    assert_eq!(summary.current, *intended);
    assert!(summary.history.is_empty());
    summary
}

fn independent_reads(
    evidence: &Evidence,
    fixture: &Fixture,
    prefix: &str,
    note: &SummaryRecord,
) -> Vec<u8> {
    let mut canonical = None;
    for kind in ["notes", "single_note", "discussions"] {
        let path = if kind == "single_note" {
            format!("{}/notes/{}", fixture.mr_path(), note.id.as_str())
        } else {
            format!("{}/{}", fixture.mr_path(), kind)
        };
        let mut page = 1;
        let mut matches = vec![];
        loop {
            assert!(page <= 100, "independent pagination exceeded its bound");
            let endpoint = format!("{path}?per_page=100&page={page}");
            let raw = fixture.author.api.raw(Method::GET, &endpoint, Value::Null);
            assert_eq!(raw.status, 200);
            let file = format!("{prefix}/{kind}-{page}.json");
            evidence.save(&file, &raw.body);
            evidence.json(
                &format!("{prefix}/{kind}-{page}.headers.json"),
                &json!({"status":raw.status,"headers":raw.headers,"path":endpoint}),
            );
            let response: Value = serde_json::from_slice(&raw.body).unwrap();
            let notes = if kind == "single_note" {
                vec![&response]
            } else if kind == "notes" {
                response.as_array().unwrap().iter().collect()
            } else {
                response
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|discussion| discussion["notes"].as_array().unwrap())
                    .collect()
            };
            matches.extend(
                notes
                    .into_iter()
                    .filter(|row| row["id"].as_u64().unwrap().to_string() == note.id.as_str())
                    .map(|row| row["body"].as_str().unwrap().as_bytes().to_vec()),
            );
            let next = raw
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("x-next-page"))
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            if kind == "single_note" || next.is_empty() {
                break;
            }
            let next: usize = next.parse().unwrap();
            assert_eq!(next, page + 1);
            page = next;
        }
        assert_eq!(
            matches.len(),
            1,
            "independent {kind} lost or repeated the note"
        );
        if let Some(expected) = &canonical {
            assert_eq!(&matches[0], expected);
        }
        canonical = Some(matches.remove(0));
    }
    let canonical = canonical.unwrap();
    let body = std::str::from_utf8(&canonical).unwrap();
    let payload = retained_payload(body);
    assert_eq!(
        State::decode(payload.as_bytes())
            .unwrap()
            .into_rounds()
            .unwrap(),
        (note.current.clone(), note.history.clone())
    );
    let first =
        serde_json::to_vec(&State::from_rounds(&note.current, &note.history).unwrap()).unwrap();
    let (restored, history) = State::decode(payload.as_bytes())
        .unwrap()
        .into_rounds()
        .unwrap();
    assert_eq!(
        first,
        serde_json::to_vec(&State::from_rounds(&restored, &history).unwrap()).unwrap()
    );
    canonical
}

fn markdown(
    evidence: &Evidence,
    fixture: &Fixture,
    prefix: &str,
    body: &str,
    visible: &str,
    canary: &str,
) {
    let project = fixture
        .author
        .api
        .get(&format!("projects/{}", fixture.project));
    let response = fixture.author.api.raw(
        Method::POST,
        "markdown",
        json!({"text":body,"project":project["path_with_namespace"],"gfm":true}),
    );
    assert!(matches!(response.status, 200 | 201));
    evidence.save(&format!("{prefix}/markdown.json"), &response.body);
    let json: Value = serde_json::from_slice(&response.body).unwrap();
    let html = json["html"].as_str().unwrap();
    let mut without_comments = String::new();
    let mut rest = html;
    while let Some((before, comment)) = rest.split_once("<!--") {
        without_comments.push_str(before);
        rest = comment.split_once("-->").unwrap().1;
    }
    without_comments.push_str(rest);
    assert!(without_comments.contains(visible));
    for hidden in [
        canary,
        "review-kit:state:",
        "Review Kit data",
        "\"policy_names\":",
        "\"findings\":",
        "\"version\":",
    ] {
        assert!(
            !without_comments.contains(hidden),
            "retained state became Markdown content"
        );
    }
}

fn preserved(evidence: &mut Evidence, service: &GitLabService, target: usize) {
    let case = format!("state-{target}");
    let fixture = Fixture::new(service);
    evidence.retain(&fixture, &case);
    let client = fixture.connect();
    let revision = fixture.revision();
    let canary = format!("HIDDEN_STATE_CANARY_{target}");
    let mut intended = sized_round(
        client.subject(),
        client.reviewer(),
        &revision,
        target,
        &canary,
    );
    let mut previous = None;
    let mut previous_payload = None;
    for method in ["POST", "PUT"] {
        if method == "PUT" {
            intended.status = RoundStatus::Completed;
        }
        let body = format!("State preservation {target} {method}");
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert!(matches!(
            write(
                &client,
                &Write::Summary {
                    expected: previous.clone().map(Box::new),
                    round: Box::new(intended.clone()),
                    history: vec![],
                    body: body.clone(),
                }
            ),
            WriteOutcome::Acknowledged
        ));
        let dispatched = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let summary = summary(&client, &intended);
        assert_eq!(summary.body, body);
        let prefix = format!("{case}/{method}");
        let raw = independent_reads(evidence, &fixture, &prefix, &summary);
        let raw_body = std::str::from_utf8(&raw).unwrap();
        let payload = retained_payload(raw_body);
        assert!(payload.contains(&canary));
        assert!(payload.len().abs_diff(target) <= 1024.max(target / 20));
        if let Some(previous) = &previous_payload {
            assert_ne!(payload, previous);
        }
        previous_payload = Some(payload.to_owned());
        evidence.save(&format!("{prefix}/body.txt"), &raw);
        evidence.json(
            &format!("{prefix}/intended-round.json"),
            &serde_json::to_value(RoundData::from(&intended)).unwrap(),
        );
        markdown(evidence, &fixture, &prefix, raw_body, &body, &canary);
        evidence.rows.push(json!({"case":case,"method":method,"note_id":summary.id.as_str(),"mr_url":fixture.mr_url,
            "target_payload_bytes":target,"payload_bytes":payload.len(),"body_bytes":raw.len(),
            "native_outcome":"Acknowledged","typed_round_equal":true,"byte_stable_reencoding":true,
            "dispatch_started":started,"dispatch_finished":dispatched,"visible_label":body,"hidden_canary":canary}));
        if method == "PUT" {
            evidence.browser(&fixture, &case, &summary, &canary);
        }
        previous = Some(summary);
    }
}

fn rejected(evidence: &mut Evidence, service: &GitLabService, update: bool) {
    let case = if update {
        "rejected-PUT"
    } else {
        "rejected-POST"
    };
    let fixture = Fixture::new(service);
    evidence.retain(&fixture, case);
    let client = fixture.connect_with_limits(Limits {
        body_bytes: REQUEST_LIMIT,
        ..Limits::default()
    });
    let revision = fixture.revision();
    let mut intended = sized_round(
        client.subject(),
        client.reviewer(),
        &revision,
        1024,
        "REJECTION_CONTROL_CANARY",
    );
    intended.status = RoundStatus::Completed;
    let expected = if update {
        assert!(matches!(
            write(
                &client,
                &Write::Summary {
                    expected: None,
                    round: Box::new(intended.clone()),
                    history: vec![],
                    body: "Existing rejection control".into()
                }
            ),
            WriteOutcome::Acknowledged
        ));
        Some(summary(&client, &intended))
    } else {
        None
    };
    let before = fixture.notes();
    evidence.json(&format!("{case}/before-notes.json"), &json!(before));
    let body = format!("Oversized summary\n{}", "x".repeat(1_100_000));
    assert!(body.len() > Limits::default().body_bytes);
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let outcome = write(
        &client,
        &Write::Summary {
            expected: expected.clone().map(Box::new),
            round: Box::new(intended),
            history: vec![],
            body,
        },
    );
    let dispatched = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let WriteOutcome::Rejected(error) = outcome else {
        panic!("oversized note was not definitely rejected");
    };
    let Some(GitLabError::Rejected(status)) = error
        .source()
        .and_then(|error| error.downcast_ref::<GitLabError>())
    else {
        panic!("oversized note did not reach a typed server rejection");
    };
    assert!((400..500).contains(status));
    let after = fixture.notes();
    evidence.json(&format!("{case}/after-notes.json"), &json!(after));
    assert_eq!(before, after);
    if let Some(expected) = &expected {
        independent_reads(evidence, &fixture, &format!("{case}/unchanged"), expected);
    }
    evidence.rows.push(json!({"case":case,"method":if update { "PUT" } else { "POST" },"native_outcome":"Rejected",
        "native_error_kind":"Rejected","native_error_status":status,"request_limit_bytes":REQUEST_LIMIT,
        "default_request_limit_bytes":Limits::default().body_bytes,"typed_rejection":true,"independent_notes_unchanged":true,
        "dispatch_started":started,"dispatch_finished":dispatched,"proof_scope":"direct_adapter"}));
}

pub fn run(service: &GitLabService) {
    let mut evidence = Evidence::new();
    for target in [1024, 102_400, 921_600] {
        preserved(&mut evidence, service, target);
    }
    rejected(&mut evidence, service, false);
    rejected(&mut evidence, service, true);
    evidence.json("native-rows.json", &json!({"execution_mode":"sequential","rows":evidence.rows,"fixtures_retained_for_browser":true}));
}

#[test]
fn large_typed_states_restore_scope_exactly() {
    let subject = Subject::new(
        "gitlab".into(),
        "https://example.test".into(),
        "group/project".into(),
        1,
    )
    .unwrap();
    let reviewer = ReviewerId::try_from("42".to_owned()).unwrap();
    let revision = Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap();
    for target in [1024, 102_400, 921_600] {
        let intended = sized_round(
            &subject,
            &reviewer,
            &revision,
            target,
            "HIDDEN_CONTROL_CANARY",
        );
        let state = State::from_rounds(&intended, &[]).unwrap();
        let bytes = serde_json::to_vec(&state).unwrap();
        let (restored, history) = State::decode(&bytes).unwrap().into_rounds().unwrap();
        assert_eq!(restored, intended);
        assert!(history.is_empty());
        assert!(
            restored
                .assessment
                .scope()
                .contains("--> <!-- nested & ``` ~ \" \\ \nUnicode λ ☃\n")
        );
        assert_eq!(
            bytes,
            serde_json::to_vec(&State::from_rounds(&restored, &history).unwrap()).unwrap()
        );
        assert!(state_size(&restored).abs_diff(target) <= 1024);
    }
}
