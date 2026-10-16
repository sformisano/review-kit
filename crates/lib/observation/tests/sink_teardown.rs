use std::{
    io::{self, Write},
    panic::{catch_unwind, resume_unwind},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use review_kit_observation::{Completion, Fields, Observer, observe, with_observer};

struct DropPanicWriter(Arc<AtomicUsize>);

impl Write for DropPanicWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for DropPanicWriter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        resume_unwind(Box::new("private writer destructor"));
    }
}

#[test]
fn owned_sink_teardown_preserves_the_result_and_original_business_panic() {
    let drops = Arc::new(AtomicUsize::new(0));
    let returned = catch_unwind(|| {
        with_observer(&Observer::json(DropPanicWriter(drops.clone())), || {
            observe(
                "sink.success",
                Fields::new(),
                || 42,
                |_| Completion::success(Fields::new()),
            )
        })
    });
    assert_eq!(returned.ok(), Some(42));
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    let unwound = catch_unwind(|| {
        with_observer(&Observer::json(DropPanicWriter(drops.clone())), || {
            observe(
                "sink.business_panic",
                Fields::new(),
                || resume_unwind(Box::new("original business panic")),
                |_| Completion::success(Fields::new()),
            )
        })
    });
    assert_eq!(
        unwound
            .err()
            .and_then(|payload| payload.downcast::<&str>().ok())
            .as_deref(),
        Some(&"original business panic")
    );
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}
