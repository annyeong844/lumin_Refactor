use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigObservation, ConfigSyntax, ConfigValue, Limitation, LogicalSourceId,
    PackageIdentityState, RepoPath, RepositoryRootIdentity, ResolutionProfile,
    ResolutionProfileSource, RootedPathRelation, SelectedResolutionProfile, SemanticConfigSnapshot,
    SourceKind, SourceSnapshot,
};

use crate::generated_config_policy::{self, FieldClassification};
use crate::{ConfigDemand, ResolverError};

#[derive(Clone, Debug)]
pub(crate) struct ImporterSettings {
    pub profile: ResolutionProfile,
    pub allow_extensionless: bool,
    pub static_condition: PackageConditionMode,
    pub base_url: Option<RepoPath>,
    pub paths: Option<PathMappings>,
    pub blocked: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageConditionMode {
    Import,
    Require,
}

#[derive(Clone, Debug)]
pub(crate) struct PathMappings {
    pub base: RepoPath,
    pub entries: Vec<PathMapping>,
}

#[derive(Clone, Debug)]
pub(crate) struct PathMapping {
    pub pattern: String,
    pub targets: Vec<String>,
    pub source_order: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ConfigSelection {
    pub settings: BTreeMap<LogicalSourceId, ImporterSettings>,
    pub profiles: Vec<SelectedResolutionProfile>,
    pub limitations: Vec<Limitation>,
    pub demands: Vec<ConfigDemand>,
}

#[derive(Clone, Debug, Default)]
struct EffectiveConfig {
    module_resolution: Option<(ResolutionProfile, RepoPath)>,
    module: Option<String>,
    base_url: Option<RepoPath>,
    paths: Option<PathMappings>,
    blocked: bool,
}

enum ExtendsSelection {
    Selected(RepoPath),
    NeedsInput,
    Blocked,
}

pub(crate) fn select(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    override_profile: Option<ResolutionProfile>,
) -> Result<ConfigSelection, ResolverError> {
    let mut selection = ConfigSelection::default();
    for source in sources {
        select_importer(
            source,
            config,
            repository_root,
            override_profile,
            &mut selection,
        )?;
    }
    selection
        .profiles
        .sort_by(|left, right| left.source_id.cmp(&right.source_id));
    selection.demands.sort();
    selection.demands.dedup();
    Ok(selection)
}

fn select_importer(
    source: &SourceSnapshot,
    config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    override_profile: Option<ResolutionProfile>,
    selection: &mut ConfigSelection,
) -> Result<(), ResolverError> {
    let controlling = nearest_config(&source.path, config);
    let mut visiting = BTreeSet::new();
    let effective = match controlling {
        Some(ref path) => evaluate_config(
            path,
            config,
            repository_root,
            &mut visiting,
            &mut selection.demands,
            &mut selection.limitations,
        )?,
        None => EffectiveConfig::default(),
    };
    let (profile, profile_source) = if let Some(profile) = override_profile {
        (profile, ResolutionProfileSource::Invocation)
    } else if let Some((profile, path)) = &effective.module_resolution {
        (
            *profile,
            ResolutionProfileSource::Config {
                path_canonical: path.canonical_bytes().to_vec(),
                path_display: path.display_escaped(),
            },
        )
    } else {
        (
            ResolutionProfile::Bundler,
            ResolutionProfileSource::ProductDefault,
        )
    };
    let mut blocked = effective.blocked;
    if !module_is_compatible(profile, effective.module.as_deref()) {
        blocked = true;
        selection
            .limitations
            .push(Limitation::TsconfigSemanticsUnsupported {
                path: controlling
                    .as_ref()
                    .map_or_else(|| source.path.display_escaped(), RepoPath::display_escaped),
                detail: format!(
                    "module value {:?} is incompatible with {}",
                    effective.module,
                    profile.as_str()
                ),
            });
    }
    let (allow_extensionless, static_condition) = match profile {
        ResolutionProfile::Bundler => (true, PackageConditionMode::Import),
        ResolutionProfile::Node => (true, PackageConditionMode::Require),
        ResolutionProfile::Node16 | ResolutionProfile::NodeNext => {
            match importer_is_esm(source, config, &mut selection.limitations) {
                Ok(true) => (false, PackageConditionMode::Import),
                Ok(false) => (true, PackageConditionMode::Require),
                Err(()) => {
                    blocked = true;
                    (false, PackageConditionMode::Import)
                }
            }
        }
    };
    selection.profiles.push(SelectedResolutionProfile {
        source_id: source.id.clone(),
        profile,
        source: profile_source,
    });
    selection.settings.insert(
        source.id.clone(),
        ImporterSettings {
            profile,
            allow_extensionless,
            static_condition,
            base_url: effective.base_url,
            paths: effective.paths,
            blocked,
        },
    );
    Ok(())
}

fn nearest_config(path: &RepoPath, config: &SemanticConfigSnapshot) -> Option<RepoPath> {
    config
        .observations
        .keys()
        .filter(|candidate| {
            matches!(
                candidate.file_name_portable(),
                Some("tsconfig.json" | "jsconfig.json")
            ) && candidate
                .parent()
                .is_some_and(|parent| path.is_within(&parent))
        })
        .max_by(|left, right| {
            left.components_len()
                .cmp(&right.components_len())
                .then_with(|| {
                    let left_ts = left.file_name_portable() == Some("tsconfig.json");
                    let right_ts = right.file_name_portable() == Some("tsconfig.json");
                    left_ts.cmp(&right_ts)
                })
        })
        .cloned()
}

fn evaluate_config(
    path: &RepoPath,
    config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    visiting: &mut BTreeSet<RepoPath>,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<EffectiveConfig, ResolverError> {
    if !visiting.insert(path.clone()) {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: path.display_escaped(),
            detail: "config extends cycle".to_owned(),
        });
        return Ok(EffectiveConfig {
            blocked: true,
            ..EffectiveConfig::default()
        });
    }
    let Some(observation) = config.observations.get(path) else {
        demands.push(ConfigDemand {
            path: path.clone(),
            syntax: ConfigSyntax::Jsonc,
        });
        visiting.remove(path);
        return Ok(EffectiveConfig {
            blocked: true,
            ..EffectiveConfig::default()
        });
    };
    let document = match observation {
        ConfigObservation::Present { document, .. } => document,
        ConfigObservation::Missing { .. } | ConfigObservation::NonRegular { .. } => {
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: path.display_escaped(),
                detail: "selected config does not exist as a regular file".to_owned(),
            });
            visiting.remove(path);
            return Ok(EffectiveConfig {
                blocked: true,
                ..EffectiveConfig::default()
            });
        }
        ConfigObservation::Unreadable { .. } => {
            visiting.remove(path);
            return Ok(EffectiveConfig {
                blocked: true,
                ..EffectiveConfig::default()
            });
        }
    };
    let Some(root) = document.root.as_object() else {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: path.display_escaped(),
            detail: "config root must be an object".to_owned(),
        });
        visiting.remove(path);
        return Ok(EffectiveConfig {
            blocked: true,
            ..EffectiveConfig::default()
        });
    };

    let mut effective = EffectiveConfig::default();
    if let Some(extends) = document.root.get("extends") {
        match select_extends(path, extends, config, repository_root, demands, limitations)? {
            ExtendsSelection::Selected(parent) => {
                effective = evaluate_config(
                    &parent,
                    config,
                    repository_root,
                    visiting,
                    demands,
                    limitations,
                )?;
            }
            ExtendsSelection::NeedsInput | ExtendsSelection::Blocked => {
                effective.blocked = true;
            }
        }
    }
    validate_top_level(root, path, limitations, &mut effective);
    if let Some(compiler_options) = document.root.get("compilerOptions") {
        apply_compiler_options(
            compiler_options,
            path,
            repository_root,
            limitations,
            &mut effective,
        )?;
    }
    visiting.remove(path);
    Ok(effective)
}

