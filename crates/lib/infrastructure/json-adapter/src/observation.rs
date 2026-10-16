use review_kit_observation::{ErrorInfo, Fields};

use crate::JsonError;

pub(crate) fn json_error(error: &JsonError) -> ErrorInfo {
    match error {
        JsonError::Syntax(source) => ErrorInfo::new("json", "syntax").caused_by(
            ErrorInfo::new(
                "serde_json",
                match source.classify() {
                    serde_json::error::Category::Io => "io",
                    serde_json::error::Category::Syntax => "syntax",
                    serde_json::error::Category::Data => "data",
                    serde_json::error::Category::Eof => "eof",
                },
            )
            .caused_by(ErrorInfo::opaque("serde_source")),
        ),
        JsonError::Version(_) => ErrorInfo::new("json", "version"),
        JsonError::RetainedStateLimit => ErrorInfo::new("json", "retained_state_limit"),
        JsonError::InvalidValue => ErrorInfo::new("json", "invalid_value"),
        JsonError::Finding(_) => {
            ErrorInfo::new("json", "finding").caused_by(ErrorInfo::opaque("finding"))
        }
        JsonError::ContextBinding => ErrorInfo::new("json", "context_binding"),
        JsonError::Discovery(_) => {
            ErrorInfo::new("json", "discovery").caused_by(ErrorInfo::opaque("discovery"))
        }
        JsonError::Preparation(_) => {
            ErrorInfo::new("json", "preparation").caused_by(ErrorInfo::opaque("preparation"))
        }
        JsonError::Policies(_) => ErrorInfo::new("json", "policy_coverage")
            .caused_by(ErrorInfo::opaque("policy_coverage")),
        JsonError::Draft(diagnostics) => ErrorInfo::new("json", "draft")
            .with_fields(Fields::new().count("diagnostics", diagnostics.len())),
    }
}
