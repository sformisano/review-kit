use serde_json::json;

use super::{
    harness::{ERROR_POLICY, Harness},
    proxy::Proxy,
    server,
};

#[test]
fn runs_for_several_merge_requests_share_one_worktree_and_are_selected_explicitly() {
    let second = server::Fixture::new();
    let first = server::Fixture::new();
    let proxy = Proxy::start(&first, &second);
    let harness = Harness::with_server(first, proxy.url.clone());

    let seven = harness.ok(&["run", "start", "--mr", &harness.mr_number(7)]);
    let eight = harness.ok(&["run", "start", "--mr", &harness.mr_number(8)]);
    let seven = seven["run_id"].as_str().unwrap().to_owned();
    let eight = eight["run_id"].as_str().unwrap().to_owned();
    assert_eq!(harness.selected(), eight);
    let listed = harness.ok(&["run", "list"]);
    let runs = listed["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    for (run_id, request) in [(&seven, 7), (&eight, 8)] {
        let run = runs.iter().find(|run| run["run_id"] == **run_id).unwrap();
        assert_eq!(run["subject"]["request"], request);
        assert_eq!(run["subject"]["repository"], "group/sub/repo");
    }
    assert_eq!(
        harness.manifest(&eight)["revision"]["head"],
        super::proxy::SECOND_HEAD
    );

    harness.ok(&["run", "select", &seven]);
    harness.declare_policies(&seven, &[ERROR_POLICY]);

    harness.fill_finding(&seven);
    harness.ok(&["accept"]);
    harness.fails(
        &["run", "start", "--mr", &harness.mr_number(7)],
        5,
        "open_run_exists",
    );
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert_eq!(
        harness.discussions(),
        1,
        "merge request 7 received its finding"
    );
    assert_eq!(
        second.state.lock().unwrap().writes().len(),
        0,
        "merge request 8 is untouched"
    );

    harness.ok(&["run", "select", &eight]);
    assert_eq!(harness.ok(&["status"])["subject"]["request"], 8);
    harness.declare_policies(&eight, &[ERROR_POLICY]);

    harness.fill_ready(&eight);
    harness.ok(&["accept"]);
    let published = harness.ok(&["publish", "--effects", "review"]);
    assert_eq!(published["status"], "settled", "{published:#}");
    assert!(!second.state.lock().unwrap().writes().is_empty());
    let journals = std::fs::read_dir(harness.state())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .count();
    assert_eq!(journals, 2, "each merge request has its own journal");
    assert_eq!(harness.manifest(&seven)["completion"], json!("published"));
}
