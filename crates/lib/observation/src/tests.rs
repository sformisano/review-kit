use std::{
    cell::Cell,
    io,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::Value;

use super::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("capture poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn records(&self) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        let bytes = self.0.lock().map_err(|_| "capture poisoned")?.clone();
        Ok(String::from_utf8(bytes)?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()?
            .into_iter()
            .map(|v| v["execution"].clone())
            .collect())
    }
}

fn success<T>(_: &T) -> Completion {
    Completion::success(Fields::new())
}

#[test]
fn typed_listener_runs_before_the_sink_and_keeps_business_results() {
    struct Sink(Arc<AtomicUsize>);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(self.0.load(Ordering::SeqCst) > 0);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let observer = Observer::json(Sink(count.clone())).with_listener(move |event| {
        match event {
            OperationEvent::Started { operation } => assert_eq!(operation, "test.listener"),
            OperationEvent::Completed { operation, outcome } => {
                assert_eq!(operation, "test.listener");
                assert!(matches!(outcome, Outcome::Success));
            }
        }
        seen.fetch_add(1, Ordering::SeqCst);
    });
    let value = with_observer(&observer, || {
        observe(
            "test.listener",
            Fields::new().secret("body", "private"),
            || 17,
            success,
        )
    });
    assert_eq!(value, 17);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn panicking_listener_does_not_change_results_or_json_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let capture = Capture::default();
    let observer = Observer::json(capture.clone()).with_listener(|_| {
        std::panic::resume_unwind(Box::new("private listener payload"));
    });
    let value = with_observer(&observer, || {
        observe("test.listener", Fields::new(), || 17, success)
    });
    assert_eq!(value, 17);
    assert_eq!(capture.records()?.len(), 2);
    Ok(())
}

#[test]
fn replacing_a_listener_contains_its_capture_destructor_panic()
-> Result<(), Box<dyn std::error::Error>> {
    struct PanickingDrop;
    impl Drop for PanickingDrop {
        fn drop(&mut self) {
            std::panic::resume_unwind(Box::new("private capture destructor"));
        }
    }
    let capture = Capture::default();
    let retained = PanickingDrop;
    let observer = Observer::json(capture.clone()).with_listener(move |_| {
        let _ = &retained;
    });
    let observer = observer.with_listener(|_| {});
    let value = with_observer(&observer, || {
        observe("test.listener", Fields::new(), || 17, success)
    });
    assert_eq!(value, 17);
    assert_eq!(capture.records()?.len(), 2);
    Ok(())
}

#[test]
fn actual_json_preserves_structure_and_original_values() -> Result<(), Box<dyn std::error::Error>> {
    let capture = Capture::default();
    let observer = Observer::json(capture.clone());
    let secret = "private-token-url-path-review-秘密";
    let returned = with_observer(&observer, || {
        observe(
            "test.parent",
            Fields::new()
                .secret("credential", secret)
                .object("settings", Fields::new().flag("enabled", true))
                .objects("items", [Fields::new().number("n", 7).absent("optional")]),
            || {
                observe(
                    "test.child",
                    Fields::new(),
                    || secret.to_owned(),
                    |value| {
                        Completion::success(
                            Fields::new()
                                .secret("body", value.clone())
                                .count("bytes", value.len()),
                        )
                    },
                )
            },
            success,
        )
    });
    assert_eq!(returned, secret);
    let records = capture.records()?;
    assert_eq!(records.len(), 4);
    assert!(records[0]["parent_execution_id"].is_null());
    assert_eq!(
        records[1]["parent_execution_id"],
        records[0]["execution_id"]
    );
    assert_eq!(records[1]["execution_id"], records[2]["execution_id"]);
    assert_eq!(records[0]["execution_id"], records[3]["execution_id"]);
    assert_eq!(records[0]["input"]["credential"], "[REDACTED]");
    assert_eq!(records[0]["input"]["settings"]["enabled"], true);
    assert_eq!(records[0]["input"]["items"][0]["n"], 7);
    assert!(records[0]["input"]["items"][0]["optional"].is_null());
    assert!(records[3]["duration_us"].is_u64());
    assert!(!serde_json::to_string(&records)?.contains(secret));
    Ok(())
}

#[test]
fn errors_are_structured_without_reading_foreign_messages() -> Result<(), Box<dyn std::error::Error>>
{
    let capture = Capture::default();
    let result: Result<(), io::Error> = with_observer(&Observer::json(capture.clone()), || {
        observe(
            "test.failure",
            Fields::new(),
            || Err(io::Error::other("SECRET error chain")),
            |result| {
                Completion::result(
                    result,
                    |_| Fields::new(),
                    |_| ErrorInfo::new("load", "read").caused_by(ErrorInfo::opaque("io")),
                )
            },
        )
    });
    assert_eq!(
        result.err().ok_or("missing error")?.to_string(),
        "SECRET error chain"
    );
    let records = capture.records()?;
    assert_eq!(records[1]["error"]["cause"]["opaque"], true);
    assert_eq!(records[1]["outcome"], "failure");
    assert!(!serde_json::to_string(&records)?.contains("SECRET"));
    Ok(())
}

