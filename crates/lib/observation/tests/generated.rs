use std::{
    cell::Cell,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use review_kit_observation::{
    Completion, ErrorInfo, Fields, Observer, observed_port, operations, with_observer,
};
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

mod lexical {
    #[review_kit_observation::operations]
    #[operation(
        name = "generated.function",
        input = super::Fields::new().secret("value", value),
        output = |result| super::Completion::result(result,
            |value| super::Fields::new().count("bytes", value.len()),
            |_| super::ErrorInfo::opaque("io"))
    )]
    pub fn borrowed<'a>(value: &'a str, reject: bool) -> Result<&'a str, super::io::Error> {
        if reject {
            return Err(super::io::Error::other("private standalone error"));
        }
        Ok(value)
    }
}

fn success<T>(_: &T) -> Completion {
    Completion::success(Fields::new())
}

fn port_input() -> Fields {
    Fields::new().number("source", 1)
}

fn io_result<T>(result: &Result<T, io::Error>) -> Completion {
    Completion::result(result, |_| Fields::new(), |_| ErrorInfo::opaque("io"))
}

fn failed_input() -> Fields {
    std::panic::resume_unwind(Box::new("private input projection"))
}

fn failed_output() -> fn(&u64) -> Completion {
    std::panic::resume_unwind(Box::new("private output projection"))
}

#[operations]
mod actions {
    use super::*;

    #[operation(name = "generated.child", input = Fields::new().count("before", values.len()), output = success)]
    pub fn child(values: &mut Vec<String>) -> &str {
        values.push("private child value".into());
        values.last().map(String::as_str).unwrap_or("")
    }

    #[operation(name = "generated.parent", input = Fields::new().secret("value", value.as_ref()), output = success)]
    pub fn parent<T: AsRef<str>>(values: &mut Vec<String>, value: T) -> usize {
        child(values);
        values.push(value.as_ref().to_owned());
        values.len()
    }

    #[operation(name = "generated.borrowed", input = Fields::new().secret("value", value), output = success)]
    pub fn borrowed<'a>(value: &'a str, _unrelated: &str) -> &'a str {
        value
    }

    #[operation(name = "generated.early", input = Fields::new(), output = io_result)]
    pub fn early(value: Result<u64, io::Error>) -> Result<u64, io::Error> {
        let value = value?;
        if value == 0 {
            return Ok(1);
        }
        Ok(value)
    }

    #[operation(name = "generated.failed_input", input = failed_input(), output = success)]
    pub fn input_failure(calls: &mut usize) -> u64 {
        *calls += 1;
        7
    }

    #[operation(name = "generated.failed_output", input = Fields::new(), output = failed_output())]
    pub fn output_failure() -> u64 {
        8
    }

    #[helper]
    pub fn ordinary(value: u64) -> u64 {
        value
    }

    #[operation(
        name = "generated.counted",
        input = { projections.set(projections.get() + 1); Fields::new() },
        output = success
    )]
    pub fn counted(projections: &Cell<usize>, calls: &mut usize) -> usize {
        *calls += 1;
        *calls
    }

    #[operation(
        name = "generated.hygiene",
        input = Fields::new().secret("value", __review_kit_input.as_str()),
        output = |value: &String| Completion::success(Fields::new()
            .number("authored_parameter", __review_kit_result)
            .secret("value", value.as_str()))
    )]
    pub fn collision(__review_kit_input: String, __review_kit_result: u64) -> String {
        __review_kit_input
    }
}

struct Counter(usize);

#[operations]
impl Counter {
    #[operation(name = "generated.increment", input = Fields::new().count("before", self.0), output = success)]
    fn increment(&mut self) -> &usize {
        self.0 += 1;
        &self.0
    }
}

#[observed_port(wrapper = Reader, ownership = "borrowed")]
trait RawReader {
    #[operation(name = "port.read", input = crate::port_input().label("path_text", "super::literal"), output = self::success)]
    #[operation_alias(method = readback, name = "port.readback", input = Fields::new(), output = success)]
    fn read(&mut self) -> usize;

    #[operation(name = "port.default", input = Fields::new(), output = success)]
    fn default_read(&mut self) -> usize {
        self.read()
    }
}

#[observed_port(wrapper = Session, ownership = "boxed", reader = Reader)]
trait RawSession: RawReader {
    #[helper]
    fn label(&self) -> &str;

    #[operation(name = "port.write", input = Fields::new().count("value", value), output = success)]
    fn write(&mut self, value: usize);
}

#[observed_port(wrapper = Host, ownership = "borrowed")]
trait RawHost {
    #[operation(
        name = "port.acquire", input = Fields::new(), output = success,
        returns = Session<'_>, map = Session::new
    )]
    fn acquire(&mut self) -> Box<dyn RawSession + '_>;
}

struct Provider {
    value: usize,
}
struct BorrowedSession<'a>(&'a mut usize);

impl RawReader for BorrowedSession<'_> {
    fn read(&mut self) -> usize {
        *self.0
    }
}
impl RawSession for BorrowedSession<'_> {
    fn label(&self) -> &str {
        "private provider label"
    }
    fn write(&mut self, value: usize) {
        *self.0 = value;
    }
}
impl RawHost for Provider {
    fn acquire(&mut self) -> Box<dyn RawSession + '_> {
        Box::new(BorrowedSession(&mut self.value))
    }
}

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
        String::from_utf8(bytes)?
            .lines()
            .map(|line| Ok(serde_json::from_str::<Value>(line)?["execution"].clone()))
            .collect()
    }
}

