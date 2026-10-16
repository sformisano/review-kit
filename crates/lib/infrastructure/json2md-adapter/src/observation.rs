use std::error::Error;

use review_kit_application::{ports::PortError, summary::SummaryView};
use review_kit_domain::Finding;
use review_kit_observation::{Completion, ErrorInfo, Fields};

use crate::RendererError;

pub(crate) fn finding(finding: &Finding) -> Fields {
    let fields = Fields::new()
        .secret("content", "finding content")
        .flag("validated", finding.validated())
        .object(
            "location",
            Fields::new().secret("path", finding.location().path().as_str()),
        );
    match finding.suggestion() {
        Some(suggestion) => fields.object(
            "suggestion",
            Fields::new()
                .count("original_lines", suggestion.original().len())
                .count("replacement_lines", suggestion.replacement().len())
                .secret("content", "suggestion content"),
        ),
        None => fields.absent("suggestion"),
    }
}

pub(crate) fn summary(view: &SummaryView) -> Fields {
    Fields::new()
        .secret("content", "summary content")
        .count("history", view.history.len())
        .count("policies", view.policies.len())
        .object(
            "counts",
            Fields::new()
                .count("total", view.counts.total)
                .count("open", view.counts.open)
                .count("fixed", view.counts.fixed)
                .count("withdrawn", view.counts.withdrawn)
                .count("uncertain", view.counts.uncertain),
        )
}

pub(crate) fn error(error: &RendererError) -> ErrorInfo {
    let (variant, cause) = match error {
        RendererError::Read { .. } => ("read", Some("io")),
        RendererError::ManifestJson { .. } => ("manifest_json", Some("json")),
        RendererError::UnsupportedManifest { .. } => ("unsupported_manifest", None),
        RendererError::UnsupportedContract { .. } => ("unsupported_contract", None),
        RendererError::InvalidProfileId(_) => ("invalid_profile_id", None),
        RendererError::AssetPath { .. } => ("asset_path", None),
        RendererError::ChangedDuringLoad(_) => ("changed_during_load", None),
        RendererError::SchemaJson { .. } => ("schema_json", Some("json")),
        RendererError::Rendering { .. } => ("rendering", Some("json2md")),
        RendererError::MissingContext(_) => ("missing_context", None),
        RendererError::MissingAsset => ("missing_asset", None),
        RendererError::ExportExists(_) => ("export_exists", None),
        RendererError::ExportIo { .. } => ("export_io", Some("io")),
        RendererError::Subject => ("subject", None),
        RendererError::Url(_) => ("url", Some("url")),
    };
    let info = ErrorInfo::new("renderer", variant);
    match cause {
        Some(cause) => info.caused_by(ErrorInfo::opaque(cause)),
        None => info,
    }
}

pub(crate) fn port_error(error: &PortError) -> ErrorInfo {
    let info = ErrorInfo::new("port", "renderer");
    match error
        .source()
        .and_then(|cause| cause.downcast_ref::<RendererError>())
    {
        Some(cause) => info.caused_by(self::error(cause)),
        None => info.caused_by(ErrorInfo::opaque("port_source")),
    }
}

pub(crate) fn result<T>(
    result: &Result<T, RendererError>,
    output: impl FnOnce(&T) -> Fields,
) -> Completion {
    Completion::result(result, output, error)
}

pub(crate) fn rendered(result: &Result<String, PortError>) -> Completion {
    Completion::result(
        result,
        |body| {
            Fields::new()
                .count("bytes", body.len())
                .secret("body", "rendered content")
        },
        port_error,
    )
}
