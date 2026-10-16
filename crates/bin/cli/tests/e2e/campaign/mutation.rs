//! Provider mutations for the follow-up matrix. Each operation verifies that
//! the campaign owns the merge request before it writes and saves a private
//! evidence record with its inputs and the provider identifiers it changed.
//! Note and thread operations act as the fixture author or commenter; only
//! edits, deletions and managed-root removal may act as the reviewer, so a
//! campaign can corrupt managed history on purpose.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};

use super::{Campaign, Role, Target, commit_change, family};
use crate::fixture::{marker, process, provision};

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn commenting(role: Role) -> Result<()> {
    ensure!(
        matches!(role, Role::Author | Role::Commenter),
        "this operation acts as the author or the commenter"
    );
    Ok(())
}

impl Campaign {
    fn record_mutation(&self, operation: &str, target: &Target, details: Value) -> Result<Value> {
        let mut record = json!({"operation": operation, "family": target.family, "merge_request": target.label, "url": self.record(target)?["url"]});
        if let (Some(record), Some(details)) = (record.as_object_mut(), details.as_object()) {
            record.extend(details.clone());
        }
        let path = self.evidence("mutations", operation, &record)?;
        record["evidence"] = json!(path);
        Ok(record)
    }

    fn target_head(&self, target: &Target) -> Result<String> {
        let branch = self.record(target)?["target"]
            .as_str()
            .context("target branch")?
            .to_owned();
        self.branch_head(
            &self.api(Role::Author)?,
            &self.project_id(&target.family)?,
            &branch,
        )
    }

    /// Commits a named overlay to the merge request branch, or with
    /// `to_target` to its target branch, as the author.
    pub fn apply(&mut self, target: &Target, overlay: &str, to_target: bool) -> Result<Value> {
        let before = self.verify_merge_request(target)?;
        let family = family::load(&family::root(), &target.family)?;
        let (change, message) = family.change(overlay)?;
        let branch = if to_target {
            &before["target_branch"]
        } else {
            &before["source_branch"]
        }
        .as_str()
        .context("branch")?
        .to_owned();
        let author = self.api(Role::Author)?;
        let project = self.project_id(&target.family)?;
        let head = commit_change(
            &author,
            &project,
            &branch,
            None,
            &change,
            message,
            &family.descriptor.executable,
        )?;
        let mut details = json!({"overlay": overlay, "branch": branch, "commit": head, "files": change.write.keys().collect::<Vec<_>>(), "deleted": change.delete, "before": before["diff_refs"]});
        if !to_target {
            provision::wait_diff(
                &author,
                &project,
                &before["iid"],
                &head,
                Some(&self.target_head(target)?),
            )?;
            self.settle(target)?;
            self.manifest["families"][&target.family]["merge_requests"][&target.label]["applied"]
                .as_array_mut()
                .context("applied overlays")?
                .push(json!(overlay));
            let after = self.refresh_record(target)?;
            details["after"] = after["diff_refs"].clone();
            details["checkout"] = json!(self.checkout(target)?);
        }
        self.record_mutation("apply", target, details)
    }

