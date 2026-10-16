use crate::observation;
use review_kit_observation::Fields;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use json2md::View;
use review_kit_application::ports::RenderProfileIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{RendererError, projection::InputContract};

pub const MANIFEST_VERSION: &str = "review-kit-render-profile/v1";
pub const MANIFEST_FILENAME: &str = "profile.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: String,
    id: String,
    /// Parsed by `parse_manifest`, which reports an unsupported value as an unsupported contract.
    input_contract: String,
    artifacts: Artifacts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifacts {
    finding: AssetPaths,
    summary: AssetPaths,
    transition: AssetPaths,
    answer: AssetPaths,
    suggestion: AssetPaths,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetPaths {
    schema: PathBuf,
    template: PathBuf,
}

#[derive(Clone, Copy)]
pub(crate) enum Role {
    Finding,
    Summary,
    Transition,
    Answer,
    Suggestion,
}

#[review_kit_observation::operations]
impl Role {
    const ALL: [Self; 5] = [
        Self::Finding,
        Self::Summary,
        Self::Transition,
        Self::Answer,
        Self::Suggestion,
    ];

    #[helper]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Finding => "finding",
            Self::Summary => "summary",
            Self::Transition => "transition",
            Self::Answer => "answer",
            Self::Suggestion => "suggestion",
        }
    }

    #[helper]
    fn paths(self, manifest: &Manifest) -> &AssetPaths {
        match self {
            Self::Finding => &manifest.artifacts.finding,
            Self::Summary => &manifest.artifacts.summary,
            Self::Transition => &manifest.artifacts.transition,
            Self::Answer => &manifest.artifacts.answer,
            Self::Suggestion => &manifest.artifacts.suggestion,
        }
    }

    #[helper]
    fn embedded(self) -> (&'static str, &'static str) {
        match self {
            Self::Finding => (
                include_str!("../assets/finding.schema.json"),
                include_str!("../assets/finding.j2"),
            ),
            Self::Summary => (
                include_str!("../assets/summary.schema.json"),
                include_str!("../assets/summary.j2"),
            ),
            Self::Transition => (
                include_str!("../assets/transition.schema.json"),
                include_str!("../assets/transition.j2"),
            ),
            Self::Answer => (
                include_str!("../assets/answer.schema.json"),
                include_str!("../assets/answer.j2"),
            ),
            Self::Suggestion => (
                include_str!("../assets/suggestion.schema.json"),
                include_str!("../assets/suggestion.j2"),
            ),
        }
    }
}

struct Asset {
    role: Role,
    schema_path: PathBuf,
    template_path: PathBuf,
    schema: String,
    template: String,
}

pub(crate) struct CompiledAsset {
    pub(crate) view: View,
    pub(crate) schema_path: PathBuf,
    pub(crate) template_path: PathBuf,
    pub(crate) role: Role,
}

pub(crate) struct Profile {
    pub(crate) identity: RenderProfileIdentity,
    pub(crate) contract: InputContract,
    pub(crate) finding: CompiledAsset,
    pub(crate) summary: CompiledAsset,
    pub(crate) transition: CompiledAsset,
    pub(crate) answer: CompiledAsset,
    pub(crate) suggestion: CompiledAsset,
}

#[review_kit_observation::operations]
impl Profile {
    #[helper]
    pub(crate) fn embedded() -> Result<Self, RendererError> {
        let (manifest, contract) = parse_manifest(
            include_str!("../assets/profile.json"),
            Path::new("embedded/profile.json"),
        )?;
        let assets = Role::ALL
            .into_iter()
            .map(|role| {
                let paths = role.paths(&manifest);
                let (schema, template) = role.embedded();
                Asset {
                    role,
                    schema_path: Path::new("embedded").join(&paths.schema),
                    template_path: Path::new("embedded").join(&paths.template),
                    schema: schema.into(),
                    template: template.into(),
                }
            })
            .collect();
        Self::compile(manifest, contract, assets)
    }

    #[helper]
    pub(crate) fn load(path: &Path) -> Result<Self, RendererError> {
        let manifest_text = read(path)?;
        let (manifest, contract) = parse_manifest(&manifest_text, path)?;
        let root = path.parent().unwrap_or(Path::new("."));
        let mut assets = Vec::new();
        for role in Role::ALL {
            let paths = role.paths(&manifest);
            let schema_path = root.join(&paths.schema);
            let template_path = root.join(&paths.template);
            assets.push(Asset {
                role,
                schema: read(&schema_path)?,
                template: read(&template_path)?,
                schema_path,
                template_path,
            });
        }
        // Rendering owns these bytes; changes during loading must not mix profiles.
        if read(path)? != manifest_text {
            return Err(RendererError::ChangedDuringLoad(path.to_owned()));
        }
        for asset in &assets {
            for (path, expected) in [
                (&asset.schema_path, &asset.schema),
                (&asset.template_path, &asset.template),
            ] {
                if &read(path)? != expected {
                    return Err(RendererError::ChangedDuringLoad(path.clone()));
                }
            }
        }
        Self::compile(manifest, contract, assets)
    }

