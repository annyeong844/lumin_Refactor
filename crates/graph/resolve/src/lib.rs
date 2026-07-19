use std::collections::BTreeMap;

use lumin_model::{
    FileFacts, LogicalSourceId, RepoPath, ResolutionOutcome, ResolvedSourceUse, SourceSnapshot,
    SourceUseFact,
};

pub const RESOLVER_VERSION: &str = "relative-bundler-resolution.v1";

pub fn resolve_all(sources: &[SourceSnapshot], facts: &[FileFacts]) -> Vec<ResolvedSourceUse> {
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
        for source_use in &file.uses {
            resolved.push(ResolvedSourceUse {
                source_use: source_use.clone(),
                outcome: resolve_one(importer_path, source_use, &source_by_path),
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
    resolved
}

fn resolve_one(
    importer_path: &RepoPath,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
) -> ResolutionOutcome {
    let specifier = source_use.specifier.as_str();
    if specifier.starts_with('/') || specifier.starts_with('\\') {
        return ResolutionOutcome::Unsupported {
            specifier: specifier.to_owned(),
            reason: "root-absolute internal-looking specifier".to_owned(),
        };
    }
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        if specifier.starts_with('#') {
            return ResolutionOutcome::Unsupported {
                specifier: specifier.to_owned(),
                reason: "package imports are not available in this increment".to_owned(),
            };
        }
        return ResolutionOutcome::External {
            package: package_name(specifier),
        };
    }

    let Some(base) = normalize_relative(importer_path, specifier) else {
        return ResolutionOutcome::Unsupported {
            specifier: specifier.to_owned(),
            reason: "relative specifier escapes the canonical root".to_owned(),
        };
    };
    let candidates = candidates(&base);
    for candidate in &candidates {
        if let Some(target) = sources.get(candidate) {
            return ResolutionOutcome::Internal {
                target: target.clone(),
            };
        }
    }

    if has_unsupported_explicit_extension(&base) {
        return ResolutionOutcome::NonSourceAsset {
            specifier: specifier.to_owned(),
        };
    }

    ResolutionOutcome::Unresolved {
        specifier: specifier.to_owned(),
        candidates: candidates.iter().map(RepoPath::display_escaped).collect(),
    }
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

fn candidates(base: &RepoPath) -> Vec<RepoPath> {
    let Some(file_name) = base.file_name_portable() else {
        return vec![base.clone()];
    };
    let Some(parent) = base.parent() else {
        return vec![base.clone()];
    };

    let names: Vec<String> = if let Some(stem) = file_name.strip_suffix(".js") {
        [".ts", ".tsx", ".js", ".jsx"]
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".jsx") {
        [".tsx", ".jsx"]
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".mjs") {
        [".mts", ".mjs"]
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".cjs") {
        [".cts", ".cjs"]
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if file_name.contains('.') {
        return vec![base.clone()];
    } else {
        [".ts", ".tsx", ".js", ".jsx"]
            .into_iter()
            .map(|extension| format!("{file_name}{extension}"))
            .collect()
    };

    names
        .into_iter()
        .filter_map(|name| parent.join_portable(&name).ok())
        .collect()
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
        let outcome = resolve_one(
            &importer.path,
            &source_use,
            &[(target.path.clone(), target.id.clone())]
                .into_iter()
                .collect(),
        );
        assert_eq!(outcome, ResolutionOutcome::Internal { target: target.id });
        Ok(())
    }
}
