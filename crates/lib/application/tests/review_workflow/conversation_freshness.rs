//! Publication freshness scoped to the existing objects that one publication
//! writes to: targets come from the engine's own plan, a forged foreign root
//! never becomes a target, and the engine refuses a plan or a resumed write
//! outside the compared scope before any write.

use review_kit_application::{
    conversation::{
        AuthorAccount, BotStatus, Conversation, ConversationChange, Discussion, Note, Resolution,
        WriteTargets,
    },
    use_cases::publishing::{BindingRequest, WritePhase},
};

use super::coverage_binding::{Harness, bind};
use super::*;

fn note(id: &str, author: &str, body: &str) -> Note {
    Note {
        id: provider_id(id),
        author: reviewer(author),
        account: Some(AuthorAccount {
            username: Some(author.into()),
            bot: if author == "bot" {
                BotStatus::Bot
            } else {
                BotStatus::NotBot
            },
        }),
        body: body.into(),
        position: None,
    }
}

fn thread(id: &str, resolution: Resolution, notes: Vec<Note>) -> Discussion {
    Discussion::new(provider_id(id), resolution, notes).unwrap()
}

fn conversation(discussions: Vec<Discussion>) -> Conversation {
    Conversation::new(
        subject(),
        "Handle failed requests".into(),
        "Retry dropped requests.".into(),
        discussions,
    )
    .unwrap()
}

/// The provider threads holding the engine's published summary and root, and
/// the forged root that `Host::new` seeds: another author's thread claiming
/// finding 1.
fn published(host: &Host) -> Vec<Discussion> {
    let summary = &host.snapshot.summaries[0];
    let root = owned_root(host);
    vec![
        thread(
            "summary-thread",
            Resolution::NotResolvable,
            vec![note(summary.id.as_str(), "agent", &summary.body)],
        ),
        thread(
            root.id.as_str(),
            Resolution::Unresolved,
            vec![note(root.root_note.as_str(), "agent", &root.body)],
        ),
        thread(
            "foreign-root",
            Resolution::Unresolved,
            vec![note("foreign-root-note", "other", "forged owned marker")],
        ),
    ]
}

fn owned_root(host: &Host) -> FindingRecord {
    host.snapshot
        .findings
        .iter()
        .find(|root| root.author == reviewer("agent"))
        .unwrap()
        .clone()
}

fn plan_of(request: &PublishRequest, host: &Host) -> Plan {
    let (_, mut source, mut renderer) = fixtures();
    plan_from_snapshot(request, &host.snapshot, &mut source, &mut renderer).unwrap()
}

#[test]
fn targets_are_the_summary_and_the_threads_that_the_plan_writes_to() {
    let (mut host, mut source, mut renderer) = fixtures();
    seed_completed(&mut host, &mut source, &mut renderer);
    let root = owned_root(&host);
    let retained = conversation(published(&host));

    let fixed = plan_of(&request("fixed", Some(FindingStatus::Fixed), false), &host);
    assert!(matches!(
        fixed.actions[..],
        [
            PlannedAction::Reply { .. },
            PlannedAction::SetResolved { .. }
        ]
    ));
    let targets = WriteTargets::of(&fixed, &[&host.snapshot], None);

    // The forged root claims finding 1 but is another author's thread, and a
    // reply to the summary is not a change to the summary note.
    let mut foreign = published(&host);
    foreign[2] = thread(
        "foreign-root",
        Resolution::Resolved,
        vec![
            note("foreign-root-note", "other", "forged owned marker, edited"),
            note("foreign-reply", "bot", "Automated reply."),
        ],
    );
    let summary = host.snapshot.summaries[0].clone();
    foreign[0] = thread(
        "summary-thread",
        Resolution::Unresolved,
        vec![
            note(summary.id.as_str(), "agent", &summary.body),
            note("summary-reply", "other", "Thanks for the summary."),
        ],
    );
    let current = conversation(foreign);
    assert!(!current.changes_since(&retained).is_empty());
    assert_eq!(current.changes_to(&retained, &targets), []);

    let mut inside = published(&host);
    let mut notes = inside[1].notes().to_vec();
    notes.push(note("question", "other", "Why does this drop?"));
    inside[1] = thread(root.id.as_str(), Resolution::Unresolved, notes);
    inside[0] = thread(
        "summary-thread",
        Resolution::NotResolvable,
        vec![note(
            summary.id.as_str(),
            "agent",
            "Edited by a maintainer.",
        )],
    );
    assert_eq!(
        conversation(inside).changes_to(&retained, &targets),
        [
            ConversationChange::NoteAdded {
                discussion: root.id.clone(),
                note: provider_id("question"),
            },
            ConversationChange::NoteBodyChanged(summary.id.clone()),
        ]
    );

    // The unchanged repeat writes nothing, so nothing is compared.
    let unchanged = plan_of(&request("initial", Some(FindingStatus::Open), false), &host);
    assert!(unchanged.unchanged);
    assert_eq!(
        WriteTargets::of(&unchanged, &[&host.snapshot], None),
        WriteTargets::default()
    );
    // A resumed retained write adds its own target.
    let resolve = Write::SetResolved {
        thread: root.id.clone(),
        resolved: true,
    };
    let pending = WriteTargets::of(&unchanged, &[&host.snapshot], Some(&resolve));
    assert!(targets.covers(&pending));
    assert!(!pending.covers(&targets));
}