    /// Replays the merge request commits onto `onto` (default: the current
    /// target branch head) with new commit identities, optionally squashed
    /// into one commit, and force-pushes the branch as the author.
    pub fn force_push(
        &mut self,
        target: &Target,
        onto: Option<&str>,
        squash: bool,
        message: Option<&str>,
    ) -> Result<Value> {
        let before = self.verify_merge_request(target)?;
        let branch = before["source_branch"]
            .as_str()
            .context("source branch")?
            .to_owned();
        let old_head = before["diff_refs"]["head_sha"]
            .as_str()
            .context("head")?
            .to_owned();
        let base = before["diff_refs"]["base_sha"]
            .as_str()
            .context("base")?
            .to_owned();
        let work = self.author_clone(target, &branch)?;
        let onto = match onto {
            None => {
                let target_branch = before["target_branch"].as_str().context("target branch")?;
                self.git(
                    &work,
                    "fetch-target",
                    &["fetch", "--quiet", "origin", target_branch],
                )?;
                self.git_output(&work, &["rev-parse", "FETCH_HEAD"])?
            }
            Some("initial") => self.family_record(&target.family)?["initial_head"]
                .as_str()
                .context("initial head")?
                .to_owned(),
            Some(reference)
                if reference.len() == 40
                    && reference.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                reference.to_owned()
            }
            Some(reference) => {
                self.git(
                    &work,
                    "fetch-onto",
                    &["fetch", "--quiet", "origin", reference],
                )?;
                self.git_output(&work, &["rev-parse", "FETCH_HEAD"])?
            }
        };
        self.git(
            &work,
            "rebase",
            &[
                "rebase",
                "--quiet",
                "--force-rebase",
                "--onto",
                &onto,
                &base,
                &branch,
            ],
        )?;
        if squash {
            self.git(&work, "squash-reset", &["reset", "--soft", &onto])?;
            let message = message.unwrap_or("Combine the merge request changes");
            self.git(
                &work,
                "squash-commit",
                &["commit", "--quiet", "--message", message],
            )?;
        }
        let head = self.git_output(&work, &["rev-parse", "HEAD"])?;
        ensure!(head != old_head, "rewritten history kept the previous head");
        let lease = format!("--force-with-lease={branch}:{old_head}");
        self.git(
            &work,
            "force-push",
            &[
                "push",
                "--quiet",
                &lease,
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
        )?;
        let author = self.api(Role::Author)?;
        provision::wait_diff(
            &author,
            &self.project_id(&target.family)?,
            &before["iid"],
            &head,
            Some(&self.target_head(target)?),
        )?;
        self.settle(target)?;
        let after = self.refresh_record(target)?;
        let checkout = self.checkout(target)?;
        self.record_mutation(
            "force-push",
            target,
            json!({"onto": onto, "squash": squash, "previous_head": old_head, "head": head, "before": before["diff_refs"], "after": after["diff_refs"], "checkout": checkout}),
        )
    }

    /// Points the merge request at `branch`, creating that branch from `from`
    /// (a branch, a commit or `initial`) and committing `overlay` to it when
    /// given. The source head stays unchanged.
    pub fn retarget(
        &mut self,
        target: &Target,
        branch: &str,
        from: &str,
        overlay: Option<&str>,
    ) -> Result<Value> {
        let before = self.verify_merge_request(target)?;
        ensure!(
            branch != before["source_branch"],
            "the target must differ from the source branch"
        );
        let author = self.api(Role::Author)?;
        let project = self.project_id(&target.family)?;
        let from = if from == "initial" {
            self.family_record(&target.family)?["initial_head"]
                .as_str()
                .context("initial head")?
                .to_owned()
        } else {
            from.to_owned()
        };
        let path = format!(
            "projects/{project}/repository/branches/{}",
            super::encode(branch)
        );
        let created = if author.get_if_exists(&path)?.is_none() {
            author.post(
                &format!("projects/{project}/repository/branches"),
                &json!({"branch": branch, "ref": from}),
            )?;
            true
        } else {
            false
        };
        let mut commit = Value::Null;
        if let Some(overlay) = overlay {
            let family = family::load(&family::root(), &target.family)?;
            let (change, message) = family.change(overlay)?;
            commit = json!(commit_change(
                &author,
                &project,
                branch,
                None,
                &change,
                message,
                &family.descriptor.executable
            )?);
        }
        let head = before["diff_refs"]["head_sha"]
            .as_str()
            .context("head")?
            .to_owned();
        author.put(
            &self.merge_request_path(target)?,
            &json!({"target_branch": branch}),
        )?;
        self.manifest["families"][&target.family]["merge_requests"][&target.label]["target"] =
            json!(branch);
        self.save()?;
        let start = self.branch_head(&author, &project, branch)?;
        provision::wait_diff(&author, &project, &before["iid"], &head, Some(&start))?;
        self.settle(target)?;
        let after = self.refresh_record(target)?;
        ensure!(
            after["diff_refs"]["head_sha"] == before["diff_refs"]["head_sha"],
            "retargeting changed the source head"
        );
        self.record_mutation(
            "retarget",
            target,
            json!({"branch": branch, "created_from": if created { json!(from) } else { Value::Null }, "overlay": overlay, "overlay_commit": commit, "previous_target": before["target_branch"], "before": before["diff_refs"], "after": after["diff_refs"]}),
        )
    }

