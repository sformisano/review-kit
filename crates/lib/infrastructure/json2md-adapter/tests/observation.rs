mod support;

use std::{
    fs,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use review_kit_application::ports::{FindingRenderInput, ReviewRenderer};
use review_kit_json2md_adapter::Json2MdRenderer;
use review_kit_observation::{Observer, with_observer};
use serde_json::Value;

use support::{TestResult, context, finding, render_finding};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("test log poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn text(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(String::from_utf8(
            self.0.lock().map_err(|_| "test log poisoned")?.clone(),
        )?)
    }
    fn records(&self) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        self.text()?
            .lines()
            .map(|line| Ok(serde_json::from_str::<Value>(line)?["execution"].clone()))
            .collect()
    }
}

#[test]
fn profile_and_nested_suggestion_rendering_emit_safe_json() -> TestResult {
    let log = Log::default();
    with_observer(&Observer::json(log.clone()), || -> TestResult {
        let mut renderer = Json2MdRenderer::new()?;
        let finding = finding(Some(vec!["private-replacement-token".into()]))?;
        assert!(render_finding(&mut renderer, &finding)?.contains("private-replacement-token"));
        Ok(())
    })?;
    let text = log.text()?;
    for secret in [
        "private-replacement-token",
        "src/lib.rs",
        "Rust Error Contracts",
        "The result disappears",
        "Reproduced with an unavailable peer",
    ] {
        assert!(!text.contains(secret));
    }
    let records = log.records()?;
    let load = records
        .iter()
        .find(|record| record["operation"] == "renderer.load" && record["phase"] == "completed")
        .ok_or("load record missing")?;
    let compile = records
        .iter()
        .find(|record| {
            record["operation"] == "renderer.compile_profile" && record["phase"] == "completed"
        })
        .ok_or("compile record missing")?;
    assert_eq!(compile["parent_execution_id"], load["execution_id"]);
    let compile_start = records
        .iter()
        .find(|record| {
            record["execution_id"] == compile["execution_id"] && record["phase"] == "started"
        })
        .ok_or("compile start missing")?;
    assert!(compile_start["input"]["assets"].is_u64());
    let finding = records
        .iter()
        .find(|record| record["operation"] == "renderer.finding" && record["phase"] == "completed")
        .ok_or("finding record missing")?;
    let finding_start = records
        .iter()
        .find(|record| {
            record["execution_id"] == finding["execution_id"] && record["phase"] == "started"
        })
        .ok_or("finding start missing")?;
    assert_eq!(finding_start["input"]["finding"]["validated"], true);
    assert_eq!(
        finding_start["input"]["finding"]["suggestion"]["original_lines"],
        3
    );
    assert_eq!(
        finding_start["input"]["finding"]["suggestion"]["replacement_lines"],
        1
    );
    assert_eq!(
        finding_start["input"]["finding"]["location"]["path"],
        "[REDACTED]"
    );
    let roles: Vec<_> = records
        .iter()
        .filter(|record| {
            record["operation"] == "renderer.render_asset" && record["phase"] == "completed"
        })
        .collect();
    assert_eq!(roles.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record["operation"] == "renderer.render_asset"
                && record["input"]["role"] == "suggestion")
    );
    assert!(
        records
            .iter()
            .any(|record| record["operation"] == "renderer.render_asset"
                && record["input"]["role"] == "finding")
    );
    for role in roles {
        assert_eq!(role["parent_execution_id"], finding["execution_id"]);
        assert!(role["output"]["bytes"].is_u64());
        assert_eq!(role["outcome"], "success");
    }
    Ok(())
}

#[test]
fn profile_and_template_error_chains_exclude_local_paths_and_diagnostic_values() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("private-profile-path");
    let manifest = Json2MdRenderer::export_default(&path)?;
    fs::write(path.join("finding.j2"), "{{ private_template_variable }}")?;
    let log = Log::default();
    with_observer(&Observer::json(log.clone()), || -> TestResult {
        let mut renderer = Json2MdRenderer::from_profile(&manifest)?;
        let finding = finding(None)?;
        let context = context()?;
        let error = renderer
            .finding_context(FindingRenderInput {
                finding: &finding,
                policy_name: "private-policy-name",
                context: &context,
            })
            .err()
            .ok_or("render unexpectedly succeeded")?;
        let diagnostic = std::error::Error::source(&error)
            .ok_or("renderer cause missing")?
            .to_string();
        assert!(diagnostic.contains("private_template_variable"));
        assert!(Json2MdRenderer::from_profile(&path.join("private-missing-manifest")).is_err());
        Ok(())
    })?;
    let text = log.text()?;
    for secret in [
        "private-profile-path",
        "private_template_variable",
        "private-missing-manifest",
        "private-policy-name",
    ] {
        assert!(!text.contains(secret));
    }
    assert!(!text.contains(directory.path().to_string_lossy().as_ref()));
    let records = log.records()?;
    let failed_render = records
        .iter()
        .find(|record| {
            record["operation"] == "renderer.render_asset" && record["phase"] == "completed"
        })
        .ok_or("render record missing")?;
    assert_eq!(failed_render["outcome"], "failure");
    assert_eq!(failed_render["error"]["cause"]["variant"], "rendering");
    assert_eq!(failed_render["error"]["cause"]["cause"]["opaque"], true);
    assert!(failed_render["error"]["cause"]["cause"]["cause"].is_null());
    assert!(records.iter().any(
        |record| record["operation"] == "renderer.load" && record["error"]["variant"] == "read"
    ));
    Ok(())
}
