use std::collections::BTreeMap;

const BASE: &str = "../../../../../tests/cli/fixtures/base";
pub const AUDIT_ORIGINAL: &str = "    fs::write(audit, \"saved\").map_err(SaveError::Retryable)?;";
pub const AUDIT_REPLACEMENT: &str = "    let _ = fs::write(audit, \"saved\");";
pub const QUANTITY_DERIVE: &str =
    "#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]";

pub fn base_files() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (
            "Cargo.toml",
            include_str!("../../../../../tests/cli/fixtures/base/Cargo.toml").to_owned(),
        ),
        (
            "Cargo.lock",
            include_str!("../../../../../tests/cli/fixtures/base/Cargo.lock").to_owned(),
        ),
        (
            "README.md",
            include_str!("../../../../../tests/cli/fixtures/base/README.md").to_owned(),
        ),
        (
            "src/lib.rs",
            include_str!("../../../../../tests/cli/fixtures/base/src/lib.rs").to_owned(),
        ),
        (
            "src/quantity.rs",
            include_str!("../../../../../tests/cli/fixtures/base/src/quantity.rs").to_owned(),
        ),
        (
            "src/storage.rs",
            include_str!("../../../../../tests/cli/fixtures/base/src/storage.rs").to_owned(),
        ),
    ])
}

pub fn quantity_input() -> String {
    let source = base_files()
        .remove("src/quantity.rs")
        .expect(BASE)
        .replace(
            "use serde::Serialize;",
            "use serde::{Deserialize, Serialize};",
        )
        .replace(
            "PartialEq, Eq, Serialize)",
            "PartialEq, Eq, Serialize, Deserialize)",
        );
    source
        + r##"
#[cfg(test)]
mod json_input_tests {
    use super::*;
    #[test]
    fn reads_positive_quantity() {
        let quantity: Quantity = serde_json::from_str(r#"{"value": 3}"#).unwrap();
        assert_eq!(quantity.get(), 3);
    }
}
"##
}

pub fn quantity_fixed() -> String {
    quantity_input().replacen(
        "pub struct Quantity {",
        "#[serde(try_from = \"WireQuantity\")]\npub struct Quantity {",
        1,
    ) + r##"
#[derive(Deserialize)]
struct WireQuantity {
    value: u32,
}

impl TryFrom<WireQuantity> for Quantity {
    type Error = QuantityError;

    fn try_from(value: WireQuantity) -> Result<Self, Self::Error> {
        Self::new(value.value)
    }
}

#[cfg(test)]
mod json_boundary_tests {
    use super::*;
    #[test]
    fn rejects_zero_quantity_input() {
        assert!(serde_json::from_str::<Quantity>(r#"{"value": 0}"#).is_err());
    }
}
"##
}

pub fn quantity_clean() -> String {
    base_files().remove("src/quantity.rs").expect(BASE)
        .replace("    pub fn get(self)", "    pub fn checked_double(self) -> Option<Self> {\n        self.value.checked_mul(2).and_then(|value| Self::new(value).ok())\n    }\n\n    pub fn get(self)")
        .replace("    #[test]\n    fn constructor", "    #[test]\n    fn checked_double_keeps_positive_values_and_detects_overflow() {\n        assert_eq!(Quantity::new(3).unwrap().checked_double().unwrap().get(), 6);\n        assert!(Quantity::new(u32::MAX).unwrap().checked_double().is_none());\n    }\n\n    #[test]\n    fn constructor")
}

pub fn storage_audit() -> String {
    base_files()
        .remove("src/storage.rs")
        .expect(BASE)
        .replace("_audit: &Path", "audit: &Path")
        .replacen(
            "    Ok(())\n}",
            &format!("{AUDIT_ORIGINAL}\n    Ok(())\n}}"),
            1,
        )
}

pub fn storage_fixed() -> String {
    storage_audit().replace(AUDIT_ORIGINAL, r#"    // Existing audit destinations can alias the primary file.
    let _ = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(audit)
        .and_then(|mut file| io::Write::write_all(&mut file, b"saved"));"#)
        .replace("    #[test]\n    fn saves_primary_text()", r#"    #[test]
    fn audit_at_primary_path_preserves_saved_data() {
        let directory = Directory::new();
        let primary = directory.0.join("data.txt");
        save(&primary, &primary, "three items").unwrap();
        assert_eq!(fs::read_to_string(primary).unwrap(), "three items");
    }

    #[test]
    fn new_audit_file_records_successful_save() {
        let directory = Directory::new();
        let primary = directory.0.join("data.txt");
        let audit = directory.0.join("audit.txt");
        save(&primary, &audit, "three items").unwrap();
        assert_eq!(fs::read_to_string(primary).unwrap(), "three items");
        assert_eq!(fs::read_to_string(audit).unwrap(), "saved");
    }

    #[test]
    fn audit_failure_preserves_successful_save() {
        let directory = Directory::new();
        let primary = directory.0.join("data.txt");
        let audit = directory.0.join("missing").join("audit.txt");
        save(&primary, &audit, "three items").unwrap();
        assert_eq!(fs::read_to_string(primary).unwrap(), "three items");
    }

    #[test]
    fn unavailable_primary_has_no_saved_data() {
        let directory = Directory::new();
        let primary = directory.0.join("missing").join("data.txt");
        assert!(matches!(save(&primary, &directory.0.join("audit.txt"), "three items"), Err(SaveError::Retryable(_))));
        assert!(!primary.exists());
    }

    #[test]
    fn saves_primary_text()"#)
}

pub fn line_of(source: &str, text: &str) -> usize {
    let matches: Vec<_> = source
        .lines()
        .enumerate()
        .filter(|(_, line)| *line == text)
        .collect();
    assert_eq!(matches.len(), 1, "expected one fixture source line");
    matches[0].0 + 1
}