    pub fn edit_description(&mut self, target: &Target, description: &str) -> Result<Value> {
        let before = self.verify_merge_request(target)?;
        let after = self.api(Role::Author)?.put(
            &self.merge_request_path(target)?,
            &json!({"description": description}),
        )?;
        ensure!(
            after["description"] == description,
            "GitLab did not store the description"
        );
        self.record_mutation(
            "edit-description",
            target,
            json!({"before_sha256": digest(before["description"].as_str().unwrap_or("")), "after_sha256": digest(description), "updated_at": after["updated_at"]}),
        )
    }

    pub fn note(&mut self, target: &Target, role: Role, body: &str) -> Result<Value> {
        commenting(role)?;
        self.verify_merge_request(target)?;
        let note = self.api(role)?.post(
            &format!("{}/notes", self.merge_request_path(target)?),
            &json!({"body": body}),
        )?;
        self.record_mutation(
            "note",
            target,
            json!({"as": role.key(), "note": note["id"], "author": note["author"]["id"], "body_sha256": digest(body)}),
        )
    }

    /// Starts an unmanaged diff thread at `path`. `line` is the new-file line;
    /// `old_line` is also required for an unchanged context line.
    pub fn inline_note(
        &mut self,
        target: &Target,
        role: Role,
        position: InlinePosition<'_>,
        body: &str,
    ) -> Result<Value> {
        commenting(role)?;
        let mr = self.verify_merge_request(target)?;
        let refs = &mr["diff_refs"];
        let mut anchor = json!({"position_type": "text", "base_sha": refs["base_sha"], "start_sha": refs["start_sha"], "head_sha": refs["head_sha"], "new_path": position.path, "old_path": position.old_path.unwrap_or(position.path), "new_line": position.line});
        if let Some(old_line) = position.old_line {
            anchor["old_line"] = json!(old_line);
        }
        let discussion = self.api(role)?.post(
            &format!("{}/discussions", self.merge_request_path(target)?),
            &json!({"body": body, "position": anchor}),
        )?;
        let note = &discussion["notes"][0];
        ensure!(
            note["type"] == "DiffNote" && note["position"]["new_line"] == position.line,
            "GitLab did not anchor the note at the requested line"
        );
        self.record_mutation(
            "inline-note",
            target,
            json!({"as": role.key(), "discussion": discussion["id"], "note": note["id"], "position": note["position"], "body_sha256": digest(body)}),
        )
    }

    /// Finds the discussion that contains `note` among all discussion pages.
    pub fn discussion_of(&self, target: &Target, note: u64) -> Result<String> {
        let discussions = self
            .api(Role::Author)?
            .pages(&format!("{}/discussions", self.merge_request_path(target)?))?;
        discussions
            .as_array()
            .context("discussions")?
            .iter()
            .find(|discussion| {
                discussion["notes"]
                    .as_array()
                    .is_some_and(|notes| notes.iter().any(|candidate| candidate["id"] == note))
            })
            .and_then(|discussion| discussion["id"].as_str())
            .map(str::to_owned)
            .with_context(|| format!("no discussion contains note {note}"))
    }