fn select_extends(
    config_path: &RepoPath,
    value: &ConfigValue,
    config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<ExtendsSelection, ResolverError> {
    let Some(specifier) = value.as_str() else {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: config_path.display_escaped(),
            detail: "extends must be one string".to_owned(),
        });
        return Ok(ExtendsSelection::Blocked);
    };
    if specifier.is_empty() || specifier.contains('\0') {
        return Err(ResolverError::Configuration(format!(
            "extends must be a nonempty NUL-free string: {}",
            config_path.display_escaped()
        )));
    }
    let normalized = specifier.replace('\\', "/");
    match repository_root.classify_rooted_utf8_path(&normalized) {
        RootedPathRelation::Outside => {
            return Err(ResolverError::Configuration(
                "extends path escapes the repository root".to_owned(),
            ));
        }
        RootedPathRelation::Contained => {
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: config_path.display_escaped(),
                detail: "rooted extends specifier is unsupported".to_owned(),
            });
            return Ok(ExtendsSelection::Blocked);
        }
        RootedPathRelation::NotRooted => {}
    }
    if normalized.starts_with("./") || normalized.starts_with("../") {
        return select_relative_extends(
            config_path,
            specifier,
            &normalized,
            config,
            demands,
            limitations,
        );
    }
    select_workspace_extends(
        config_path,
        specifier,
        &normalized,
        config,
        demands,
        limitations,
    )
}