#[test]
fn function_declarations_preserve_lexical_paths_and_infer_result_policies() -> TestResult {
    let capture = Capture::default();
    let value = String::from("private standalone value");
    with_observer(&Observer::json(capture.clone()), || -> TestResult {
        let borrowed = lexical::borrowed(&value, false)?;
        assert!(std::ptr::eq(borrowed.as_ptr(), value.as_ptr()));
        assert_eq!(
            lexical::borrowed(&value, true)
                .err()
                .ok_or("missing error")?
                .to_string(),
            "private standalone error"
        );
        Ok(())
    })?;
    let records = capture.records()?;
    assert_eq!(records.len(), 4);
    assert_eq!(records[1]["output"]["bytes"], value.len());
    assert_eq!(records[3]["outcome"], "failure");
    assert!(!serde_json::to_string(&records)?.contains("private"));
    Ok(())
}

#[test]
fn generated_entries_preserve_generic_borrowed_and_early_results() -> TestResult {
    let capture = Capture::default();
    let mut values = Vec::new();
    let text = "private borrowed value".to_owned();
    let mut counter = Counter(0);
    with_observer(&Observer::json(capture.clone()), || -> TestResult {
        assert_eq!(
            actions::parent(&mut values, String::from("private parent value")),
            2
        );
        assert_eq!(actions::borrowed(&text, "other"), text);
        assert_eq!(actions::early(Ok(0))?, 1);
        assert_eq!(
            actions::early(Err(io::Error::other("private error")))
                .err()
                .ok_or("missing error")?
                .to_string(),
            "private error"
        );
        assert_eq!(*counter.increment(), 1);
        assert_eq!(actions::ordinary(3), 3);
        assert_eq!(
            actions::collision("private collision value".into(), 43),
            "private collision value"
        );
        Ok(())
    })?;
    let records = capture.records()?;
    assert_eq!(records.len(), 14);
    assert_eq!(
        records[1]["parent_execution_id"],
        records[0]["execution_id"]
    );
    assert_eq!(records[1]["input"]["before"], 0);
    assert!(records.iter().any(
        |record| record["operation"] == "generated.increment" && record["input"]["before"] == 0
    ));
    assert!(!serde_json::to_string(&records)?.contains("private"));
    assert!(
        records
            .iter()
            .any(|record| record["operation"] == "generated.hygiene"
                && record["output"]["authored_parameter"] == 43)
    );
    assert_eq!(values.len(), 2);
    Ok(())
}

#[test]
fn projection_failures_do_not_replace_or_skip_business_execution() -> TestResult {
    let capture = Capture::default();
    let mut calls = 0;
    with_observer(&Observer::json(capture.clone()), || {
        assert_eq!(actions::input_failure(&mut calls), 7);
        assert_eq!(actions::output_failure(), 8);
    });
    assert_eq!(calls, 1);
    let records = capture.records()?;
    assert_eq!(
        records[0]["input"]["observation_input"],
        "projection_failed"
    );
    assert_eq!(records[1]["outcome"], "success");
    assert_eq!(records[3]["outcome"], "unclassified");
    assert!(!serde_json::to_string(&records)?.contains("private"));
    assert_eq!(actions::input_failure(&mut calls), 7);
    assert_eq!(calls, 2);
    let projections = Cell::new(0);
    let mut counted_calls = 0;
    with_observer(&Observer::json(capture), || {
        assert_eq!(actions::counted(&projections, &mut counted_calls), 1);
    });
    assert_eq!(projections.get(), 1);
    assert_eq!(actions::counted(&projections, &mut counted_calls), 2);
    assert_eq!(projections.get(), 1);
    Ok(())
}

#[test]
fn managed_acquisition_keeps_borrowed_state_defaults_and_readback_observed() -> TestResult {
    let capture = Capture::default();
    let mut provider = Provider { value: 0 };
    with_observer(&Observer::json(capture.clone()), || {
        let mut host = Host::new(&mut provider);
        let mut session = host.acquire();
        assert_eq!(session.label(), "private provider label");
        session.write(9);
        assert_eq!(session.reader().read(), 9);
        assert_eq!(session.reader().default_read(), 9);
        assert_eq!(session.reader().readback(), 9);
    });
    assert_eq!(provider.value, 9);
    let records = capture.records()?;
    let starts: Vec<_> = records
        .iter()
        .filter(|record| record["phase"] == "started")
        .map(|record| record["operation"].as_str())
        .collect();
    assert_eq!(
        starts,
        [
            Some("port.acquire"),
            Some("port.write"),
            Some("port.read"),
            Some("port.default"),
            Some("port.readback")
        ]
    );
    assert_eq!(records.len(), 10);
    let read = records
        .iter()
        .find(|r| r["operation"] == "port.read" && r["phase"] == "started")
        .ok_or("missing read")?;
    assert_eq!(read["input"]["source"], 1);
    assert_eq!(read["input"]["path_text"], "super::literal");
    assert!(!serde_json::to_string(&records)?.contains("private"));
    Ok(())
}