    pub fn reply(
        &mut self,
        target: &Target,
        role: Role,
        discussion: &str,
        body: &str,
    ) -> Result<Value> {
        commenting(role)?;
        self.verify_merge_request(target)?;
        let note = self.api(role)?.post(
            &format!(
                "{}/discussions/{discussion}/notes",
                self.merge_request_path(target)?
            ),
            &json!({"body": body}),
        )?;
        self.record_mutation(
            "reply",
            target,
            json!({"as": role.key(), "discussion": discussion, "note": note["id"], "body_sha256": digest(body)}),
        )
    }

    pub fn edit_note(
        &mut self,
        target: &Target,
        role: Role,
        note: u64,
        body: &str,
    ) -> Result<Value> {
        self.verify_merge_request(target)?;
        let path = format!("{}/notes/{note}", self.merge_request_path(target)?);
        let before = self.api(Role::Author)?.get(&path)?;
        let after = self.api(role)?.put(&path, &json!({"body": body}))?;
        ensure!(
            after["body"] == body,
            "GitLab did not store the edited note"
        );
        self.record_mutation(
            "edit-note",
            target,
            json!({"as": role.key(), "note": note, "note_author": before["author"]["id"], "before_sha256": digest(before["body"].as_str().unwrap_or("")), "after_sha256": digest(body)}),
        )
    }

    pub fn delete_note(&mut self, target: &Target, role: Role, note: u64) -> Result<Value> {
        self.verify_merge_request(target)?;
        let path = format!("{}/notes/{note}", self.merge_request_path(target)?);
        let reader = self.api(Role::Author)?;
        let before = reader.get(&path)?;
        self.api(role)?.delete(&path)?;
        ensure!(
            reader.get_if_exists(&path)?.is_none(),
            "note {note} still exists"
        );
        self.record_mutation(
            "delete-note",
            target,
            json!({"as": role.key(), "note": note, "note_author": before["author"]["id"], "body_sha256": digest(before["body"].as_str().unwrap_or("")), "type": before["type"]}),
        )
    }

    pub fn resolve(
        &mut self,
        target: &Target,
        role: Role,
        discussion: &str,
        resolved: bool,
    ) -> Result<Value> {
        commenting(role)?;
        self.verify_merge_request(target)?;
        let after = self.api(role)?.put(
            &format!(
                "{}/discussions/{discussion}",
                self.merge_request_path(target)?
            ),
            &json!({"resolved": resolved}),
        )?;
        let states: Vec<_> = after["notes"]
            .as_array()
            .context("discussion notes")?
            .iter()
            .filter(|note| note["resolvable"] == true)
            .map(|note| note["resolved"].clone())
            .collect();
        ensure!(
            !states.is_empty() && states.iter().all(|state| *state == json!(resolved)),
            "GitLab did not apply the resolution state"
        );
        self.record_mutation(
            "resolve",
            target,
            json!({"as": role.key(), "discussion": discussion, "resolved": resolved}),
        )
    }

    /// Creates `count` general notes so that discussion listings span pages.
    pub fn bulk_notes(
        &mut self,
        target: &Target,
        role: Role,
        count: u32,
        prefix: &str,
    ) -> Result<Value> {
        commenting(role)?;
        ensure!(count > 0, "bulk note count must be positive");
        self.verify_merge_request(target)?;
        let api = self.api(role)?;
        let path = format!("{}/notes", self.merge_request_path(target)?);
        let mut created = Vec::new();
        for index in 1..=count {
            let note = api.post(
                &path,
                &json!({"body": format!("{prefix} {index} of {count}.")}),
            )?;
            created.push(note["id"].clone());
        }
        let discussions = self
            .api(Role::Author)?
            .pages(&format!("{}/discussions", self.merge_request_path(target)?))?
            .as_array()
            .context("discussions")?
            .len();
        self.record_mutation(
            "bulk-notes",
            target,
            json!({"as": role.key(), "count": count, "first_note": created.first(), "last_note": created.last(), "notes": created, "discussions_after": discussions, "discussion_pages": discussions.div_ceil(100)}),
        )
    }

