use std::{
    cell::{Cell, RefCell},
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use review_kit_observation::{Completion, Fields, Observer, observe, with_observer};

struct PanickingPayload;

impl Drop for PanickingPayload {
    fn drop(&mut self) {
        resume_unwind(Box::new("private payload destructor"));
    }
}

fn logging_panic<T>() -> T {
    resume_unwind(Box::new(PanickingPayload))
}

struct PanickingWriter;

struct TerminalPanicWriter(Arc<AtomicUsize>);

impl Write for TerminalPanicWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
            return logging_panic();
        }
        Ok(())
    }
}

impl Write for PanickingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        logging_panic()
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn success<T>(_: &T) -> Completion {
    Completion::success(Fields::new())
}

#[test]
fn writer_panic_payload_cannot_skip_business_or_replace_its_panic() {
    let calls = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_observer(&Observer::json(PanickingWriter), || {
            observe(
                "writer.failure",
                Fields::new(),
                || {
                    calls.set(calls.get() + 1);
                    42
                },
                success,
            )
        })
    }));
    assert_eq!(result.ok(), Some(42));
    assert_eq!(calls.get(), 1);

    let flushes = Arc::new(AtomicUsize::new(0));
    let result = catch_unwind(|| {
        with_observer(
            &Observer::json(TerminalPanicWriter(flushes.clone())),
            || {
                observe(
                    "business.failure",
                    Fields::new(),
                    || resume_unwind(Box::new("original business panic")),
                    success,
                )
            },
        )
    });
    assert_eq!(
        result
            .err()
            .and_then(|payload| payload.downcast::<&str>().ok())
            .as_deref(),
        Some(&"original business panic")
    );
    assert_eq!(flushes.load(Ordering::SeqCst), 2);
}

#[review_kit_observation::operations]
mod generated {
    use super::*;

    #[operation(name = "input.failure", input = logging_panic(), output = success)]
    pub fn input(calls: &Cell<usize>) -> usize {
        calls.set(calls.get() + 1);
        17
    }

    #[operation(name = "output.failure", input = Fields::new(), output = |_| logging_panic())]
    pub fn output(calls: &Cell<usize>) -> usize {
        calls.set(calls.get() + 1);
        23
    }

    #[operation(name = "input.capture", input = { drop(value); Fields::new() }, output = success)]
    pub(super) fn owned_input(value: PanickingPayload, calls: &Cell<usize>) -> usize {
        calls.set(calls.get() + 1);
        31
    }
}

#[test]
fn unused_projection_captures_cannot_replace_results_or_business_panics() {
    let calls = Cell::new(0);
    let captured = PanickingPayload;
    let result = catch_unwind(AssertUnwindSafe(|| {
        observe(
            "disabled.capture",
            Fields::new(),
            || {
                calls.set(calls.get() + 1);
                42
            },
            move |_| {
                let _keep_capture = &captured;
                success(&())
            },
        )
    }));
    assert_eq!(result.ok(), Some(42));
    assert_eq!(calls.get(), 1);
    assert_eq!(generated::owned_input(PanickingPayload, &calls), 31);
    assert_eq!(calls.get(), 2);

    let captured = PanickingPayload;
    let result = catch_unwind(|| {
        observe(
            "disabled.business_panic",
            Fields::new(),
            || resume_unwind(Box::new("business panic with unused projection")),
            move |_| {
                let _keep_capture = &captured;
                success(&())
            },
        )
    });
    assert_eq!(
        result
            .err()
            .and_then(|p| p.downcast::<&str>().ok())
            .as_deref(),
        Some(&"business panic with unused projection")
    );
}

#[test]
fn projection_panic_payloads_preserve_generated_business_results() {
    let calls = Cell::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_observer(&Observer::json(io::sink()), || {
            (generated::input(&calls), generated::output(&calls))
        })
    }));
    assert_eq!(result.ok(), Some((17, 23)));
    assert_eq!(calls.get(), 2);
}

#[derive(Default)]
struct TeardownResult {
    calls: AtomicUsize,
    projections: AtomicUsize,
    escaped: AtomicBool,
}

struct OnExit {
    result: Arc<TeardownResult>,
    install_scope: bool,
}

impl Drop for OnExit {
    fn drop(&mut self) {
        let call = || {
            observe(
                "thread.teardown",
                Fields::new(),
                || self.result.calls.fetch_add(1, Ordering::SeqCst),
                |_| {
                    self.result.projections.fetch_add(1, Ordering::SeqCst);
                    success(&())
                },
            )
        };
        let returned = catch_unwind(AssertUnwindSafe(|| {
            if self.install_scope {
                with_observer(&Observer::json(io::sink()), call)
            } else {
                call()
            }
        }));
        self.result
            .escaped
            .store(returned.is_err(), Ordering::SeqCst);
    }
}

thread_local! {
    static ON_EXIT: RefCell<Option<OnExit>> = const { RefCell::new(None) };
}

#[test]
fn destroyed_thread_context_skips_logging_and_runs_business_once() {
    for install_scope in [false, true] {
        let result = Arc::new(TeardownResult::default());
        let child = result.clone();
        let worker = std::thread::spawn(move || {
            ON_EXIT.with(|slot| {
                slot.replace(Some(OnExit {
                    result: child,
                    install_scope,
                }));
            });
            // The observer context is destroyed before the earlier business thread-local.
            observe("thread.body", Fields::new(), || (), success);
        });
        assert!(worker.join().is_ok(), "worker thread failed");
        assert!(!result.escaped.load(Ordering::SeqCst));
        assert_eq!(result.calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.projections.load(Ordering::SeqCst), 0);
    }
}