    #[operation(
        name = "renderer.compile_profile",
        input = Fields::new().count("assets", assets.len()),
        output = |result| observation::result(result, |_| Fields::new()),
    )]
    fn compile(
        manifest: Manifest,
        contract: InputContract,
        assets: Vec<Asset>,
    ) -> Result<Self, RendererError> {
        let canonical =
            serde_json::to_vec(&manifest).map_err(|source| RendererError::ManifestJson {
                path: PathBuf::from(MANIFEST_FILENAME),
                source,
            })?;
        let mut hash = Sha256::new();
        hash_part(&mut hash, &canonical);
        let mut compiled = Vec::new();
        for asset in assets {
            hash_part(&mut hash, asset.role.name().as_bytes());
            hash_part(&mut hash, asset.schema.as_bytes());
            hash_part(&mut hash, asset.template.as_bytes());
            let schema = serde_json::from_str(&asset.schema).map_err(|source| {
                RendererError::SchemaJson {
                    role: asset.role.name(),
                    path: asset.schema_path.clone(),
                    source,
                }
            })?;
            let view = View::new(&schema, &asset.template).map_err(|source| {
                RendererError::asset(
                    asset.role.name(),
                    &asset.schema_path,
                    &asset.template_path,
                    source,
                )
            })?;
            compiled.push(CompiledAsset {
                view,
                schema_path: asset.schema_path,
                template_path: asset.template_path,
                role: asset.role,
            });
        }
        let mut compiled = compiled.into_iter();
        let mut next = || compiled.next().ok_or(RendererError::MissingAsset);
        Ok(Self {
            identity: RenderProfileIdentity {
                id: manifest.id,
                input_contract: manifest.input_contract,
                fingerprint: format!("sha256:{:x}", hash.finalize()),
            },
            contract,
            finding: next()?,
            summary: next()?,
            transition: next()?,
            answer: next()?,
            suggestion: next()?,
        })
    }
}

#[review_kit_observation::operations]
#[helper]
fn hash_part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

#[review_kit_observation::operations]
#[helper]
fn read(path: &Path) -> Result<String, RendererError> {
    fs::read_to_string(path).map_err(|source| RendererError::Read {
        path: path.to_owned(),
        source,
    })
}

#[review_kit_observation::operations]
#[helper]
fn parse_manifest(text: &str, path: &Path) -> Result<(Manifest, InputContract), RendererError> {
    let manifest: Manifest =
        serde_json::from_str(text).map_err(|source| RendererError::ManifestJson {
            path: path.to_owned(),
            source,
        })?;
    if manifest.version != MANIFEST_VERSION {
        return Err(RendererError::UnsupportedManifest {
            path: path.to_owned(),
            version: manifest.version,
        });
    }
    let Some(contract) = InputContract::parse(&manifest.input_contract) else {
        return Err(RendererError::UnsupportedContract {
            path: path.to_owned(),
            contract: manifest.input_contract,
        });
    };
    if manifest.id.is_empty()
        || manifest.id.len() > 128
        || !manifest
            .id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_' | b'.'))
        || matches!(manifest.id.as_str(), "." | "..")
    {
        return Err(RendererError::InvalidProfileId(path.to_owned()));
    }
    for role in Role::ALL {
        let paths = role.paths(&manifest);
        for path in [&paths.schema, &paths.template] {
            if path.as_os_str().is_empty() || path.is_absolute() {
                return Err(RendererError::AssetPath {
                    role: role.name(),
                    path: path.clone(),
                });
            }
        }
    }
    Ok((manifest, contract))
}

#[review_kit_observation::operations]
#[helper]
pub(crate) fn export(directory: &Path) -> Result<PathBuf, RendererError> {
    let (manifest, _) = parse_manifest(
        include_str!("../assets/profile.json"),
        Path::new("embedded/profile.json"),
    )?;
    let mut files = vec![(
        directory.join(MANIFEST_FILENAME),
        include_str!("../assets/profile.json"),
    )];
    for role in Role::ALL {
        let paths = role.paths(&manifest);
        let (schema, template) = role.embedded();
        files.push((directory.join(&paths.schema), schema));
        files.push((directory.join(&paths.template), template));
    }
    for (path, _) in &files {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(RendererError::ExportExists(path.clone())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(RendererError::ExportIo {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    fs::create_dir_all(directory).map_err(|source| RendererError::ExportIo {
        path: directory.to_owned(),
        source,
    })?;
    let mut created = Vec::new();
    for (path, text) in files {
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|source| RendererError::ExportIo {
                    path: path.clone(),
                    source,
                })?;
            created.push(path.clone());
            file.write_all(text.as_bytes())
                .map_err(|source| RendererError::ExportIo {
                    path: path.clone(),
                    source,
                })
        })();
        if let Err(error) = result {
            for path in &created {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
    }
    Ok(directory.join(MANIFEST_FILENAME))
}
