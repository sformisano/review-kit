#![allow(clippy::unwrap_used)]

use std::{error::Error, io};

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::{InvalidIdentity, Snapshot},
    use_cases::{
        assessment::{CheckIssue, CheckProblem, PolicyCoverageIssue},
        discovery::{DiscoveryConflict, DiscoveryError, discover},
        discovery_bundle::BundleError,
        initialization::InitializationError,
        publishing::PlanError,
    },
};
use review_kit_domain::{FindingId, PolicyId, ReviewerId, Subject};

fn failure() -> PortError {
    PortError::new(
        "read review",
        io::Error::other("private credential diagnostic"),
    )
}

fn source_as<T: Error + 'static>(error: &dyn Error) -> &T {
    error.source().unwrap().downcast_ref::<T>().unwrap()
}

#[test]
fn discovery_reports_the_operation_and_preserves_the_private_cause() {
    struct FailingReader;
    impl ReviewReader for FailingReader {
        fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
            Err(failure())
        }
    }

    let subject = Subject::new("gitlab".into(), "host".into(), "group/repo".into(), 7).unwrap();
    let reviewer = ReviewerId::try_from("reviewer".to_owned()).unwrap();
    let error = discover(&mut FailingReader, &subject, &reviewer).unwrap_err();

    assert_eq!(error.to_string(), "read review failed");
    let port = source_as::<PortError>(&error);
    let diagnostic = source_as::<io::Error>(port);
    assert_eq!(diagnostic.to_string(), "private credential diagnostic");
    assert!(diagnostic.source().is_none());
}

#[test]
fn delegated_errors_retain_each_wrapper_in_the_source_chain() {
    let bundle = BundleError::FindingState(PlanError::Discovery(DiscoveryError::Read(failure())));
    assert_eq!(bundle.to_string(), "read review failed");
    let plan = source_as::<PlanError>(&bundle);
    let discovery = source_as::<DiscoveryError>(plan);
    let port = source_as::<PortError>(discovery);
    assert_eq!(
        source_as::<io::Error>(port).to_string(),
        "private credential diagnostic"
    );

    let bundle = BundleError::Discovery(DiscoveryError::Conflict(DiscoveryConflict::WrongReviewer));
    assert_eq!(bundle.to_string(), "discovery conflict: WrongReviewer");
    let discovery = source_as::<DiscoveryError>(&bundle);
    let conflict = source_as::<DiscoveryConflict>(discovery);
    assert_eq!(*conflict, DiscoveryConflict::WrongReviewer);
    assert!(conflict.source().is_none());

    let ports: Vec<Box<dyn Error>> = vec![
        Box::new(BundleError::Changes(failure())),
        Box::new(InitializationError::Write(failure())),
        Box::new(PlanError::Evidence(failure())),
        Box::new(PlanError::Render(failure())),
    ];
    for error in ports {
        assert_eq!(error.to_string(), "read review failed");
        let port = source_as::<PortError>(error.as_ref());
        assert_eq!(
            source_as::<io::Error>(port).to_string(),
            "private credential diagnostic"
        );
    }

    let error = InitializationError::Discovery(DiscoveryConflict::WrongSubject);
    assert_eq!(error.to_string(), "discovery conflict: WrongSubject");
    assert_eq!(
        source_as::<DiscoveryConflict>(&error),
        &DiscoveryConflict::WrongSubject
    );
}

#[test]
fn grouped_plan_failures_display_counts_and_retain_the_diagnostics() -> Result<(), &'static str> {
    let policy = PolicyId::try_from("private-policy".to_owned()).unwrap();
    let cases = [
        (
            PlanError::ShapeChecks(vec![PlanError::Render(failure()), PlanError::EmptyBody]),
            "2 publication shape checks failed",
        ),
        (
            PlanError::ReplyChecks(vec![PlanError::Render(failure())]),
            "1 reply checks failed",
        ),
        (
            PlanError::Coverage(vec![PolicyCoverageIssue::Missing(policy.clone())]),
            "1 policy coverage checks failed",
        ),
        (
            PlanError::Checks(vec![CheckIssue {
                finding: None,
                problem: CheckProblem::Source(failure()),
            }]),
            "1 assessment checks failed",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
        match error {
            PlanError::ShapeChecks(errors) | PlanError::ReplyChecks(errors) => {
                let PlanError::Render(port) = &errors[0] else {
                    return Err("render diagnostic is missing");
                };
                assert_eq!(
                    source_as::<io::Error>(port).to_string(),
                    "private credential diagnostic"
                );
            }
            PlanError::Coverage(issues) => {
                assert_eq!(issues, vec![PolicyCoverageIssue::Missing(policy.clone())]);
            }
            PlanError::Checks(issues) => {
                let CheckProblem::Source(port) = &issues[0].problem else {
                    return Err("source diagnostic is missing");
                };
                assert_eq!(
                    source_as::<io::Error>(port).to_string(),
                    "private credential diagnostic"
                );
            }
            _ => return Err("unexpected grouped failure"),
        }
    }
    assert_eq!(
        PlanError::ShapeChecks(vec![]).to_string(),
        "0 publication shape checks failed"
    );
    Ok(())
}

#[test]
fn discovery_conflicts_keep_debug_id_spelling_while_plan_errors_use_numeric_ids() {
    let id = FindingId::try_from(1).unwrap();
    let conflicts = [
        (
            DiscoveryConflict::WrongSubject,
            "discovery conflict: WrongSubject",
        ),
        (
            DiscoveryConflict::WrongReviewer,
            "discovery conflict: WrongReviewer",
        ),
        (
            DiscoveryConflict::InvalidFindingState(id),
            "discovery conflict: InvalidFindingState(FindingId(1))",
        ),
        (
            DiscoveryConflict::MultipleSummaries,
            "discovery conflict: MultipleSummaries",
        ),
        (
            DiscoveryConflict::DuplicateFinding(id),
            "discovery conflict: DuplicateFinding(FindingId(1))",
        ),
        (
            DiscoveryConflict::DuplicateProviderId,
            "discovery conflict: DuplicateProviderId",
        ),
        (
            DiscoveryConflict::InvalidRound,
            "discovery conflict: InvalidRound",
        ),
        (
            DiscoveryConflict::ForeignRound,
            "discovery conflict: ForeignRound",
        ),
    ];
    for (error, expected) in conflicts {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
    }
    assert_eq!(
        PlanError::MissingTransition(id).to_string(),
        "finding 1 needs a transition explanation",
    );
    assert_eq!(
        PlanError::ConflictingFinding(id).to_string(),
        "published evidence conflicts for finding 1",
    );
}

#[test]
fn leaf_application_errors_do_not_gain_a_source() {
    let errors: Vec<(Box<dyn Error>, &str)> = vec![
        (Box::new(InvalidIdentity), "identity must not be blank"),
        (
            Box::new(BundleError::WrongSubject),
            "changed files belong to another review subject",
        ),
        (
            Box::new(BundleError::ChangedRevision),
            "review revision changed while collecting discovery",
        ),
        (
            Box::new(BundleError::FindingIdExhausted),
            "no finding number remains after the current maximum",
        ),
    ];
    for (error, expected) in errors {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
    }
}
