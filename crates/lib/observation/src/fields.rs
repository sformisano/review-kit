use std::collections::BTreeMap;

use redactable::{Secret, Sensitive};
use serde::Serialize;

/// Explicit disclosure for operation facts. Keys and labels are authored constants.
/// Dynamic text must use `secret`; arbitrary JSON and business objects are not accepted.
#[derive(Clone, Default, Serialize, Sensitive)]
#[serde(transparent)]
pub struct Fields(BTreeMap<&'static str, Value>);

#[derive(Clone, Serialize, Sensitive)]
#[serde(untagged)]
enum Value {
    Number(#[not_sensitive] u64),
    Flag(#[not_sensitive] bool),
    Label(#[not_sensitive] &'static str),
    Secret(#[sensitive(Secret)] String),
    Object(Fields),
    Error(Box<ErrorInfo>),
    List(Vec<Value>),
    Null,
}

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn number(mut self, key: &'static str, value: u64) -> Self {
        self.0.insert(key, Value::Number(value));
        self
    }

    pub fn count(self, key: &'static str, value: usize) -> Self {
        self.number(key, value as u64)
    }

    pub fn flag(mut self, key: &'static str, value: bool) -> Self {
        self.0.insert(key, Value::Flag(value));
        self
    }

    pub fn label(mut self, key: &'static str, value: &'static str) -> Self {
        self.0.insert(key, Value::Label(value));
        self
    }

    pub fn secret(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.0.insert(key, Value::Secret(value.into()));
        self
    }

    pub fn object(mut self, key: &'static str, value: Self) -> Self {
        self.0.insert(key, Value::Object(value));
        self
    }

    pub fn error(mut self, key: &'static str, value: ErrorInfo) -> Self {
        self.0.insert(key, Value::Error(Box::new(value)));
        self
    }

    pub fn objects(mut self, key: &'static str, values: impl IntoIterator<Item = Self>) -> Self {
        self.0.insert(
            key,
            Value::List(values.into_iter().map(Value::Object).collect()),
        );
        self
    }

    pub fn labels(
        mut self,
        key: &'static str,
        values: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.0.insert(
            key,
            Value::List(values.into_iter().map(Value::Label).collect()),
        );
        self
    }

    pub fn absent(mut self, key: &'static str) -> Self {
        self.0.insert(key, Value::Null);
        self
    }
}

/// Error categories and reviewed causes, without formatting the original error.
#[derive(Clone, Serialize, Sensitive)]
pub struct ErrorInfo {
    #[not_sensitive]
    r#type: &'static str,
    #[not_sensitive]
    variant: &'static str,
    fields: Fields,
    cause: Option<Box<ErrorInfo>>,
    #[not_sensitive]
    opaque: bool,
}

impl ErrorInfo {
    pub fn new(r#type: &'static str, variant: &'static str) -> Self {
        Self {
            r#type,
            variant,
            fields: Fields::new(),
            cause: None,
            opaque: false,
        }
    }

    /// Hides the complete foreign cause, including its message and source chain.
    pub fn opaque(r#type: &'static str) -> Self {
        Self {
            opaque: true,
            ..Self::new(r#type, "opaque")
        }
    }

    pub fn with_fields(mut self, fields: Fields) -> Self {
        self.fields = fields;
        self
    }

    pub fn caused_by(mut self, cause: Self) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }
}