fn select_relative_extends(
    config_path: &RepoPath,
    original_specifier: &str,
    normalized_specifier: &str,
    config: &SemanticConfigSnapshot,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<ExtendsSelection, ResolverError> {
    if normalized_specifier.split('/').any(str::is_empty) {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: config_path.display_escaped(),
            detail: format!("relative extends contains an empty component: {original_specifier}"),
        });
        return Ok(ExtendsSelection::Blocked);
    }
    let base = config_path.parent().unwrap_or_else(RepoPath::empty);
    let Some(exact) = normalize_from(&base, normalized_specifier) else {
        return Err(ResolverError::Configuration(format!(
            "extends escapes the repository root: {original_specifier}"
        )));
    };
    select_relative_candidate(exact, config, demands, limitations)
}

fn select_workspace_extends(
    config_path: &RepoPath,
    original_specifier: &str,
    normalized_specifier: &str,
    config: &SemanticConfigSnapshot,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<ExtendsSelection, ResolverError> {
    if config.packages.iter().any(|package| {
        package.workspace_root.is_some()
            && matches!(
                &package.identity,
                PackageIdentityState::Unsupported {
                    candidate: Some(identity)
                } if identity.as_str() == normalized_specifier
            )
    }) {
        return Ok(ExtendsSelection::Blocked);
    }
    let matches = config
        .packages
        .iter()
        .filter(|package| {
            package.workspace_root.is_some()
                && matches!(
                    &package.identity,
                    PackageIdentityState::Valid(identity)
                        if identity.as_str() == normalized_specifier
                )
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: config_path.display_escaped(),
            detail: format!(
                "extends must match one admitted workspace package identity: {original_specifier}"
            ),
        });
        return Ok(ExtendsSelection::Blocked);
    }
    let package = matches[0];
    let Some(ConfigObservation::Present {
        document: manifest, ..
    }) = config.observations.get(&package.manifest_path)
    else {
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: config_path.display_escaped(),
            detail: format!(
                "workspace config package manifest is unavailable: {original_specifier}"
            ),
        });
        return Ok(ExtendsSelection::Blocked);
    };
    let target = match manifest.root.get("tsconfig") {
        None => package
            .root
            .join_portable("tsconfig.json")
            .map_err(|error| {
                ResolverError::Configuration(format!("invalid workspace tsconfig path: {error}"))
            })?,
        Some(ConfigValue::String(value)) if !value.is_empty() => {
            let Some(target) = normalize_from(&package.root, &value.replace('\\', "/")) else {
                limitations.push(Limitation::TsconfigSemanticsUnsupported {
                    path: package.manifest_path.display_escaped(),
                    detail: "workspace package tsconfig target escapes the repository root"
                        .to_owned(),
                });
                return Ok(ExtendsSelection::Blocked);
            };
            if !target.is_within(&package.root) {
                limitations.push(Limitation::TsconfigSemanticsUnsupported {
                    path: package.manifest_path.display_escaped(),
                    detail: "workspace package tsconfig target escapes the package root".to_owned(),
                });
                return Ok(ExtendsSelection::Blocked);
            }
            target
        }
        Some(_) => {
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: package.manifest_path.display_escaped(),
                detail: "workspace package tsconfig field must be a nonempty string".to_owned(),
            });
            return Ok(ExtendsSelection::Blocked);
        }
    };
    select_exact_candidate(target, config, demands, limitations)
}

