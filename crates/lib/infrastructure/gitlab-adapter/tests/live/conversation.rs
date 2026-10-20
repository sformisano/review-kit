use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::Method;
use review_kit_application::{
    conversation::{BotStatus, ConversationChange, Resolution, WriteTargets},
    ports::ReviewReader,
    publication::{Effects, ProviderId},
    use_cases::publishing::{PublicationProgress, PublicationStatus, plan_from_snapshot, publish},
};
use review_kit_domain::FindingStatus;
use review_kit_gitlab_adapter::BotLookup;
use review_kit_json_adapter::{ConversationDocument, encode};
use serde_json::{Value, json};

use super::{
    fixture::Fixture,
    model::{Renderer, finding, request, transition},
    service::GitLabService,
};

/// The civil UTC date `days` after today, as `YYYY-MM-DD`.
fn date_after(days: u64) -> String {
    let today = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86_400;
    // Proleptic Gregorian conversion from days since 1970-01-01.
    let z = (today + days) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Each distinct non-system note author's `bot` flag from an administrator's
/// independent `GET /users/:id`.
fn bot_flags(fixture: &Fixture, discussions: &[Value]) -> BTreeMap<u64, Value> {
    let mut flags = BTreeMap::new();
    for note in discussions
        .iter()
        .flat_map(|discussion| discussion["notes"].as_array().unwrap())
        .filter(|note| note["system"] == false)
    {
        let author = note["author"]["id"].as_u64().unwrap();
        flags
            .entry(author)
            .or_insert_with(|| fixture.admin.get(&format!("users/{author}"))["bot"].clone());
    }
    flags
}

/// The supported conversation derived directly from raw REST discussions and
/// independent account reads, without adapter code.
fn expected_discussions(discussions: &[Value], bots: &BTreeMap<u64, Value>) -> Vec<Value> {
    let mut expected: Vec<Value> = discussions
        .iter()
        .filter_map(|discussion| {
            let all = discussion["notes"].as_array().unwrap();
            let mut notes: Vec<Value> = all
                .iter()
                .filter(|note| note["system"] == false)
                .map(|note| {
                    let author = note["author"]["id"].as_u64().unwrap();
                    json!({
                        "id": note["id"].as_u64().unwrap().to_string(),
                        "author": author.to_string(),
                        "author_username": note["author"]["username"],
                        "author_bot": bots[&author],
                        "body": note["body"],
                        "position": note.get("position").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect();
            if notes.is_empty() {
                return None;
            }
            notes.sort_by_key(|note| note["id"].as_str().unwrap().to_owned());
            let resolvable: Vec<_> = all
                .iter()
                .filter(|note| note["resolvable"] == true)
                .collect();
            let resolved = if resolvable.is_empty() {
                Value::Null
            } else {
                resolvable
                    .iter()
                    .all(|note| note["resolved"] == true)
                    .into()
            };
            Some(json!({
                "id": discussion["id"],
                "resolvable": !resolvable.is_empty(),
                "resolved": resolved,
                "notes": notes,
            }))
        })
        .collect();
    expected.sort_by_key(|discussion| discussion["id"].as_str().unwrap().to_owned());
    expected
}

fn sorted(mut discussions: Vec<Value>) -> Vec<Value> {
    for discussion in &mut discussions {
        discussion["notes"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|note| note["id"].as_str().unwrap().to_owned());
    }
    discussions.sort_by_key(|discussion| discussion["id"].as_str().unwrap().to_owned());
    discussions
}

fn id(value: &Value) -> ProviderId {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    ProviderId::new(text).unwrap()
}

// D09, F14, F17 and F23 against a real server: general, bot, unmanaged inline
// and resolvable thread notes beyond the first discussions page are captured
// as context with verbatim positions; system notes are excluded; edits and
// deletions are reflected; a later edit and resolution change are differences.
pub fn conversation_capture_matches_independent_rest_reads(service: &GitLabService) {
    let mut fixture = Fixture::new(service);
    let commenter = fixture.add_user("commenter", 30);
    let bot = fixture.add_project_bot(&date_after(30));
    // A pushed commit adds a system-only discussion.
    fixture.push_prefix();
    let path = fixture.mr_path();
    let initial = fixture.admin.get(&path);
    println!("Initial description: {}", initial["description"]);
    fixture.author.api.call(
        Method::PUT,
        &path,
        json!({"title":"Conversation capture verification",
            "description":"Capture every supported note.\n\nIncluding *Markdown*."}),
    );
    let notes = format!("{path}/notes");
    let threads = format!("{path}/discussions");
    let filler = |fixture: &Fixture, range: std::ops::Range<usize>| {
        for index in range {
            let api = if index % 2 == 0 {
                &commenter.api
            } else {
                &fixture.author.api
            };
            api.post(&notes, json!({"body":format!("General note {index}")}));
        }
    };
    filler(&fixture, 0..101);

    let refs = fixture.admin.get(&path)["diff_refs"].clone();
    let inline = commenter.api.post(
        &threads,
        json!({"body":"Unmanaged inline note about this added line.","position":{
            "position_type":"text","base_sha":refs["base_sha"],"start_sha":refs["start_sha"],
            "head_sha":refs["head_sha"],"old_path":"src/lib.rs","new_path":"src/lib.rs","new_line":4}}),
    );
    let inline_id = inline["id"].as_str().unwrap().to_owned();
    fixture.reviewer.api.post(
        &format!("{threads}/{inline_id}/notes"),
        json!({"body":"Reply inside the inline thread."}),
    );
    let thread = fixture
        .author
        .api
        .post(&threads, json!({"body":"Resolvable general thread."}));
    let thread_id = thread["id"].as_str().unwrap().to_owned();
    commenter.api.post(
        &format!("{threads}/{thread_id}/notes"),
        json!({"body":"Answered in the description."}),
    );
    fixture.author.api.call(
        Method::PUT,
        &format!("{threads}/{thread_id}"),
        json!({"resolved":true}),
    );
    let bot_note = bot.as_ref().map(|bot| {
        bot.api
            .post(&notes, json!({"body":"Automated pipeline report."}))["id"]
            .as_u64()
            .unwrap()
    });
    let edited = commenter
        .api
        .post(&notes, json!({"body":"Original wording."}))["id"]
        .as_u64()
        .unwrap();
    commenter.api.call(
        Method::PUT,
        &format!("{notes}/{edited}"),
        json!({"body":"Edited wording."}),
    );
    let deleted = commenter
        .api
        .post(&notes, json!({"body":"Note that is deleted."}))["id"]
        .as_u64()
        .unwrap();
    commenter.api.delete(&format!("{notes}/{deleted}"));
    filler(&fixture, 101..202);

    let client = fixture.connect();
    let captured = client
        .capture_conversation(client.subject(), BotLookup::Resolve)
        .unwrap();
    let raw = fixture.discussions();
    let request = fixture.admin.get(&path);

    assert!(
        raw.len() > 200,
        "fixture must span several discussion pages"
    );
    let system_only = raw
        .iter()
        .filter(|discussion| {
            discussion["notes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|note| note["system"] == true)
        })
        .count();
    assert!(
        system_only > 0,
        "fixture must contain system-only discussions"
    );
    let first_page = fixture.admin.get(&format!("{threads}?per_page=100&page=1"));
    let first_page: Vec<_> = first_page
        .as_array()
        .unwrap()
        .iter()
        .map(|discussion| discussion["id"].clone())
        .collect();
    assert_eq!(first_page.len(), 100);
    assert!(!first_page.contains(&inline_id.clone().into()));
    assert!(!first_page.contains(&thread_id.clone().into()));

    assert_eq!(captured.subject(), client.subject());
    assert_eq!(captured.title(), request["title"]);
    assert_eq!(captured.description(), request["description"]);
    let document = ConversationDocument::new(&captured, "2026-10-13T12:00:00Z".into()).unwrap();
    let document: Value = serde_json::from_slice(&encode(&document).unwrap()).unwrap();
    let stored = sorted(document["discussions"].as_array().unwrap().clone());
    let bots = bot_flags(&fixture, &raw);
    assert!(
        bots.values().all(Value::is_boolean),
        "GitLab reports every author's bot flag"
    );
    assert_eq!(stored, expected_discussions(&raw, &bots));
    assert_eq!(
        stored.len() + system_only,
        raw.len(),
        "only system-only discussions are omitted"
    );

    let find = |discussion: &str| {
        captured
            .discussions()
            .iter()
            .find(|candidate| candidate.id().as_str() == discussion)
            .unwrap()
    };
    let inline = find(&inline_id);
    assert_eq!(inline.resolution(), Resolution::Unresolved);
    assert_eq!(inline.notes().len(), 2);
    assert_eq!(inline.notes()[0].author.as_str(), commenter.id.to_string());
    let person = inline.notes()[0].account.as_ref().unwrap();
    let user = fixture.admin.get(&format!("users/{}", commenter.id));
    assert_eq!(person.username.as_deref(), user["username"].as_str());
    assert_eq!(person.bot, BotStatus::NotBot);
    let raw_position = raw
        .iter()
        .find(|discussion| discussion["id"] == inline_id.as_str())
        .unwrap()["notes"][0]["position"]
        .clone();
    println!("Retained inline position: {raw_position}");
    assert_eq!(raw_position["new_line"], 4);
    assert_eq!(raw_position["head_sha"], refs["head_sha"]);
    assert_eq!(find(&thread_id).resolution(), Resolution::Resolved);
    let note_ids: Vec<String> = captured
        .discussions()
        .iter()
        .flat_map(|discussion| {
            discussion
                .notes()
                .iter()
                .map(|note| note.id.as_str().to_owned())
        })
        .collect();
    assert!(!note_ids.contains(&deleted.to_string()));
    let edited_note = captured
        .discussions()
        .iter()
        .flat_map(|discussion| discussion.notes())
        .find(|note| note.id.as_str() == edited.to_string())
        .unwrap();
    assert_eq!(edited_note.body, "Edited wording.");
    match (&bot, bot_note) {
        (Some(bot), Some(note)) => {
            let observed = captured
                .discussions()
                .iter()
                .flat_map(|discussion| discussion.notes())
                .find(|candidate| candidate.id.as_str() == note.to_string())
                .unwrap();
            assert_eq!(observed.author.as_str(), bot.id.to_string());
            let user = fixture.admin.get(&format!("users/{}", bot.id));
            println!(
                "Bot note author: bot={} username={}",
                user["bot"], user["username"]
            );
            let account = observed.account.as_ref().unwrap();
            assert_eq!(account.username.as_deref(), user["username"].as_str());
            assert_eq!(account.bot, BotStatus::Bot);
        }
        _ => println!("Bot note skipped: project access tokens are unavailable"),
    }

    // A note edit and a resolution change after capture are freshness differences.
    commenter.api.call(
        Method::PUT,
        &format!("{notes}/{edited}"),
        json!({"body":"Edited again after capture."}),
    );
    fixture.author.api.call(
        Method::PUT,
        &format!("{threads}/{thread_id}"),
        json!({"resolved":false}),
    );
    let fresh = client
        .capture_conversation(client.subject(), BotLookup::Resolve)
        .unwrap();
    assert_eq!(
        fresh.changes_since(&captured),
        [
            ConversationChange::ResolutionChanged {
                discussion: id(&thread_id.clone().into()),
                retained: Resolution::Resolved,
                current: Resolution::Unresolved,
            },
            ConversationChange::NoteBodyChanged(id(&edited.into())),
        ]
    );
    fixture.cleanup();
}

/// The target-scoped freshness rule with real discovery: after a round with
/// two findings, a follow-up fixes finding 1 only. Activity by a person and a
/// project bot elsewhere, a copied finding marker, a reply and a resolution in
/// finding 2's untouched thread, a reply to the summary and title and
/// description edits are outside its targets; the author's reply and
/// resolution in finding 1's thread are the only reported changes.
pub fn write_target_scope_follows_the_plan(service: &GitLabService) {
    let mut fixture = Fixture::new(service);
    let commenter = fixture.add_user("commenter", 30);
    let bot = fixture.add_project_bot(&date_after(30));
    let client = fixture.connect();
    let revision = fixture.revision();
    let first = finding(&revision, 1, "src/lib.rs", FindingStatus::Open, None);
    let second = finding(&revision, 2, "single.rs", FindingStatus::Open, None);
    let published = publish(
        &request(&client, &revision, vec![first, second.clone()]),
        &mut client.publication_host(Effects::review()),
        &mut client.clone(),
        &mut Renderer::default(),
        &mut PublicationProgress::default(),
    )
    .unwrap();
    assert_eq!(published.status, PublicationStatus::Completed);
    let captured_history = client.clone().discover(client.subject()).unwrap();
    let captured = client
        .capture_conversation(client.subject(), BotLookup::Skip)
        .unwrap();
    let root = |number: u64| {
        captured_history
            .findings
            .iter()
            .find(|root| root.finding.id().get() == number)
            .unwrap()
            .id
            .clone()
    };
    let (fixed_thread, untouched) = (root(1), root(2));
    let summary_note = captured_history.summaries[0].id.clone();
    let mut fixed = request(
        &client,
        &revision,
        vec![
            finding(&revision, 1, "src/lib.rs", FindingStatus::Fixed, None),
            second,
        ],
    );
    transition(&mut fixed, 1, FindingStatus::Open, FindingStatus::Fixed);
    let plan = plan_from_snapshot(
        &fixed,
        &captured_history,
        &mut client.clone(),
        &mut Renderer::default(),
    )
    .unwrap();
    let targets = WriteTargets::of(&plan, &[&captured_history], None);

    let path = fixture.mr_path();
    let notes = format!("{path}/notes");
    let threads = format!("{path}/discussions");
    commenter.api.post(
        &notes,
        json!({"body":"Could the error path log its cause?"}),
    );
    if let Some(bot) = &bot {
        bot.api.post(
            &notes,
            json!({"body":"Automated review: no blocking issues."}),
        );
        bot.api.post(
            &format!("{threads}/{}/notes", untouched.as_str()),
            json!({"body":"Automated note: the linked issue moved."}),
        );
    }
    let refs = fixture.admin.get(&path)["diff_refs"].clone();
    let copied = commenter.api.post(
        &threads,
        json!({"body":"Copied.\n\n<!-- review-kit:v1 {\"kind\":\"finding\",\"finding\":1,\"round\":1} -->",
            "position":{"position_type":"text","base_sha":refs["base_sha"],"start_sha":refs["start_sha"],
            "head_sha":refs["head_sha"],"old_path":"src/lib.rs","new_path":"src/lib.rs","new_line":3}}),
    );
    let copied = copied["id"].as_str().unwrap().to_owned();
    commenter.api.call(
        Method::PUT,
        &format!("{threads}/{copied}"),
        json!({"resolved":true}),
    );
    fixture.author.api.post(
        &format!("{threads}/{}/notes", untouched.as_str()),
        json!({"body":"I disagree with finding 2."}),
    );
    fixture.author.api.call(
        Method::PUT,
        &format!("{threads}/{}", untouched.as_str()),
        json!({"resolved":true}),
    );
    let summary_id: u64 = summary_note.as_str().parse().unwrap();
    let summary_thread = fixture
        .discussions()
        .into_iter()
        .find(|discussion| discussion["notes"][0]["id"] == summary_id)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    commenter.api.post(
        &format!("{threads}/{summary_thread}/notes"),
        json!({"body":"Thanks for the summary."}),
    );
    fixture.author.api.call(
        Method::PUT,
        &path,
        json!({"title":"Write target scope verification","description":"Edited after capture."}),
    );
    let current_history = client.clone().discover(client.subject()).unwrap();
    assert_eq!(
        current_history.findings.len(),
        2,
        "a copied finding marker is not an owned root"
    );
    let current = client
        .capture_conversation(client.subject(), BotLookup::Skip)
        .unwrap();
    assert!(current.changes_since(&captured).len() >= 8);
    assert_eq!(current.changes_to(&captured, &targets), []);

    let reply = fixture.author.api.post(
        &format!("{threads}/{}/notes", fixed_thread.as_str()),
        json!({"body":"Does the regression cover this case?"}),
    );
    fixture.author.api.call(
        Method::PUT,
        &format!("{threads}/{}", fixed_thread.as_str()),
        json!({"resolved":true}),
    );
    let current = client
        .capture_conversation(client.subject(), BotLookup::Skip)
        .unwrap();
    assert_eq!(
        current.changes_to(&captured, &targets),
        [
            ConversationChange::ResolutionChanged {
                discussion: fixed_thread.clone(),
                retained: Resolution::Unresolved,
                current: Resolution::Resolved,
            },
            ConversationChange::NoteAdded {
                discussion: fixed_thread,
                note: id(&reply["id"]),
            },
        ]
    );
    fixture.cleanup();
}
