use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use review_kit_application::review_run::{RunId, SnapshotFiles};
use review_kit_filesystem_adapter::{
    GitWorktree, RunFile, RunWriter, RuntimeStore, SnapshotFile, WorkingFile, WorkspaceLock,
    content_ref,
};
use serde::{Deserialize, Serialize};

/// A temporary Git repository with one initial commit, so linked worktrees can be added.
pub struct Repository {
    directory: tempfile::TempDir,
    root: PathBuf,
}

impl Repository {
    pub fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("rkc-runs-")
            .tempdir()
            .unwrap();
        let root = directory.path().join("main");
        fs::create_dir(&root).unwrap();
        let repository = Self {
            root: root.canonicalize().unwrap(),
            directory,
        };
        repository.git(&["init", "-q", "-b", "main"]);
        repository.git(&["commit", "-q", "--allow-empty", "-m", "initial"]);
        repository
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A directory beside the repository, outside every worktree of it.
    pub fn outside(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    pub fn runtime(&self) -> PathBuf {
        self.root.join(".review-kit")
    }

    pub fn run_path(&self, run_id: &RunId) -> PathBuf {
        self.runtime().join("review-runs").join(run_id.as_str())
    }

    pub fn git(&self, arguments: &[&str]) -> String {
        git_in(&self.root, arguments)
    }

    /// Add a linked worktree beside the main one and return its root.
    pub fn linked_worktree(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        self.git(&["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
        path.canonicalize().unwrap()
    }

    pub fn store(&self) -> RuntimeStore {
        RuntimeStore::open(GitWorktree::locate(&self.root).unwrap()).unwrap()
    }
}

/// Run git with configuration isolated from the host and require success.
pub fn git_in(directory: &Path, arguments: &[&str]) -> String {
    let output = git_output(directory, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn git_output(directory: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .args([
            "-c",
            "user.name=Review Kit",
            "-c",
            "user.email=rkc@example.test",
        ])
        .args(arguments)
        .current_dir(directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
}

pub fn run_id(number: u64) -> RunId {
    RunId::try_from(format!("00000000-0000-4000-8000-{number:012}")).unwrap()
}

/// A stand-in for `review-workspace.json`; the store is generic over its schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub selected_run_id: String,
}

impl Selection {
    pub fn of(run_id: &RunId) -> Self {
        Self {
            selected_run_id: run_id.as_str().into(),
        }
    }
}

pub const SNAPSHOT: [(SnapshotFile, &[u8]); 4] = [
    (SnapshotFile::Context, b"{\"context\": \"revision-1\"}\n"),
    (SnapshotFile::Source, b"{\"source\": \"revision-1\"}\n"),
    (SnapshotFile::Diff, b"diff --git a/lib.rs b/lib.rs\n"),
    (SnapshotFile::Conversation, b"{\"discussions\": []}\n"),
];

pub const MANIFEST: &[u8] = b"{\"manifest\": true}\n";
pub const ASSESSMENT_DRAFT: &[u8] = b"{\"policies\": []}\n";

pub fn expected_snapshot() -> SnapshotFiles {
    let hash = |file: SnapshotFile| {
        let (_, bytes) = SNAPSHOT.iter().find(|(name, _)| *name == file).unwrap();
        content_ref(bytes).unwrap()
    };
    SnapshotFiles {
        context: hash(SnapshotFile::Context),
        source: hash(SnapshotFile::Source),
        diff: hash(SnapshotFile::Diff),
        conversation: hash(SnapshotFile::Conversation),
    }
}

/// Stage every file a run start writes and drop the handle, as an interrupted run start does.
pub fn stage_without_installing(lock: &WorkspaceLock, run_id: &RunId) {
    let staged = lock.stage_run(run_id.clone()).unwrap();
    for (file, bytes) in SNAPSHOT {
        staged.write(file.into(), bytes).unwrap();
    }
    staged
        .write(RunFile::Working(WorkingFile::Draft), ASSESSMENT_DRAFT)
        .unwrap();
    staged
        .write(WorkingFile::Manifest.into(), MANIFEST)
        .unwrap();
}

pub fn install_run<'lock>(lock: &'lock WorkspaceLock, run_id: &RunId) -> RunWriter<'lock> {
    let staged = lock.stage_run(run_id.clone()).unwrap();
    for (file, bytes) in SNAPSHOT {
        staged.write(file.into(), bytes).unwrap();
    }
    staged
        .write(WorkingFile::Manifest.into(), MANIFEST)
        .unwrap();
    staged.install().unwrap()
}

/// Names in `directory`, sorted, after a successful listing.
pub fn entries(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}