fn select_relative_candidate(
    exact: RepoPath,
    config: &SemanticConfigSnapshot,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<ExtendsSelection, ResolverError> {
    match config.observations.get(&exact) {
        None => {
            demands.push(ConfigDemand {
                path: exact,
                syntax: ConfigSyntax::Jsonc,
            });
            Ok(ExtendsSelection::NeedsInput)
        }
        Some(ConfigObservation::Present { .. }) => Ok(ExtendsSelection::Selected(exact)),
        Some(ConfigObservation::Missing { .. } | ConfigObservation::NonRegular { .. }) => {
            if exact
                .file_name_portable()
                .is_some_and(|name| name.ends_with(".json"))
            {
                limitations.push(Limitation::TsconfigSemanticsUnsupported {
                    path: exact.display_escaped(),
                    detail: "relative extends target is missing or non-regular".to_owned(),
                });
                return Ok(ExtendsSelection::Blocked);
            }
            let fallback = append_json(&exact)?;
            select_exact_candidate(fallback, config, demands, limitations)
        }
        Some(ConfigObservation::Unreadable { .. }) => Ok(ExtendsSelection::Blocked),
    }
}

fn select_exact_candidate(
    path: RepoPath,
    config: &SemanticConfigSnapshot,
    demands: &mut Vec<ConfigDemand>,
    limitations: &mut Vec<Limitation>,
) -> Result<ExtendsSelection, ResolverError> {
    match config.observations.get(&path) {
        None => {
            demands.push(ConfigDemand {
                path,
                syntax: ConfigSyntax::Jsonc,
            });
            Ok(ExtendsSelection::NeedsInput)
        }
        Some(ConfigObservation::Present { .. }) => Ok(ExtendsSelection::Selected(path)),
        Some(ConfigObservation::Missing { .. } | ConfigObservation::NonRegular { .. }) => {
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: path.display_escaped(),
                detail: "selected config target is missing or non-regular".to_owned(),
            });
            Ok(ExtendsSelection::Blocked)
        }
        Some(ConfigObservation::Unreadable { .. }) => Ok(ExtendsSelection::Blocked),
    }
}

fn validate_top_level(
    entries: &[lumin_model::ConfigEntry],
    path: &RepoPath,
    limitations: &mut Vec<Limitation>,
    effective: &mut EffectiveConfig,
) {
    for entry in entries {
        if matches!(entry.key.as_str(), "extends" | "compilerOptions") {
            continue;
        }
        let policies =
            generated_config_policy::tsconfig_top_level_fields(&entry.key).collect::<Vec<_>>();
        let Some(matched) = policies
            .iter()
            .find(|policy| shape_matches(policy.shape, &entry.value))
        else {
            effective.blocked = true;
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: path.display_escaped(),
                detail: format!("unknown or malformed tsconfig field {}", entry.key),
            });
            continue;
        };
        if matched.classification == FieldClassification::UnsupportedResolutionAffecting {
            effective.blocked = true;
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: path.display_escaped(),
                detail: format!(
                    "unsupported resolution-affecting tsconfig field {}",
                    entry.key
                ),
            });
        }
    }
}

fn apply_compiler_options(
    value: &ConfigValue,
    config_path: &RepoPath,
    repository_root: &RepositoryRootIdentity,
    limitations: &mut Vec<Limitation>,
    effective: &mut EffectiveConfig,
) -> Result<(), ResolverError> {
    let Some(entries) = value.as_object() else {
        effective.blocked = true;
        limitations.push(Limitation::TsconfigSemanticsUnsupported {
            path: config_path.display_escaped(),
            detail: "compilerOptions must be an object".to_owned(),
        });
        return Ok(());
    };
    let mut modeled = Vec::new();
    for entry in entries {
        let Some(policy) = generated_config_policy::compiler_option(&entry.key) else {
            effective.blocked = true;
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: config_path.display_escaped(),
                detail: format!("unknown compiler option {}", entry.key),
            });
            continue;
        };
        if !shape_matches(policy.shape, &entry.value) {
            effective.blocked = true;
            limitations.push(Limitation::TsconfigSemanticsUnsupported {
                path: config_path.display_escaped(),
                detail: format!("compiler option {} has the wrong shape", entry.key),
            });
            continue;
        }
        match policy.classification {
            FieldClassification::KnownResolutionNeutral => {}
            FieldClassification::UnsupportedResolutionAffecting => {
                effective.blocked = true;
                limitations.push(Limitation::TsconfigSemanticsUnsupported {
                    path: config_path.display_escaped(),
                    detail: format!(
                        "unsupported resolution-affecting compiler option {}",
                        entry.key
                    ),
                });
            }
            FieldClassification::SupportedAndModeled => modeled.push(entry),
        }
    }
    let declares_base_url = modeled.iter().any(|entry| entry.key == "baseUrl");
    for key in ["baseUrl", "paths", "moduleResolution", "module"] {
        if let Some(entry) = modeled.iter().find(|entry| entry.key == key) {
            apply_modeled_option(
                &entry.key,
                &entry.value,
                config_path,
                repository_root,
                declares_base_url,
                limitations,
                effective,
            )?;
        }
    }
    Ok(())
}

