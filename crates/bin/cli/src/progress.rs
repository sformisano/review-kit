//! Stderr progress uses fixed stage labels and owns its timer until command completion.

use std::{
    io::{self, IsTerminal, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use review_kit_observation::{Observer, OperationEvent};

const WAIT_INTERVAL: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Mode {
    Auto,
    Plain,
    None,
}

#[review_kit_observation::operations]
impl Mode {
    #[helper]
    fn terminal(self, stderr_is_terminal: bool) -> bool {
        matches!(self, Self::Auto) && stderr_is_terminal
    }
}

enum Update {
    Stage(&'static str),
    Finish,
}

pub struct Progress {
    sender: SyncSender<Update>,
    worker: Worker,
}

enum Worker {
    Pending(Receiver<Update>),
    Running(JoinHandle<()>),
    Disabled,
}

#[review_kit_observation::operations]
impl Progress {
    #[helper]
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(32);
        Self {
            sender,
            worker: Worker::Pending(receiver),
        }
    }

    #[helper]
    pub fn start(&mut self, mode: Mode) {
        let receiver = match std::mem::replace(&mut self.worker, Worker::Disabled) {
            Worker::Pending(receiver) => receiver,
            worker => {
                self.worker = worker;
                return;
            }
        };
        if matches!(mode, Mode::None) {
            return;
        }
        let terminal = mode.terminal(io::stderr().is_terminal());
        let worker = thread::Builder::new()
            .name("rkc-progress".into())
            .spawn(move || run(receiver, io::stderr(), terminal));
        if let Ok(worker) = worker {
            self.worker = Worker::Running(worker);
        }
    }

    #[helper]
    pub fn attach(&self, observer: Observer) -> Observer {
        let sender = self.sender.clone();
        observer.with_listener(move |event| {
            if let OperationEvent::Started { operation } = event
                && let Some(label) = stage(operation)
            {
                // Progress may drop queued stages; it never delays an operation's dispatch.
                let _ = sender.try_send(Update::Stage(label));
            }
        })
    }

    #[helper]
    pub fn finish(mut self) {
        self.stop();
    }

    #[helper]
    fn stop(&mut self) {
        if let Worker::Running(worker) = std::mem::replace(&mut self.worker, Worker::Disabled) {
            let _ = self.sender.send(Update::Finish);
            let _ = worker.join();
        }
    }
}

#[review_kit_observation::operations]
impl Drop for Progress {
    #[helper]
    fn drop(&mut self) {
        self.stop();
    }
}

struct Cadence {
    label: &'static str,
    next_wait: Duration,
}

#[review_kit_observation::operations]
impl Cadence {
    #[helper]
    fn new() -> Self {
        Self {
            label: "Starting command",
            next_wait: WAIT_INTERVAL,
        }
    }

    #[helper]
    fn stage(&mut self, label: &'static str, now: Duration) -> Option<String> {
        if self.label == label {
            return None;
        }
        self.next_wait = now + WAIT_INTERVAL;
        self.label = label;
        Some(format!("rkc: {label}\n"))
    }

    #[helper]
    fn waiting(&mut self, now: Duration) -> Option<String> {
        if now < self.next_wait {
            return None;
        }
        self.next_wait = now + WAIT_INTERVAL;
        Some(format!("rkc: Still working: {}\n", self.label))
    }

    #[helper]
    fn message(
        &mut self,
        label: Option<&'static str>,
        now: Duration,
        terminal: bool,
    ) -> Option<String> {
        label
            .and_then(|label| self.stage(label, now))
            .or_else(|| if terminal { None } else { self.waiting(now) })
    }

    #[helper]
    fn spinner(&self, frame: usize) -> String {
        let glyph = ["|", "/", "-", "\\"][frame % 4];
        format!("\r\x1b[2Krkc: {glyph} {}", self.label)
    }
}

#[review_kit_observation::operations]
#[helper]
fn run(receiver: Receiver<Update>, mut writer: impl Write, terminal: bool) {
    let started = Instant::now();
    let mut cadence = Cadence::new();
    let mut frame = 0;
    let mut next_frame = TICK;
    if write(&mut writer, "rkc: Starting command\n").is_err() {
        return;
    }
    loop {
        let update = receiver.recv_timeout(TICK);
        let label = match update {
            Ok(Update::Stage(label)) => Some(label),
            Ok(Update::Finish) | Err(RecvTimeoutError::Disconnected) => {
                let line = if terminal {
                    "\r\x1b[2Krkc: Command finished\n"
                } else {
                    "rkc: Command finished\n"
                };
                let _ = write(&mut writer, line);
                return;
            }
            Err(RecvTimeoutError::Timeout) => None,
        };
        let line = cadence.message(label, started.elapsed(), terminal);
        let line = line.or_else(|| {
            let now = started.elapsed();
            if terminal {
                if now < next_frame {
                    return None;
                }
                next_frame = now + TICK;
                frame += 1;
                Some(cadence.spinner(frame))
            } else {
                None
            }
        });
        if let Some(line) = line {
            let line = if terminal && line.ends_with('\n') {
                format!("\r\x1b[2K{}", line.trim_end())
            } else {
                line
            };
            if write(&mut writer, &line).is_err() {
                return;
            }
        }
    }
}

#[review_kit_observation::operations]
#[helper]
fn write(writer: &mut impl Write, line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

#[review_kit_observation::operations]
#[helper]
fn stage(operation: &str) -> Option<&'static str> {
    Some(match operation {
        "cli.discover" => "Discovering review",
        "cli.init" => "Installing review run",
        "cli.run.start" => "Starting review run",
        "cli.run.list" => "Listing review runs",
        "cli.run.select" => "Selecting review run",
        "cli.run.finish" => "Finishing review run",
        "cli.run.stop" => "Stopping review run",
        "cli.status" => "Reading review status",
        "cli.check" => "Checking assessment draft",
        "cli.accept" => "Accepting assessment draft",
        "cli.revise" => "Revising assessment draft",
        "cli.publish" | "cli.publish.run" => "Preparing publication",
        "gitlab.capture_source" => "Capturing committed source",
        "gitlab.capture_conversation" => "Capturing conversation",
        "filesystem.open_journal" => "Opening publication journal",
        "cli.abandon" => "Closing review round",
        "cli.render_profile.export" => "Exporting rendering profile",
        "config.load" => "Reading configuration",
        "config.connect" => "Connecting to GitLab",
        "input.load_assessment" => "Loading assessment",
        "input.load_draft_context" => "Loading retained review",
        "input.load_source" => "Loading retained source",
        "application.discovery" | "application.provider.discover" => "Reading current review",
        "application.source.changes" => "Capturing source changes",
        "application.validation.snapshot" | "application.validation.findings" => {
            "Validating review data"
        }
        "application.publication.reconcile_history" => "Reconciling review history",
        "application.publication.reconcile_progress" => "Reconciling saved progress",
        "application.publication.build_plan" => "Planning review effects",
        "application.publication.publish" => "Publishing review",
        "application.provider.write" => "Sending review effect",
        "application.provider.readback" | "application.publication.verify_effect" => {
            "Verifying review effect"
        }
        "application.evidence.record" => "Saving execution evidence",
        "rendering.load" => "Loading rendering profile",
        "rendering.save_preview" | "rendering.save_run_preview" => "Saving review previews",
        "filesystem.save_result" => "Saving command result",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_lines_even_on_a_terminal() {
        assert!(Mode::Auto.terminal(true));
        assert!(!Mode::Auto.terminal(false));
        assert!(!Mode::Plain.terminal(true));
        assert!(!Mode::None.terminal(true));
    }

    fn parsed_mode(options: &[&str]) -> Result<Mode, clap::Error> {
        use clap::Parser as _;
        let command = ["render-profile", "export", "--dir", "profile"];
        let argv = ["rkc"].iter().chain(options).chain(&command);
        Ok(crate::args::Args::try_parse_from(argv)?.progress)
    }

    #[test]
    fn omitted_option_selects_plain_and_explicit_options_keep_their_modes()
    -> Result<(), clap::Error> {
        let omitted = parsed_mode(&[])?;
        assert!(matches!(omitted, Mode::Plain));
        assert!(!omitted.terminal(true));
        assert!(matches!(parsed_mode(&["--progress=plain"])?, Mode::Plain));
        assert!(parsed_mode(&["--progress=auto"])?.terminal(true));
        assert!(matches!(parsed_mode(&["--progress=none"])?, Mode::None));
        Ok(())
    }

    #[test]
    fn plain_waits_after_silence_and_resets_at_actual_stages() {
        let mut cadence = Cadence::new();
        assert!(cadence.waiting(Duration::from_secs(9)).is_none());
        assert_eq!(
            cadence.waiting(Duration::from_secs(10)).as_deref(),
            Some("rkc: Still working: Starting command\n")
        );
        assert!(cadence.waiting(Duration::from_secs(19)).is_none());
        assert_eq!(
            cadence
                .stage("Reading current review", Duration::from_secs(19))
                .as_deref(),
            Some("rkc: Reading current review\n")
        );
        assert!(cadence.waiting(Duration::from_secs(28)).is_none());
        assert_eq!(
            cadence.waiting(Duration::from_secs(29)).as_deref(),
            Some("rkc: Still working: Reading current review\n")
        );
    }

    #[test]
    fn repeated_stage_keeps_waiting_cadence_without_duplicate_line() {
        let mut cadence = Cadence::new();
        for tick in 0..1000 {
            assert!(
                cadence
                    .message(
                        Some("Starting command"),
                        Duration::from_millis(tick * 10),
                        false
                    )
                    .is_none()
            );
        }
        assert_eq!(
            cadence
                .message(Some("Starting command"), Duration::from_secs(10), false)
                .as_deref(),
            Some("rkc: Still working: Starting command\n")
        );
        assert!(
            cadence
                .message(Some("Starting command"), Duration::from_secs(18), false)
                .is_none()
        );
    }

    #[test]
    fn failed_sink_ends_worker_without_blocking_sender() {
        struct Failing;
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("failed sink"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || run(receiver, Failing, false));
        assert!(worker.join().is_ok());
        assert!(sender.send(Update::Finish).is_err());
    }

    #[test]
    fn private_or_unknown_operation_names_have_no_progress_projection() {
        assert!(stage("private subject and token").is_none());
        assert_eq!(
            stage("application.provider.write"),
            Some("Sending review effect")
        );
    }
}
