//! Offline discovery through a custom reader, with generated operation logs on stderr.

use review_kit_application::{
    ports::{PortError, ReviewReader},
    publication::{Effects, Snapshot},
    use_cases::discovery::discover,
};
use review_kit_domain::{ReviewerId, Revision, Subject};
use review_kit_observation::{Observer, with_observer};

struct ExampleReader(Snapshot);

impl ReviewReader for ExampleReader {
    fn discover(&mut self, _: &Subject) -> Result<Snapshot, PortError> {
        Ok(self.0.clone())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subject = Subject::new(
        "gitlab".into(),
        "example.invalid".into(),
        "private-group/private-project".into(),
        7,
    )?;
    let reviewer = ReviewerId::try_from("private-reviewer".to_owned())?;
    let mut reader = ExampleReader(Snapshot {
        subject: subject.clone(),
        reviewer: reviewer.clone(),
        revision: Revision::new("private-head".into(), vec!["private-base".into()])?,
        summaries: vec![],
        findings: vec![],
        replies: vec![],
        permissions: Effects::none(),
    });

    let observer = Observer::json(std::io::stderr());
    let snapshot = with_observer(&observer, || discover(&mut reader, &subject, &reviewer))?;
    println!("Discovered {} findings.", snapshot.findings.len());
    Ok(())
}
