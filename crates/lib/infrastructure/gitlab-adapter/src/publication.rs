use crate::observation;
use crate::{
    GitLab, GitLabError,
    http::segment,
    wire::{self, Marker},
};
use reqwest::Method;
use review_kit_application::{
    ports::{PortError, PublicationHost, PublicationSession, ReviewReader, WriteOutcome},
    publication::*,
};
use review_kit_domain::{ReviewerId, Subject};
use review_kit_observation::Fields;
use serde_json::{Value, json};

pub struct GitLabHost {
    client: GitLab,
    authority: Effects,
}
#[review_kit_observation::operations]
impl GitLabHost {
    #[helper]
    pub(crate) fn new(client: GitLab, authority: Effects) -> Self {
        Self { client, authority }
    }
    #[helper]
    pub fn set_authority(&mut self, authority: Effects) {
        self.authority = authority;
    }
}
#[review_kit_observation::operations]
impl PublicationHost for GitLabHost {
    #[operation(
        name = "gitlab.acquire",
        input = Fields::new(),
        output = |result| observation::port_result(result, |_| Fields::new()),
    )]
    fn acquire(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
    ) -> Result<Box<dyn PublicationSession + '_>, PortError> {
        self.client
            .check_subject(subject)
            .map_err(|e| e.port("acquire GitLab publisher"))?;
        if reviewer != self.client.reviewer() {
            return Err(GitLabError::Identity.port("acquire GitLab publisher"));
        }
        Ok(Box::new(Session {
            client: &self.client,
            authority: &self.authority,
        }) as Box<dyn PublicationSession>)
    }
}
struct Session<'a> {
    client: &'a GitLab,
    authority: &'a Effects,
}
struct PreparedRequest {
    method: Method,
    path: String,
    payload: Value,
}
#[review_kit_observation::operations]
impl Session<'_> {
    #[helper]
    fn bind(&self, subject: &Subject, reviewer: &ReviewerId) -> Result<(), GitLabError> {
        self.client.check_subject(subject)?;
        if reviewer != self.client.reviewer() {
            return Err(GitLabError::Identity);
        }
        Ok(())
    }
    #[operation(
        name = "gitlab.prepare_write",
        input = observation::write_input(write),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    fn prepared(&self, write: &Write, snapshot: &Snapshot) -> Result<PreparedRequest, GitLabError> {
        let base = self.client.mr_path();
        let (method, path, payload) = match write {
            Write::Summary {
                expected,
                round,
                history,
                body,
            } => {
                if round.assessment.subject() != self.client.subject()
                    || round.assessment.reviewer() != self.client.reviewer()
                {
                    return Err(GitLabError::Identity);
                }
                if round.assessment.revision() != &snapshot.revision
                    && !matches!(
                        round.status,
                        RoundStatus::Superseded | RoundStatus::Abandoned
                    )
                {
                    return Err(GitLabError::RevisionChanged);
                }
                if let Some(expected) = expected {
                    if snapshot
                        .summaries
                        .iter()
                        .filter(|s| s.author == snapshot.reviewer)
                        .count()
                        != 1
                        || !snapshot.summaries.contains(expected.as_ref())
                    {
                        return Err(GitLabError::Conflict);
                    }
                } else if snapshot
                    .summaries
                    .iter()
                    .any(|s| s.author == snapshot.reviewer)
                {
                    return Err(GitLabError::Conflict);
                }
                let body = wire::encode_summary(body, round, history)?;
                let (method, path) = match expected {
                    Some(summary) => (
                        Method::PUT,
                        format!("{base}/notes/{}", segment(summary.id.as_str())),
                    ),
                    None => (Method::POST, format!("{base}/notes")),
                };
                (method, path, json!({"body":body}))
            }
            Write::Finding {
                round,
                revision,
                finding,
                body,
            } => {
                if &snapshot.revision != revision {
                    return Err(GitLabError::RevisionChanged);
                }
                let position = self.client.position(revision, finding)?;
                crate::body::validate_suggestion(body, finding)?;
                let body = wire::append_marker(
                    body,
                    &Marker::Finding {
                        finding: finding.id().get(),
                        round: *round,
                    },
                )?;
                (
                    Method::POST,
                    format!("{base}/discussions"),
                    json!({"body":body,"position":position}),
                )
            }
            Write::Reply {
                thread,
                action,
                body,
            } => {
                let round = snapshot
                    .summaries
                    .iter()
                    .filter(|s| s.author == snapshot.reviewer)
                    .flat_map(|s| s.history.iter().chain(std::iter::once(&s.current)))
                    .find(|round| round.number == action.round)
                    .ok_or(GitLabError::State)?;
                if round.assessment.revision() != &snapshot.revision {
                    return Err(GitLabError::RevisionChanged);
                }
                let root = snapshot
                    .findings
                    .iter()
                    .find(|f| f.id == *thread && f.author == snapshot.reviewer)
                    .ok_or(GitLabError::Identity)?;
                if root.finding.id() != action.finding {
                    return Err(GitLabError::Identity);
                }
                let marker = match &action.kind {
                    ReplyKind::Transition(_) => Marker::Transition {
                        finding: action.finding.get(),
                        round: action.round,
                    },
                    ReplyKind::Answer(note) => {
                        let mut targets = round
                            .answer_targets
                            .iter()
                            .filter(|target| &target.id == note && target.thread == *thread);
                        let target = targets.next().ok_or(GitLabError::Conflict)?;
                        if targets.next().is_some() || !snapshot.replies.contains(target) {
                            return Err(GitLabError::Conflict);
                        }
                        Marker::Answer {
                            finding: action.finding.get(),
                            round: action.round,
                            note: note.as_str().into(),
                        }
                    }
                };
                let body = wire::append_marker(body, &marker)?;
                (
                    Method::POST,
                    format!("{base}/discussions/{}/notes", segment(thread.as_str())),
                    json!({"body":body}),
                )
            }
            Write::SetResolved { thread, resolved } => {
                snapshot
                    .findings
                    .iter()
                    .find(|f| f.id == *thread && f.author == snapshot.reviewer)
                    .ok_or(GitLabError::Identity)?;
                let round = &snapshot
                    .summaries
                    .iter()
                    .find(|s| s.author == snapshot.reviewer)
                    .ok_or(GitLabError::State)?
                    .current;
                if round.assessment.revision() != &snapshot.revision {
                    return Err(GitLabError::RevisionChanged);
                }
                (
                    Method::PUT,
                    format!("{base}/discussions/{}", segment(thread.as_str())),
                    json!({"resolved":resolved}),
                )
            }
        };
        if serde_json::to_vec(&payload)
            .map_err(|_| GitLabError::State)?
            .len()
            > self.client.api.limits.body_bytes
        {
            return Err(GitLabError::Limit);
        }
        Ok(PreparedRequest {
            method,
            path,
            payload,
        })
    }
    #[operation(
        name = "gitlab.write",
        input = observation::write_input(write),
        output = observation::mutation_result,
    )]
    fn send(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
        write: &Write,
        before_dispatch: &mut dyn FnMut() -> Result<(), GitLabError>,
    ) -> Result<(), GitLabError> {
        self.bind(subject, reviewer)?;
        let snapshot = self.client.snapshot()?;
        if !self.authority.contains(write.effect())
            || !snapshot.permissions.contains(write.effect())
        {
            return Err(GitLabError::Denied);
        }
        let request = self.prepared(write, &snapshot)?;
        self.client.api.mutate_with_checkpoint(
            request.method,
            &request.path,
            &request.payload,
            before_dispatch,
        )
    }
}
#[review_kit_observation::operations]
impl ReviewReader for Session<'_> {
    #[operation(
        name = "gitlab.publication.discover",
        input = Fields::new(),
        output = |result| {
            observation::port_result(result, |snapshot| {
                Fields::new()
                    .count("findings", snapshot.findings.len())
                    .count("summaries", snapshot.summaries.len())
                    .count("replies", snapshot.replies.len())
            })
        },
    )]
    fn discover(&mut self, subject: &Subject) -> Result<Snapshot, PortError> {
        self.client
            .check_subject(subject)
            .and_then(|()| self.client.snapshot())
            .map_err(|e| e.port("discover GitLab publication"))
    }
}
#[review_kit_observation::operations]
impl PublicationSession for Session<'_> {
    #[helper]
    fn authenticated_reviewer(&self) -> &ReviewerId {
        self.client.reviewer()
    }
    #[operation(
        name = "gitlab.authority",
        input = Fields::new(),
        output = |result| observation::port_result(result, observation::effects),
    )]
    fn authority(&mut self) -> Result<Effects, PortError> {
        Ok(self.authority.clone())
    }
    #[helper]
    fn write(&mut self, subject: &Subject, reviewer: &ReviewerId, write: &Write) -> WriteOutcome {
        self.write_with_checkpoint(subject, reviewer, write, &mut || Ok(()))
    }

    #[helper]
    fn write_with_checkpoint(
        &mut self,
        subject: &Subject,
        reviewer: &ReviewerId,
        write: &Write,
        before_dispatch: &mut dyn FnMut() -> Result<(), PortError>,
    ) -> WriteOutcome {
        let mut checkpoint_error = None;
        let outcome = self.send(subject, reviewer, write, &mut || {
            before_dispatch().map_err(|error| {
                checkpoint_error = Some(error);
                GitLabError::State
            })
        });
        if let Some(error) = checkpoint_error {
            return WriteOutcome::Rejected(error);
        }
        match outcome {
            Ok(()) => WriteOutcome::Acknowledged,
            Err(GitLabError::Uncertain) => {
                WriteOutcome::Unknown(GitLabError::Uncertain.port("write GitLab effect"))
            }
            Err(error) => WriteOutcome::Rejected(error.port("write GitLab effect")),
        }
    }
}
