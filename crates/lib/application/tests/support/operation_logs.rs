use std::{
    io,
    sync::{Arc, Mutex},
};

use review_kit_observation::Observer;
use serde_json::Value;

#[derive(Clone, Default)]
pub struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Capture {
    pub fn observer(&self) -> Observer {
        Observer::json(self.clone())
    }

    pub fn raw(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    pub fn records(&self) -> Vec<Value> {
        self.raw()
            .lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                value.get_mut("execution").unwrap().take()
            })
            .collect()
    }
}

pub fn completed<'a>(records: &'a [Value], operation: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["operation"] == operation && record["phase"] == "completed")
        .unwrap()
}