fn apply_modeled_option(
    key: &str,
    value: &ConfigValue,
    config_path: &RepoPath,
    repository_root: &RepositoryRootIdentity,
    declares_base_url: bool,
    limitations: &mut Vec<Limitation>,
    effective: &mut EffectiveConfig,
) -> Result<(), ResolverError> {
    match key {
        "moduleResolution" => {
            let value = value.as_str().unwrap_or_default().to_ascii_lowercase();
            let profile = match value.as_str() {
                "bundler" => ResolutionProfile::Bundler,
                "node" | "node10" => ResolutionProfile::Node,
                "node16" => ResolutionProfile::Node16,
                "nodenext" => ResolutionProfile::NodeNext,
                _ => {
                    effective.blocked = true;
                    limitations.push(Limitation::TsconfigSemanticsUnsupported {
                        path: config_path.display_escaped(),
                        detail: format!("unsupported moduleResolution value {value}"),
                    });
                    return Ok(());
                }
            };
            effective.module_resolution = Some((profile, config_path.clone()));
        }
        "module" => effective.module = value.as_str().map(|value| value.to_ascii_lowercase()),
        "baseUrl" => {
            let base = config_path.parent().unwrap_or_else(RepoPath::empty);
            let value = value.as_str().unwrap_or_default().replace('\\', "/");
            match repository_root.classify_rooted_utf8_path(&value) {
                RootedPathRelation::Contained => {
                    effective.base_url = None;
                    effective.blocked = true;
                    limitations.push(Limitation::TsconfigSemanticsUnsupported {
                        path: config_path.display_escaped(),
                        detail: "rooted baseUrl syntax is unsupported".to_owned(),
                    });
                    return Ok(());
                }
                RootedPathRelation::Outside => {
                    return Err(ResolverError::Configuration(format!(
                        "baseUrl escapes the repository root in {}",
                        config_path.display_escaped()
                    )));
                }
                RootedPathRelation::NotRooted => {}
            }
            let Some(path) = normalize_from(&base, &value) else {
                return Err(ResolverError::Configuration(format!(
                    "baseUrl escapes the repository root in {}",
                    config_path.display_escaped()
                )));
            };
            effective.base_url = Some(path);
        }
        "paths" => {
            let config_directory = config_path.parent().unwrap_or_else(RepoPath::empty);
            let base = if declares_base_url {
                effective.base_url.clone().unwrap_or(config_directory)
            } else {
                config_directory
            };
            match parse_paths(value, &base, repository_root) {
                Ok(paths) => effective.paths = Some(paths),
                Err(PathMappingsError::Unsupported(detail)) => {
                    effective.blocked = true;
                    limitations.push(Limitation::TsconfigSemanticsUnsupported {
                        path: config_path.display_escaped(),
                        detail,
                    });
                }
                Err(PathMappingsError::RootEscape(detail)) => {
                    return Err(ResolverError::Configuration(format!(
                        "{detail} in {}",
                        config_path.display_escaped()
                    )));
                }
            }
        }
        _ => {
            return Err(ResolverError::Policy(format!(
                "modeled compiler option has no implementation: {key}"
            )));
        }
    }
    Ok(())
}

enum PathMappingsError {
    Unsupported(String),
    RootEscape(String),
}

fn parse_paths(
    value: &ConfigValue,
    base: &RepoPath,
    repository_root: &RepositoryRootIdentity,
) -> Result<PathMappings, PathMappingsError> {
    let entries = value.as_object().ok_or_else(|| {
        PathMappingsError::Unsupported("paths must be object<string,array<string>>".to_owned())
    })?;
    let mut mappings = Vec::new();
    for (source_order, entry) in entries.iter().enumerate() {
        if entry.key.matches('*').count() > 1 {
            return Err(PathMappingsError::Unsupported(format!(
                "paths key {} contains multiple stars",
                entry.key
            )));
        }
        let values = entry
            .value
            .as_array()
            .filter(|values| !values.is_empty())
            .ok_or_else(|| {
                PathMappingsError::Unsupported(format!(
                    "paths target {} must be a nonempty array",
                    entry.key
                ))
            })?;
        let mut targets = Vec::new();
        for value in values {
            let target = value.as_str().ok_or_else(|| {
                PathMappingsError::Unsupported(format!(
                    "paths target {} must contain strings",
                    entry.key
                ))
            })?;
            if target.matches('*').count() > 1 || (!entry.key.contains('*') && target.contains('*'))
            {
                return Err(PathMappingsError::Unsupported(format!(
                    "paths target {target} has an incompatible star shape"
                )));
            }
            let normalized_target = target.replace('\\', "/");
            let probe = normalized_target.replace('*', "lumin-star-probe");
            match repository_root.classify_rooted_utf8_path(&probe) {
                RootedPathRelation::Contained => {
                    return Err(PathMappingsError::Unsupported(
                        "paths target uses rooted syntax".to_owned(),
                    ));
                }
                RootedPathRelation::Outside => {
                    return Err(PathMappingsError::RootEscape(
                        "paths target escapes the repository root".to_owned(),
                    ));
                }
                RootedPathRelation::NotRooted => {}
            }
            if normalize_from(base, &probe).is_none() {
                return Err(PathMappingsError::RootEscape(
                    "paths target escapes the repository root".to_owned(),
                ));
            }
            targets.push(target.to_owned());
        }
        mappings.push(PathMapping {
            pattern: entry.key.clone(),
            targets,
            source_order,
        });
    }
    Ok(PathMappings {
        base: base.clone(),
        entries: mappings,
    })
}