    /// Deletes the root note of the reviewer's managed thread for `finding`,
    /// leaving any replies, so the provider history no longer matches the
    /// managed summary.
    pub fn delete_root(&mut self, target: &Target, role: Role, finding: u64) -> Result<Value> {
        ensure!(
            matches!(role, Role::Author | Role::Reviewer),
            "a managed root is deleted as the author or reviewer"
        );
        self.verify_merge_request(target)?;
        let reviewer = self.user_id(Role::Reviewer)?.clone();
        let discussions = self
            .api(Role::Author)?
            .pages(&format!("{}/discussions", self.merge_request_path(target)?))?;
        let discussion = discussions
            .as_array()
            .context("discussions")?
            .iter()
            .find(|discussion| {
                let root = &discussion["notes"][0];
                root["author"]["id"] == reviewer
                    && root["body"]
                        .as_str()
                        .and_then(marker)
                        .is_some_and(|record| {
                            record["kind"] == "finding" && record["finding"] == finding
                        })
            })
            .with_context(|| format!("no managed root for finding {finding}"))?
            .clone();
        let root = &discussion["notes"][0];
        let outcome =
            self.delete_note(target, role, root["id"].as_u64().context("root note id")?)?;
        self.record_mutation(
            "delete-root",
            target,
            json!({"as": role.key(), "finding": finding, "discussion": discussion["id"], "root_note": root["id"], "replies_kept": discussion["notes"].as_array().map_or(0, |notes| notes.len().saturating_sub(1)), "deletion": outcome["evidence"]}),
        )
    }

    fn author_clone(&mut self, target: &Target, branch: &str) -> Result<PathBuf> {
        let directory = self.directory.join("work");
        std::fs::create_dir_all(&directory)?;
        let sequence = std::fs::read_dir(&directory)?.count() + 1;
        let work = directory.join(format!("{}-{}-{sequence}", target.family, target.label));
        provision::Checkout {
            directory: &self.directory,
            tokens: &self.tokens,
            server: self.server()?,
            project_path: self.family_record(&target.family)?["project_path"]
                .as_str()
                .context("project path")?,
            branch,
            checkout: &work,
            token_file: &self.token_file(Role::Author)?,
        }
        .run()?;
        Ok(work)
    }

    fn git_command(
        &self,
        work: &std::path::Path,
        arguments: &[&str],
    ) -> Result<std::process::Command> {
        let author = &self.manifest["users"]["author"]["username"];
        let author = author.as_str().context("author username")?;
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(work)
            .args([
                "-c",
                "http.proxy=",
                "-c",
                &format!("user.name={author}"),
                "-c",
                &format!("user.email={author}@example.test"),
            ])
            .args(arguments)
            .env("GIT_ASKPASS", self.directory.join("git-askpass.sh"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("RKC_FIXTURE_TOKEN_FILE", self.token_file(Role::Author)?);
        Ok(command)
    }

    fn git(
        &self,
        work: &std::path::Path,
        label: &str,
        arguments: &[&str],
    ) -> Result<std::process::Output> {
        process::execute_git(
            &self.directory,
            &self.tokens,
            label,
            self.git_command(work, arguments)?,
            Duration::from_secs(180),
        )
    }

    fn git_output(&self, work: &std::path::Path, arguments: &[&str]) -> Result<String> {
        Ok(
            String::from_utf8(self.git(work, "git-query", arguments)?.stdout)?
                .trim()
                .to_owned(),
        )
    }
}

/// A new-file line, with the old line for an unchanged context line.
pub struct InlinePosition<'a> {
    pub path: &'a str,
    pub old_path: Option<&'a str>,
    pub line: u64,
    pub old_line: Option<u64>,
}
