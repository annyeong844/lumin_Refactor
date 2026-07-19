mod config;

use std::collections::BTreeMap;

use lumin_model::{
    ConfigSyntax, FileFacts, Limitation, LogicalSourceId, PackageIdentityState, RepoPath,
    ResolutionOutcome, ResolutionProfile, ResolvedSourceUse, SelectedResolutionProfile,
    SemanticConfigSnapshot, SourceSnapshot, SourceUseFact, SymbolNamespace,
};
use thiserror::Error;

pub const RESOLVER_VERSION: &str = "config-profile-resolution.v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfigDemand {
    pub path: RepoPath,
    pub syntax: ConfigSyntax,
}

#[derive(Clone, Debug)]
pub struct ResolverOutput {
    pub resolved: Vec<ResolvedSourceUse>,
    pub profiles: Vec<SelectedResolutionProfile>,
    pub limitations: Vec<Limitation>,
    pub demands: Vec<ConfigDemand>,
}

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("resolver policy artifact is invalid: {0}")]
    Policy(String),
    #[error("resolver configuration is invalid: {0}")]
    Configuration(String),
}

pub fn resolve_all(
    sources: &[SourceSnapshot],
    facts: &[FileFacts],
    semantic_config: &SemanticConfigSnapshot,
    override_profile: Option<ResolutionProfile>,
) -> Result<ResolverOutput, ResolverError> {
    let mut selection = config::select(sources, semantic_config, override_profile)?;
    if !selection.demands.is_empty() {
        return Ok(ResolverOutput {
            resolved: Vec::new(),
            profiles: selection.profiles,
            limitations: selection.limitations,
            demands: selection.demands,
        });
    }
    let source_by_path = sources
        .iter()
        .map(|source| (source.path.clone(), source.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let path_by_source = sources
        .iter()
        .map(|source| (source.id.clone(), source.path.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut resolved = Vec::new();
    for file in facts {
        let Some(importer_path) = path_by_source.get(&file.source_id) else {
            continue;
        };
        let Some(settings) = selection.settings.get(&file.source_id) else {
            continue;
        };
        for source_use in &file.uses {
            let (outcome, limitation) = resolve_one(
                importer_path,
                source_use,
                &source_by_path,
                settings,
                semantic_config,
            );
            if let Some(limitation) = limitation {
                selection.limitations.push(limitation);
            }
            resolved.push(ResolvedSourceUse {
                source_use: source_use.clone(),
                outcome,
            });
        }
    }
    resolved.sort_by(|left, right| {
        left.source_use
            .importer
            .cmp(&right.source_use.importer)
            .then_with(|| left.source_use.span.start.cmp(&right.source_use.span.start))
            .then_with(|| left.source_use.specifier.cmp(&right.source_use.specifier))
    });
    Ok(ResolverOutput {
        resolved,
        profiles: selection.profiles,
        limitations: selection.limitations,
        demands: Vec::new(),
    })
}

fn resolve_one(
    importer_path: &RepoPath,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings: &config::ImporterSettings,
    semantic_config: &SemanticConfigSnapshot,
) -> (ResolutionOutcome, Option<Limitation>) {
    let specifier = source_use.specifier.as_str();
    if settings.blocked {
        return (
            ResolutionOutcome::Unsupported {
                specifier: specifier.to_owned(),
                reason: "the importer's semantic configuration is incomplete".to_owned(),
            },
            None,
        );
    }
    if specifier.starts_with('/') || specifier.starts_with('\\') {
        return unsupported_with_unknown_limitation(
            source_use,
            "root-absolute internal-looking specifier".to_owned(),
        );
    }
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return resolve_bare_specifier(specifier, source_use, sources, settings, semantic_config);
    }
    resolve_relative_specifier(specifier, source_use, importer_path, sources, settings)
}

fn resolve_bare_specifier(
    specifier: &str,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings: &config::ImporterSettings,
    semantic_config: &SemanticConfigSnapshot,
) -> (ResolutionOutcome, Option<Limitation>) {
    if specifier.starts_with('#') {
        return unsupported_with_unknown_limitation(
            source_use,
            "package imports are unsupported".to_owned(),
        );
    }
    if let Some(outcome) = resolve_paths(specifier, source_use, sources, settings) {
        return (outcome, None);
    }
    if let Some(base_url) = &settings.base_url
        && let Some(base) = config::normalize_from(base_url, specifier)
    {
        let candidates = candidates(&base, source_use.namespace, settings.allow_extensionless);
        if let Some(target) = candidates.iter().find_map(|path| sources.get(path)) {
            return (
                ResolutionOutcome::Internal {
                    target: target.clone(),
                },
                None,
            );
        }
    }
    let bare_identity = package_name(specifier);
    if let Some(package) = semantic_config.packages.iter().find(|package| {
        package.workspace_root.is_some()
            && matches!(
                &package.identity,
                PackageIdentityState::Valid(identity) if identity.as_str() == bare_identity
            )
    }) {
        return (
            ResolutionOutcome::Unsupported {
                specifier: specifier.to_owned(),
                reason: "workspace package public entry resolution is not implemented yet"
                    .to_owned(),
            },
            Some(Limitation::PublicSurfaceUnsupported {
                path: package.manifest_path.display_escaped(),
                detail: format!(
                    "workspace package import {specifier} requires package entry semantics"
                ),
            }),
        );
    }
    (
        ResolutionOutcome::External {
            package: bare_identity,
        },
        None,
    )
}

fn resolve_relative_specifier(
    specifier: &str,
    source_use: &SourceUseFact,
    importer_path: &RepoPath,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings: &config::ImporterSettings,
) -> (ResolutionOutcome, Option<Limitation>) {
    let Some(base) = normalize_relative(importer_path, specifier) else {
        return unsupported_with_unknown_limitation(
            source_use,
            "relative specifier escapes the canonical root".to_owned(),
        );
    };
    if !settings.allow_extensionless
        && base
            .file_name_portable()
            .is_some_and(|name| !name.contains('.'))
    {
        return unsupported_with_unknown_limitation(
            source_use,
            format!(
                "{} ESM resolution requires an explicit relative extension",
                settings.profile.as_str()
            ),
        );
    }
    let candidates = candidates(&base, source_use.namespace, settings.allow_extensionless);
    for candidate in &candidates {
        if let Some(target) = sources.get(candidate) {
            return (
                ResolutionOutcome::Internal {
                    target: target.clone(),
                },
                None,
            );
        }
    }

    if has_unsupported_explicit_extension(&base) {
        return (
            ResolutionOutcome::NonSourceAsset {
                specifier: specifier.to_owned(),
            },
            None,
        );
    }

    (
        ResolutionOutcome::Unresolved {
            specifier: specifier.to_owned(),
            candidates: candidates.iter().map(RepoPath::display_escaped).collect(),
        },
        None,
    )
}

fn unsupported_with_unknown_limitation(
    source_use: &SourceUseFact,
    reason: String,
) -> (ResolutionOutcome, Option<Limitation>) {
    let specifier = source_use.specifier.clone();
    let detail = format!("unsupported specifier {specifier}: {reason}");
    (
        ResolutionOutcome::Unsupported { specifier, reason },
        Some(Limitation::JsModuleUseUnknown {
            source_id: source_use.importer.clone(),
            detail,
        }),
    )
}

fn normalize_relative(importer: &RepoPath, specifier: &str) -> Option<RepoPath> {
    let mut current = importer.parent()?;
    for component in specifier.split('/') {
        match component {
            "" | "." => {}
            ".." => current = current.parent()?,
            value => current = current.join_portable(value).ok()?,
        }
    }
    Some(current)
}

fn candidates(
    base: &RepoPath,
    namespace: SymbolNamespace,
    allow_extensionless: bool,
) -> Vec<RepoPath> {
    let Some(file_name) = base.file_name_portable() else {
        return vec![base.clone()];
    };
    let Some(parent) = base.parent() else {
        return vec![base.clone()];
    };

    let names: Vec<String> = if let Some(stem) = file_name.strip_suffix(".js") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".ts", ".tsx", ".d.ts", ".js", ".jsx"]
        } else {
            vec![".ts", ".tsx", ".js", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".jsx") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".tsx", ".d.ts", ".jsx"]
        } else {
            vec![".tsx", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".mjs") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".mts", ".d.mts", ".mjs"]
        } else {
            vec![".mts", ".mjs"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".cjs") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".cts", ".d.cts", ".cjs"]
        } else {
            vec![".cts", ".cjs"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if file_name.contains('.') {
        return vec![base.clone()];
    } else if allow_extensionless {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".ts", ".tsx", ".d.ts", ".js", ".jsx"]
        } else {
            vec![".ts", ".tsx", ".js", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{file_name}{extension}"))
            .collect()
    } else {
        return vec![base.clone()];
    };

    names
        .into_iter()
        .filter_map(|name| parent.join_portable(&name).ok())
        .collect()
}

fn resolve_paths(
    specifier: &str,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings: &config::ImporterSettings,
) -> Option<ResolutionOutcome> {
    let mappings = settings.paths.as_ref()?;
    let mapping = mappings
        .entries
        .iter()
        .find(|mapping| !mapping.pattern.contains('*') && mapping.pattern == specifier)
        .or_else(|| {
            mappings
                .entries
                .iter()
                .filter_map(|mapping| {
                    let (prefix, suffix) = mapping.pattern.split_once('*')?;
                    if specifier.starts_with(prefix)
                        && specifier.ends_with(suffix)
                        && specifier.len() >= prefix.len() + suffix.len()
                    {
                        Some((mapping, prefix.len()))
                    } else {
                        None
                    }
                })
                .max_by(|(left, left_prefix), (right, right_prefix)| {
                    left_prefix
                        .cmp(right_prefix)
                        .then_with(|| right.source_order.cmp(&left.source_order))
                })
                .map(|(mapping, _)| mapping)
        })?;
    let capture = mapping.pattern.split_once('*').map(|(prefix, suffix)| {
        &specifier[prefix.len()..specifier.len().saturating_sub(suffix.len())]
    });
    let mut consulted = Vec::new();
    for target in &mapping.targets {
        let target = match capture {
            Some(capture) => target.replacen('*', capture, 1),
            None => target.clone(),
        };
        let Some(base) = config::normalize_from(&mappings.base, &target) else {
            continue;
        };
        for candidate in candidates(&base, source_use.namespace, settings.allow_extensionless) {
            if let Some(target) = sources.get(&candidate) {
                return Some(ResolutionOutcome::Internal {
                    target: target.clone(),
                });
            }
            consulted.push(candidate.display_escaped());
        }
    }
    Some(ResolutionOutcome::Unresolved {
        specifier: specifier.to_owned(),
        candidates: consulted,
    })
}

fn has_supported_explicit_extension(file_name: &str) -> bool {
    [
        ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".vue", ".svelte", ".astro",
        ".d.ts", ".d.mts", ".d.cts",
    ]
    .iter()
    .any(|extension| file_name.ends_with(extension))
}

fn has_unsupported_explicit_extension(path: &RepoPath) -> bool {
    path.file_name_portable()
        .is_some_and(|name| name.contains('.') && !has_supported_explicit_extension(name))
}

fn package_name(specifier: &str) -> String {
    if let Some(scoped) = specifier.strip_prefix('@') {
        let mut parts = scoped.split('/');
        let scope = parts.next().unwrap_or_default();
        let package = parts.next().unwrap_or_default();
        if package.is_empty() {
            format!("@{scope}")
        } else {
            format!("@{scope}/{package}")
        }
    } else {
        specifier.split('/').next().unwrap_or(specifier).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use lumin_model::{
        ImportKind, SourceKind, SourceRoles, SourceSpan, SourceUseFact, SymbolNamespace,
    };

    use super::*;

    #[test]
    fn js_candidate_prefers_typescript_source() -> Result<(), Box<dyn std::error::Error>> {
        let importer = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            Vec::new(),
        );
        let target = SourceSnapshot::new(
            RepoPath::from_portable("src/lib.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            Vec::new(),
        );
        let source_use = SourceUseFact {
            importer: importer.id.clone(),
            specifier: "./lib.js".to_owned(),
            imported_name: Some("used".to_owned()),
            namespace: SymbolNamespace::Value,
            kind: ImportKind::Named,
            span: SourceSpan { start: 0, end: 10 },
        };
        let config = SemanticConfigSnapshot::default();
        let settings = config::ImporterSettings {
            profile: ResolutionProfile::Bundler,
            allow_extensionless: true,
            base_url: None,
            paths: None,
            blocked: false,
        };
        let (outcome, limitation) = resolve_one(
            &importer.path,
            &source_use,
            &[(target.path.clone(), target.id.clone())]
                .into_iter()
                .collect(),
            &settings,
            &config,
        );
        assert!(limitation.is_none());
        assert_eq!(outcome, ResolutionOutcome::Internal { target: target.id });
        Ok(())
    }
}