fn shape_matches(shape: &str, value: &ConfigValue) -> bool {
    match shape {
        "boolean" => matches!(value, ConfigValue::Boolean(_)),
        "string" | "enum" => matches!(value, ConfigValue::String(_)),
        "number" => matches!(value, ConfigValue::Number(_)),
        "object" => matches!(value, ConfigValue::Object(_)),
        "list" => matches!(value, ConfigValue::Array(_)),
        "array<string>" => value
            .as_array()
            .is_some_and(|values| values.iter().all(|value| value.as_str().is_some())),
        "array<object{path:string}>" => value.as_array().is_some_and(|values| {
            values.iter().all(|value| {
                value.as_object().is_some()
                    && value.get("path").and_then(ConfigValue::as_str).is_some()
            })
        }),
        _ => false,
    }
}

fn module_is_compatible(profile: ResolutionProfile, module: Option<&str>) -> bool {
    matches!(
        (profile, module),
        (_, None)
            | (ResolutionProfile::Node, Some("commonjs"))
            | (
                ResolutionProfile::Bundler,
                Some("preserve" | "es2015" | "es2020" | "es2022" | "esnext")
            )
            | (ResolutionProfile::Node16, Some("node16"))
            | (ResolutionProfile::NodeNext, Some("nodenext"))
    )
}

fn importer_is_esm(
    source: &SourceSnapshot,
    config: &SemanticConfigSnapshot,
    limitations: &mut Vec<Limitation>,
) -> Result<bool, ()> {
    match source.kind {
        SourceKind::Mts | SourceKind::Mjs | SourceKind::DeclarationMts => return Ok(true),
        SourceKind::Cts | SourceKind::CommonJs | SourceKind::DeclarationCts => return Ok(false),
        _ => {}
    }
    let Some(package_root) = config.source_packages.get(&source.id) else {
        return Ok(false);
    };
    let Some(package) = config
        .packages
        .iter()
        .find(|package| &package.root == package_root)
    else {
        return Ok(false);
    };
    let Some(ConfigObservation::Present {
        document: manifest, ..
    }) = config.observations.get(&package.manifest_path)
    else {
        return Ok(false);
    };
    match manifest.root.get("type") {
        None => Ok(false),
        Some(ConfigValue::String(value)) if value == "module" => Ok(true),
        Some(ConfigValue::String(value)) if value == "commonjs" => Ok(false),
        Some(_) => {
            limitations.push(Limitation::ImporterFormatUnsupported {
                path: package.manifest_path.display_escaped(),
                detail: "package type must be module or commonjs for Node profiles".to_owned(),
            });
            Err(())
        }
    }
}

pub(crate) fn normalize_from(base: &RepoPath, value: &str) -> Option<RepoPath> {
    let mut current = base.clone();
    for component in value.replace('\\', "/").split('/') {
        match component {
            "" | "." => {}
            ".." => current = current.parent()?,
            value => current = current.join_portable(value).ok()?,
        }
    }
    Some(current)
}

fn append_json(path: &RepoPath) -> Result<RepoPath, ResolverError> {
    let name = path
        .file_name_portable()
        .ok_or_else(|| ResolverError::Configuration("config path has no UTF-8 name".to_owned()))?;
    let parent = path
        .parent()
        .ok_or_else(|| ResolverError::Configuration("config path has no parent".to_owned()))?;
    parent
        .join_portable(&format!("{name}.json"))
        .map_err(|error| ResolverError::Configuration(error.to_string()))
}
