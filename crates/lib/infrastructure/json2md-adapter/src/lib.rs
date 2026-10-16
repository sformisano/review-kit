//! Public review Markdown from an offline snapshot of schemas and templates.
use review_kit_observation::Fields;

mod observation;
mod profile;
mod projection;

use std::{
    error::Error,
    path::{Path, PathBuf},
};

use review_kit_application::{
    ports::{
        AnswerRenderInput, FindingRenderInput, PortError, RenderProfileIdentity, ReviewRenderer,
        TransitionRenderInput,
    },
    summary::SummaryView,
};
use review_kit_domain::{Finding, FindingStatus};
use serde_json::Value;

use profile::{CompiledAsset, Profile};

pub use profile::{MANIFEST_FILENAME, MANIFEST_VERSION};

/// The embedded profile's rendering-input contract, the newest supported contract.
/// A loaded profile's identity names the contract it declares.
pub const INPUT_CONTRACT: &str = projection::InputContract::V2.as_str();

pub struct Json2MdRenderer {
    profile: Profile,
}

#[review_kit_observation::operations]
impl Json2MdRenderer {
    #[operation(
        name = "renderer.load",
        input = Fields::new().label("source", "embedded"),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    pub fn new() -> Result<Self, RendererError> {
        Ok(Self {
            profile: Profile::embedded()?,
        })
    }

    /// Loads and compiles all profile bytes before rendering any body.
    #[operation(
        name = "renderer.load",
        input = Fields::new()
        .label("source", "file")
        .secret("manifest", manifest.to_string_lossy()),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    pub fn from_profile(manifest: &Path) -> Result<Self, RendererError> {
        Ok(Self {
            profile: Profile::load(manifest)?,
        })
    }

    #[helper]
    pub fn identity(&self) -> &RenderProfileIdentity {
        &self.profile.identity
    }

    /// Existing destination files remain unchanged, including symbolic links.
    #[operation(
        name = "renderer.export",
        input = Fields::new().secret("directory", directory.to_string_lossy()),
        output = |result| observation::result(result, |_| Fields::new().flag("exported", true)),
    )]
    pub fn export_default(directory: &Path) -> Result<PathBuf, RendererError> {
        profile::export(directory)
    }

    #[helper]
    fn finding_input(&self, input: FindingRenderInput<'_>) -> Result<Value, PortError> {
        if input.policy_name.trim().is_empty() {
            return Err(PortError::new(
                "project finding",
                RendererError::MissingContext("trusted policy display name"),
            ));
        }
        let contract = self.profile.contract;
        let mut value =
            projection::finding(input.finding, input.policy_name, input.context, contract);
        if let Some(suggestion) = input.finding.suggestion() {
            let structured = projection::suggestion(input.finding.id(), suggestion, contract);
            let body = render(&self.profile.suggestion, &structured)?;
            let mut structured = structured;
            structured["markdown"] = Value::String(body);
            value["suggestion"] = structured;
        }
        Ok(value)
    }
}

#[review_kit_observation::operations]
#[operation(
    name = "renderer.render_asset",
    input = Fields::new()
    .label("role", asset.role.name())
    .secret("input", "rendering data"),
    output = observation::rendered,
)]
fn render(asset: &CompiledAsset, input: &Value) -> Result<String, PortError> {
    asset.view.render(input).map_err(|source| {
        PortError::new(
            "render public review body",
            RendererError::asset(
                asset.role.name(),
                &asset.schema_path,
                &asset.template_path,
                source,
            ),
        )
    })
}

#[review_kit_observation::operations]
impl ReviewRenderer for Json2MdRenderer {
    #[helper]
    fn identity(&self) -> Option<&RenderProfileIdentity> {
        Some(self.identity())
    }

