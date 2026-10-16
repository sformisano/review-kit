use std::sync::{Mutex, MutexGuard};

use review_kit_filesystem_adapter::RunStorageError;

use super::harness::{Repository, Selection, run_id};

static SERIAL: Mutex<()> = Mutex::new(());

/// No other test in this binary starts a process while the caller holds this guard.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Competing commands serialize under the workspace lock; the guard keeps the
// lock until it drops, and the lock file persists afterwards.
#[test]
fn workspace_lock_excludes_a_second_handle_until_dropped() {
    let _serial = serial();
    let repository = Repository::new();
    let first = repository.store();
    let guard = first.create_and_lock().unwrap();
    let second = repository.store();

    assert!(matches!(second.lock(), Err(RunStorageError::WorkspaceBusy)));
    assert!(matches!(
        second.create_and_lock(),
        Err(RunStorageError::WorkspaceBusy)
    ));
    assert!(matches!(first.lock(), Err(RunStorageError::WorkspaceBusy)));

    guard.write_workspace(&Selection::of(&run_id(1))).unwrap();
    drop(guard);
    let lock_file = repository.runtime().join("review-workspace.lock");
    assert!(lock_file.is_file());

    let reacquired = second.lock().unwrap();
    reacquired
        .write_workspace(&Selection::of(&run_id(2)))
        .unwrap();
    assert_eq!(
        first.read_workspace::<Selection>().unwrap(),
        Some(Selection::of(&run_id(2)))
    );
    assert!(lock_file.is_file());
}

#[cfg(unix)]
mod process {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::process::ExitStatusExt,
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };

    use review_kit_filesystem_adapter::{GitWorktree, RunStorageError, RuntimeStore};

    use super::{Repository, serial};

    struct Worker(Child);

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "invoked as a subprocess by workspace_lock_is_released_when_its_process_dies"]
    fn workspace_lock_worker() {
        let root = PathBuf::from(std::env::var_os("RKC_RUNS_LOCK_ROOT").unwrap());
        let store = RuntimeStore::open(GitWorktree::locate(&root).unwrap()).unwrap();
        let _guard = store.create_and_lock().unwrap();
        println!("lock acquired");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    // A crashed command cannot leave the workspace permanently busy.
    #[test]
    fn workspace_lock_is_released_when_its_process_dies() {
        let _serial = serial();
        let repository = Repository::new();
        let mut worker = Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "locking::process::workspace_lock_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RKC_RUNS_LOCK_ROOT", repository.root())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let output = BufReader::new(worker.0.stdout.take().unwrap());
        let (ready, received) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            for line in output.lines() {
                if line.unwrap().contains("lock acquired") {
                    ready.send(()).unwrap();
                    return;
                }
            }
        });
        assert_eq!(
            received.recv_timeout(Duration::from_secs(10)),
            Ok(()),
            "worker did not acquire the lock"
        );
        reader.join().unwrap();
        let store = repository.store();
        assert!(matches!(store.lock(), Err(RunStorageError::WorkspaceBusy)));

        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        drop(store.lock().unwrap());
    }
}
