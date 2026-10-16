use super::{Directory, fixture};
use review_kit_domain::ReviewerId;
use review_kit_filesystem_adapter::{FileError, FileProgressStore};

#[test]
fn lock_survives_progress_replacement_and_preserves_first_import() {
    let directory = Directory::new();
    let workspace = directory.workspace();
    let (snapshot, _, _) = fixture();
    let store = FileProgressStore::new(
        directory.0.path().join("shared"),
        &snapshot.subject,
        &snapshot.reviewer,
    );
    let guard = store.try_lock().unwrap();
    let progress = store.load(&workspace).unwrap();
    store.save(&progress).unwrap();
    assert!(matches!(
        store.clone().try_lock(),
        Err(FileError::PublicationBusy)
    ));

    let other_reviewer = ReviewerId::try_from("another-reviewer".to_owned()).unwrap();
    let other = FileProgressStore::new(store.root(), &snapshot.subject, &other_reviewer);
    let _independent = other.try_lock().unwrap();
    drop(guard);
    let _reopened = store.try_lock().unwrap();
    assert_eq!(store.load(&workspace).unwrap(), progress);
}

#[cfg(unix)]
mod process {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        path::Path,
        process::{Child, Command, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };

    struct Worker(Child);

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "invoked as a subprocess by lock_is_released_after_process_termination"]
    fn lock_worker() {
        let root = std::env::var_os("RKC_LOCK_TEST_ROOT").unwrap();
        let (snapshot, _, _) = fixture();
        let store = FileProgressStore::new(Path::new(&root), &snapshot.subject, &snapshot.reviewer);
        let _guard = store.try_lock().unwrap();
        println!("lock acquired");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[test]
    fn lock_is_released_after_process_termination() {
        let directory = Directory::new();
        let (snapshot, _, _) = fixture();
        let root = directory.0.path().join("shared");
        let mut worker = Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "locking::process::lock_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RKC_LOCK_TEST_ROOT", &root)
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
        let store = FileProgressStore::new(&root, &snapshot.subject, &snapshot.reviewer);
        assert!(matches!(store.try_lock(), Err(FileError::PublicationBusy)));
        worker.0.kill().unwrap();
        let status = worker.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
        let _guard = store.try_lock().unwrap();
        store.load(&directory.workspace()).unwrap();
    }
}