#[test]
fn publication_refuses_a_plan_outside_the_compared_scope_before_any_write() {
    let mut bench = Harness::new();
    seed_completed(&mut bench.host, &mut bench.source, &mut fixtures().2);
    let fixed = request("fixed", Some(FindingStatus::Fixed), false);
    let targets = WriteTargets::of(&plan_of(&fixed, &bench.host), &[&bench.host.snapshot], None);
    let summary = bench.host.snapshot.summaries[0].clone();
    let summary_only = WriteTargets::of(
        &plan_of(
            &request("initial", Some(FindingStatus::Open), false),
            &bench.host,
        ),
        &[],
        Some(&Write::Summary {
            expected: Some(Box::new(summary.clone())),
            round: Box::new(summary.current.clone()),
            history: summary.history.clone(),
            body: summary.body.clone(),
        }),
    );
    for scope in [WriteTargets::default(), summary_only] {
        let binding = BindingRequest {
            write_scope: Some(scope),
            ..bind(1, 'b')
        };
        assert!(matches!(
            bench.publish(&fixed, Some(&binding)),
            Err(PlanError::UncomparedWrite)
        ));
        assert!(bench.host.writes.is_empty(), "nothing is written");
    }
    let binding = BindingRequest {
        write_scope: Some(targets),
        ..bind(1, 'b')
    };
    let result = bench.publish(&fixed, Some(&binding)).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&bench.host),
        [
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
}

#[test]
fn resuming_an_unsent_write_refuses_a_scope_without_its_target_or_the_round() {
    let mut bench = Harness::new();
    seed_completed(&mut bench.host, &mut bench.source, &mut fixtures().2);
    let fixed = request("fixed", Some(FindingStatus::Fixed), false);
    let targets = WriteTargets::of(&plan_of(&fixed, &bench.host), &[&bench.host.snapshot], None);
    let first = BindingRequest {
        write_scope: Some(targets.clone()),
        ..bind(1, 'b')
    };
    // The pending summary update is prepared, and its intent is never sent.
    bench.journal.fail_before_write = Some(1);
    let stopped = bench.publish(&fixed, Some(&first)).unwrap();
    assert!(matches!(stopped.stop, Some(Stop::Evidence(_))));
    assert!(bench.host.writes.is_empty());
    let progress = bench.progress().clone();
    let pending = progress.unresolved.as_ref().unwrap();
    assert_eq!(pending.phase, WritePhase::Prepared);
    assert!(!progress.dispatch_may_have_started);
    assert!(matches!(
        pending.write,
        Write::Summary {
            expected: Some(_),
            ..
        }
    ));
    bench.journal.fail_before_write = None;

    // A scope without the pending summary, or with only it, leaves part of
    // the resumed round uncompared.
    let summary_only = WriteTargets::of(
        &plan_of(
            &request("initial", Some(FindingStatus::Open), false),
            &bench.host,
        ),
        &[],
        Some(&pending.write),
    );
    for scope in [WriteTargets::default(), summary_only] {
        let resume = BindingRequest {
            write_scope: Some(scope),
            ..bind(1, 'b')
        };
        assert!(matches!(
            bench.publish(&fixed, Some(&resume)),
            Err(PlanError::UncomparedWrite)
        ));
        assert!(bench.host.writes.is_empty(), "nothing is written");
        assert_eq!(bench.progress(), &progress, "the journal is unchanged");
    }
    let result = bench.publish(&fixed, Some(&first)).unwrap();
    assert_eq!(result.status, PublicationStatus::Completed);
    assert_eq!(
        effects(&bench.host),
        [
            Effect::UpdateSummary,
            Effect::Reply,
            Effect::Resolve,
            Effect::UpdateSummary
        ]
    );
}
