#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub account: String,
    /// Signed balance change in cents.
    pub change: i64,
}

#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn record(&mut self, account: &str, change: i64) {
        self.entries.push(AuditEntry {
            account: account.to_owned(),
            change,
        });
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }
}