    #[operation(
        name = "renderer.finding",
        input = Fields::new()
        .object("finding", observation::finding(finding))
        .flag("context", false),
        output = observation::rendered,
    )]
    fn finding(&mut self, finding: &Finding) -> Result<String, PortError> {
        Err(PortError::new(
            "render finding",
            RendererError::MissingContext(
                "finding policy name and public round context; use finding_context",
            ),
        ))
    }

    #[operation(
        name = "renderer.finding",
        input = Fields::new()
        .object("finding", observation::finding(input.finding))
        .flag("context", true),
        output = observation::rendered,
    )]
    fn finding_context(&mut self, input: FindingRenderInput<'_>) -> Result<String, PortError> {
        render(&self.profile.finding, &self.finding_input(input)?)
    }

    #[operation(
        name = "renderer.transition",
        input = Fields::new()
        .object("finding", observation::finding(finding))
        .flag("context", false),
        output = observation::rendered,
    )]
    fn transition(
        &mut self,
        finding: &Finding,
        _previous: FindingStatus,
        _explanation: &str,
    ) -> Result<String, PortError> {
        Err(PortError::new(
            "render transition",
            RendererError::MissingContext(
                "transition policy name and public round context; use transition_context",
            ),
        ))
    }

    #[operation(
        name = "renderer.transition",
        input = Fields::new()
        .object("finding", observation::finding(input.finding.finding))
        .flag("context", true),
        output = observation::rendered,
    )]
    fn transition_context(
        &mut self,
        input: TransitionRenderInput<'_>,
    ) -> Result<String, PortError> {
        let finding = self.finding_input(input.finding)?;
        render(
            &self.profile.transition,
            &projection::transition(input, finding, self.profile.contract),
        )
    }

    #[operation(
        name = "renderer.answer",
        input = Fields::new().secret("input", "rendering data"),
        output = observation::rendered,
    )]
    fn answer(&mut self, _body: &str) -> Result<String, PortError> {
        Err(PortError::new(
            "render answer",
            RendererError::MissingContext(
                "verified answer target and public round context; use answer_context",
            ),
        ))
    }

    #[operation(
        name = "renderer.answer",
        input = Fields::new()
        .object("finding", observation::finding(input.current_finding))
        .secret("answer", "answer content"),
        output = observation::rendered,
    )]
    fn answer_context(&mut self, input: AnswerRenderInput<'_>) -> Result<String, PortError> {
        render(
            &self.profile.answer,
            &projection::answer(input, self.profile.contract),
        )
    }

    #[operation(
        name = "renderer.summary",
        input = observation::summary(view),
        output = observation::rendered,
    )]
    fn summary(&mut self, view: &SummaryView) -> Result<String, PortError> {
        let input = projection::summary(view, self.profile.contract)
            .map_err(|source| PortError::new("project summary", source))?;
        render(&self.profile.summary, &input)
    }
}

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum RendererError {
    /// Cannot read rendering asset {path:?}: {source}. Check its path and UTF-8 contents.
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Invalid profile manifest {path:?}: {source}. Supply the versioned manifest and all five artifact roles.
    ManifestJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// Unsupported profile manifest version {version} in {path:?}; expected review-kit-render-profile/v1.
    UnsupportedManifest { path: PathBuf, version: String },
    /// Unsupported rendering input contract {contract} in {path:?}; expected review-kit-render-input/v1 or review-kit-render-input/v2.
    UnsupportedContract { path: PathBuf, contract: String },
    /// Profile {0:?} requires a public id of 1-128 ASCII letters, digits, dots, underscores or hyphens; paths and private context are forbidden.
    InvalidProfileId(PathBuf),
    /// Artifact {role} requires a nonempty asset path relative to the manifest; invalid path {path:?}.
    AssetPath { role: &'static str, path: PathBuf },
    /// Rendering asset {0:?} changed while loading; retry after finishing profile edits.
    ChangedDuringLoad(PathBuf),
    /// Invalid {role} schema JSON in {path:?}: {source}.
    SchemaJson {
        role: &'static str,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// Cannot compile or render {role} using schema {schema_path:?} and template {template_path:?}: {detail}.
    Rendering {
        role: &'static str,
        schema_path: PathBuf,
        template_path: PathBuf,
        detail: String,
        #[source]
        source: json2md::Error,
    },
    /// Missing required rendering data: {0}.
    MissingContext(&'static str),
    /// Rendering profile is missing a required compiled artifact.
    MissingAsset,
    /// Export destination {0:?} already exists; choose an empty destination or preserve it under another name.
    ExportExists(PathBuf),
    /// Cannot export rendering profile file {path:?}: {source}; existing files remain unchanged.
    ExportIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Summary requires a valid GitLab subject and root note identity.
    Subject,
    /// Summary subject host URL is invalid: {0}.
    Url(#[source] url::ParseError),
}

#[review_kit_observation::operations]
impl RendererError {
    #[helper]
    fn asset(role: &'static str, schema: &Path, template: &Path, source: json2md::Error) -> Self {
        let detail = match &source {
            json2md::Error::Validation(errors) => errors
                .issues()
                .iter()
                .map(|issue| {
                    format!(
                        "input {} (schema {}): {issue}",
                        if issue.instance_path().is_empty() {
                            "/"
                        } else {
                            issue.instance_path()
                        },
                        issue.schema_path()
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
            error => {
                let mut messages = vec![error.to_string()];
                let mut cause = error.source();
                while let Some(error) = cause {
                    messages.push(format!("{error:#}"));
                    cause = error.source();
                }
                messages.join(": ")
            }
        };
        Self::Rendering {
            role,
            schema_path: schema.to_owned(),
            template_path: template.to_owned(),
            detail,
            source,
        }
    }
}