#[test]
fn unwind_projection_failure_and_replacement_restore_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let outer = Capture::default();
    let inner = Capture::default();
    let observer = Observer::json(outer.clone());
    with_observer(&observer, || {
        observe(
            "root",
            Fields::new(),
            || {
                let panicked = catch_unwind(AssertUnwindSafe(|| {
                    observe(
                        "panic",
                        Fields::new(),
                        || {
                            std::panic::resume_unwind(Box::new("private panic payload"));
                        },
                        success,
                    )
                }));
                assert!(panicked.is_err());
                let value = observe(
                    "projection",
                    Fields::new(),
                    || 42,
                    |_| {
                        std::panic::resume_unwind(Box::new("private projection payload"));
                    },
                );
                assert_eq!(value, 42);
                with_observer(&Observer::json(inner.clone()), || {
                    observe("replacement", Fields::new(), || (), success)
                });
                observe("sibling", Fields::new(), || (), success);
            },
            success,
        )
    });
    let records = outer.records()?;
    assert_eq!(records[2]["outcome"], "panicked");
    assert_eq!(records[4]["outcome"], "unclassified");
    assert_eq!(
        records[5]["parent_execution_id"],
        records[0]["execution_id"]
    );
    assert!(inner.records()?[0]["parent_execution_id"].is_null());
    with_observer(&observer, || observe("next", Fields::new(), || (), success));
    assert!(outer.records()?[8]["parent_execution_id"].is_null());
    Ok(())
}

struct BrokenWriter {
    flush: bool,
}
impl Write for BrokenWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.flush {
            Ok(bytes.len())
        } else {
            Err(io::Error::other("SECRET writer failure"))
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("SECRET flush failure"))
    }
}

#[test]
fn failed_write_and_flush_leave_business_result_unchanged() {
    for flush in [false, true] {
        let observer = Observer::json(BrokenWriter { flush });
        let result = with_observer(&observer, || observe("test", Fields::new(), || 23, success));
        assert_eq!(result, 23);
    }
}

#[derive(Default)]
struct ReentrantCalls {
    bodies: AtomicUsize,
    projections: AtomicUsize,
    results: Mutex<Vec<usize>>,
}

struct ReentrantWriter(Arc<ReentrantCalls>);
impl Write for ReentrantWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let returned = observe(
            "writer.recursive",
            Fields::new(),
            || {
                self.0.bodies.fetch_add(1, Ordering::SeqCst);
                17
            },
            |_| {
                self.0.projections.fetch_add(1, Ordering::SeqCst);
                success(&())
            },
        );
        self.0
            .results
            .lock()
            .map_err(|_| io::Error::other("recursive result capture poisoned"))?
            .push(returned);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct PanickingWriter;
impl Write for PanickingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        std::panic::resume_unwind(Box::new("private writer panic"));
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn recursive_and_panicking_writers_preserve_results_and_later_scopes()
-> Result<(), Box<dyn std::error::Error>> {
    let recursive = Arc::new(ReentrantCalls::default());
    for observer in [
        Observer::json(ReentrantWriter(recursive.clone())),
        Observer::json(PanickingWriter),
    ] {
        assert_eq!(
            with_observer(&observer, || observe("test", Fields::new(), || 13, success)),
            13
        );
    }
    let results = recursive.results.lock().map_err(|_| "capture poisoned")?;
    assert!(!results.is_empty());
    assert!(results.iter().all(|&result| result == 17));
    assert_eq!(recursive.bodies.load(Ordering::SeqCst), results.len());
    assert_eq!(recursive.projections.load(Ordering::SeqCst), 0);
    let capture = Capture::default();
    with_observer(&Observer::json(capture.clone()), || {
        observe("after", Fields::new(), || (), success)
    });
    assert!(capture.records()?[0]["parent_execution_id"].is_null());
    assert_eq!(capture.records()?.len(), 2);
    Ok(())
}

#[test]
fn disabled_observation_does_not_invoke_projection() {
    let projections = Cell::new(0);
    assert_eq!(
        observe(
            "disabled",
            Fields::new(),
            || 17,
            |_| {
                projections.set(projections.get() + 1);
                success(&())
            }
        ),
        17
    );
    assert_eq!(projections.get(), 0);
}

#[test]
fn threads_require_explicit_scopes() -> Result<(), Box<dyn std::error::Error>> {
    let capture = Capture::default();
    let observer = Observer::json(capture.clone());
    with_observer(&observer, || {
        observe(
            "parent",
            Fields::new(),
            || {
                let other = observer.clone();
                std::thread::spawn(move || {
                    observe("disabled", Fields::new(), || (), success);
                    with_observer(&other, || {
                        observe("thread_root", Fields::new(), || (), success)
                    });
                })
                .join()
                .map_err(|_| "thread panicked")
            },
            success,
        )
    })?;
    let records = capture.records()?;
    assert_eq!(records.len(), 4);
    assert_eq!(records[1]["operation"], "thread_root");
    assert!(records[1]["parent_execution_id"].is_null());
    Ok(())
}

#[derive(Clone, Sensitive)]
struct BrokenProjection {
    #[not_sensitive]
    value: u64,
}
impl Serialize for BrokenProjection {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("SECRET serialization failure"))
    }
}

#[test]
fn serializer_failure_emits_only_fixed_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let capture = Capture::default();
    Observer::json(capture.clone()).emit_projected(&BrokenProjection { value: 1 }, None);
    let records = capture.records()?;
    assert_eq!(records, [Value::String("[REDACTED]".into())]);
    Ok(())
}
