use std::collections::BTreeMap;

use lumin_model::{
    ConfigEntry, ConfigValue, LogicalSourceId, PackageFact, PackageSurfaceLane,
    PackageSurfaceSource, RepoPath, SymbolNamespace,
};

use super::{PackageResolution, TargetRequest, resolve_base, unresolved, unsupported};

pub(super) fn resolve(
    package: &PackageFact,
    specifier: &str,
    request_key: &str,
    namespace: SymbolNamespace,
    lane: PackageSurfaceLane,
    exports: &ConfigValue,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
) -> PackageResolution {
    if let Err(detail) = validate(exports, &package.root) {
        return unsupported(package, specifier, &detail);
    }
    let selected = match select(exports, request_key, lane, namespace) {
        Ok(selected) => selected,
        Err(detail) => return unsupported(package, specifier, &detail),
    };
    let Some(selected) = selected else {
        return unresolved(specifier, Vec::new());
    };
    let Some(target) = selected.target else {
        return unresolved(specifier, Vec::new());
    };
    let base = match lower_target(&package.root, &target, selected.capture.as_deref()) {
        Ok(base) => base,
        Err(detail) => return unsupported(package, specifier, &detail),
    };
    resolve_base(
        package,
        TargetRequest {
            specifier,
            namespace,
            source: PackageSurfaceSource::Exports {
                key: selected.key,
                condition: selected.condition,
                lane,
            },
            base,
            allow_extensionless: super::fallback::lane_allows_extensionless(lane),
            allow_directory: false,
        },
        sources,
    )
}

struct SelectedExport {
    target: Option<String>,
    key: String,
    condition: Option<String>,
    capture: Option<String>,
}

pub(super) struct SelectedSubpath {
    pub target: Option<String>,
    pub condition: Option<String>,
}

fn validate(exports: &ConfigValue, package_root: &RepoPath) -> Result<(), String> {
    match exports {
        ConfigValue::String(target) => validate_target_for_key(target, false, package_root),
        ConfigValue::Null => Ok(()),
        ConfigValue::Object(entries) => match object_kind(entries)? {
            ObjectKind::Conditions => validate_condition_object(entries, false, package_root),
            ObjectKind::Subpaths => {
                for entry in entries {
                    let pattern = validate_subpath_key(&entry.key)?;
                    match &entry.value {
                        ConfigValue::String(target) => {
                            validate_target_for_key(target, pattern, package_root)?;
                        }
                        ConfigValue::Null => {}
                        ConfigValue::Object(conditions) => {
                            validate_condition_object(conditions, pattern, package_root)?;
                        }
                        _ => {
                            return Err(
                                "exports subpath values must be string, null, or one condition object"
                                    .to_owned(),
                            );
                        }
                    }
                }
                Ok(())
            }
        },
        _ => Err("package exports must match exports-v1".to_owned()),
    }
}

#[derive(Clone, Copy)]
pub(super) enum ObjectKind {
    Conditions,
    Subpaths,
}

pub(super) fn object_kind(entries: &[ConfigEntry]) -> Result<ObjectKind, String> {
    if entries.is_empty() {
        return Ok(ObjectKind::Subpaths);
    }
    let subpaths = entries
        .iter()
        .filter(|entry| entry.key.starts_with('.'))
        .count();
    let conditions = entries
        .iter()
        .filter(|entry| supported_condition(&entry.key))
        .count();
    if subpaths == entries.len() {
        Ok(ObjectKind::Subpaths)
    } else if conditions == entries.len() {
        Ok(ObjectKind::Conditions)
    } else {
        Err("exports cannot mix subpath and condition keys or use unknown conditions".to_owned())
    }
}

fn validate_condition_object(
    entries: &[ConfigEntry],
    pattern: bool,
    package_root: &RepoPath,
) -> Result<(), String> {
    for entry in entries {
        if !supported_condition(&entry.key) {
            return Err(format!("unsupported package condition {}", entry.key));
        }
        match &entry.value {
            ConfigValue::String(target) => {
                validate_target_for_key(target, pattern, package_root)?;
            }
            ConfigValue::Null => {}
            _ => return Err("nested or non-string package conditions are unsupported".to_owned()),
        }
    }
    Ok(())
}

fn supported_condition(value: &str) -> bool {
    matches!(value, "default" | "import" | "require" | "node" | "types")
}

pub(super) fn validate_subpath_key(key: &str) -> Result<bool, String> {
    if key == "." {
        return Ok(false);
    }
    let Some(path) = key.strip_prefix("./") else {
        return Err(format!("invalid exports subpath key {key}"));
    };
    validate_path_text(path, true)?;
    let stars = key.matches('*').count();
    if stars > 1 {
        return Err(format!("exports subpath key {key} contains multiple stars"));
    }
    Ok(stars == 1)
}

fn validate_target_for_key(
    target: &str,
    pattern: bool,
    package_root: &RepoPath,
) -> Result<(), String> {
    if !pattern && target.contains('*') {
        return Err("an exact exports key cannot select a pattern target".to_owned());
    }
    if target.matches('*').count() > 1 {
        return Err("an exports target may contain at most one star".to_owned());
    }
    let capture = target.contains('*').then_some("lumin-pattern");
    lower_target(package_root, target, capture).map(|_| ())
}

fn select(
    exports: &ConfigValue,
    request_key: &str,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
) -> Result<Option<SelectedExport>, String> {
    match exports {
        ConfigValue::String(target) if request_key == "." => Ok(Some(SelectedExport {
            target: Some(target.clone()),
            key: ".".to_owned(),
            condition: None,
            capture: None,
        })),
        ConfigValue::Null if request_key == "." => Ok(Some(SelectedExport {
            target: None,
            key: ".".to_owned(),
            condition: None,
            capture: None,
        })),
        ConfigValue::String(_) | ConfigValue::Null => Ok(None),
        ConfigValue::Object(entries) => match object_kind(entries)? {
            ObjectKind::Conditions if request_key == "." => Ok(select_condition(
                entries, lane, namespace,
            )
            .map(|(target, condition)| SelectedExport {
                target,
                key: ".".to_owned(),
                condition: Some(condition),
                capture: None,
            })),
            ObjectKind::Conditions => Ok(None),
            ObjectKind::Subpaths => {
                let selected = entries
                    .iter()
                    .enumerate()
                    .find(|(_, entry)| entry.key == request_key)
                    .map(|(index, entry)| (index, entry, None))
                    .or_else(|| select_pattern(entries, request_key));
                let Some((_, entry, capture)) = selected else {
                    return Ok(None);
                };
                select_subpath_value(&entry.value, lane, namespace).map(|selected| {
                    selected.map(|selected| SelectedExport {
                        target: selected.target,
                        key: entry.key.clone(),
                        condition: selected.condition,
                        capture,
                    })
                })
            }
        },
        _ => Err("package exports must match exports-v1".to_owned()),
    }
}

pub(super) fn select_subpath_value(
    value: &ConfigValue,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
) -> Result<Option<SelectedSubpath>, String> {
    match value {
        ConfigValue::String(target) => Ok(Some(SelectedSubpath {
            target: Some(target.clone()),
            condition: None,
        })),
        ConfigValue::Null => Ok(Some(SelectedSubpath {
            target: None,
            condition: None,
        })),
        ConfigValue::Object(entries) => Ok(select_condition(entries, lane, namespace).map(
            |(target, condition)| SelectedSubpath {
                target,
                condition: Some(condition),
            },
        )),
        _ => Err("exports subpath values must match exports-v1".to_owned()),
    }
}

fn select_condition(
    entries: &[ConfigEntry],
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
) -> Option<(Option<String>, String)> {
    entries.iter().find_map(|entry| {
        if entry.key != "default" && !condition_active(&entry.key, lane, namespace) {
            return None;
        }
        match &entry.value {
            ConfigValue::String(target) => Some((Some(target.clone()), entry.key.clone())),
            ConfigValue::Null => Some((None, entry.key.clone())),
            _ => None,
        }
    })
}

fn condition_active(condition: &str, lane: PackageSurfaceLane, namespace: SymbolNamespace) -> bool {
    matches!(
        (lane, namespace, condition),
        (
            PackageSurfaceLane::BundlerImport,
            SymbolNamespace::Value,
            "import"
        ) | (
            PackageSurfaceLane::BundlerImport,
            SymbolNamespace::Type,
            "types" | "import"
        ) | (
            PackageSurfaceLane::NodeImport,
            SymbolNamespace::Value,
            "node" | "import"
        ) | (
            PackageSurfaceLane::NodeImport,
            SymbolNamespace::Type,
            "node" | "types" | "import"
        ) | (
            PackageSurfaceLane::NodeRequire,
            SymbolNamespace::Value,
            "node" | "require"
        ) | (
            PackageSurfaceLane::NodeRequire,
            SymbolNamespace::Type,
            "node" | "types" | "require"
        )
    )
}

fn select_pattern<'a>(
    entries: &'a [ConfigEntry],
    request_key: &str,
) -> Option<(usize, &'a ConfigEntry, Option<String>)> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let capture = pattern_capture(&entry.key, request_key)?;
            Some((index, entry, capture))
        })
        .max_by(|(left_index, left, _), (right_index, right, _)| {
            pattern_specificity(&left.key)
                .cmp(&pattern_specificity(&right.key))
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, entry, capture)| (index, entry, Some(capture)))
}

fn pattern_specificity(value: &str) -> (usize, usize) {
    let star = value.find('*').unwrap_or(value.len());
    (
        value[..star.saturating_add(1)].encode_utf16().count(),
        value.encode_utf16().count(),
    )
}

pub(super) fn pattern_capture(pattern: &str, candidate: &str) -> Option<String> {
    let (prefix, suffix) = pattern.split_once('*')?;
    if !candidate.starts_with(prefix)
        || !candidate.ends_with(suffix)
        || candidate.len() < prefix.len() + suffix.len() + 1
    {
        return None;
    }
    let capture = &candidate[prefix.len()..candidate.len() - suffix.len()];
    validate_path_text(capture, false).ok()?;
    Some(capture.to_owned())
}

pub(super) fn lower_field_target(
    package_root: &RepoPath,
    target: &str,
) -> Result<RepoPath, String> {
    let normalized = if target.starts_with("./") {
        target.to_owned()
    } else {
        format!("./{target}")
    };
    lower_target(package_root, &normalized, None)
}

pub(super) fn lower_target(
    package_root: &RepoPath,
    target: &str,
    capture: Option<&str>,
) -> Result<RepoPath, String> {
    let Some(path) = target.strip_prefix("./") else {
        return Err("package targets must begin with ./".to_owned());
    };
    validate_path_text(path, true)?;
    let stars = path.matches('*').count();
    if stars > 1 {
        return Err("package targets may contain at most one star".to_owned());
    }
    let lowered = if stars == 1 {
        let capture = capture.ok_or_else(|| "package target pattern has no capture".to_owned())?;
        validate_path_text(capture, false)?;
        path.replacen('*', capture, 1)
    } else {
        path.to_owned()
    };
    validate_path_text(&lowered, false)?;
    let mut result = package_root.clone();
    for component in lowered.split('/') {
        result = result
            .join_portable(component)
            .map_err(|error| format!("invalid package target component: {error}"))?;
    }
    if !result.is_within(package_root) {
        return Err("package target escapes the package root".to_owned());
    }
    Ok(result)
}

fn validate_path_text(path: &str, allow_star: bool) -> Result<(), String> {
    if path.is_empty() {
        return Err("package target contains an empty component".to_owned());
    }
    let mut stars = 0;
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.eq_ignore_ascii_case("node_modules")
        {
            return Err(format!("invalid package target component {component:?}"));
        }
        if component
            .chars()
            .any(|value| matches!(value, '%' | '?' | '#' | '\\' | '\0'))
        {
            return Err("package target contains a forbidden path character".to_owned());
        }
        stars += component.matches('*').count();
    }
    if (!allow_star && stars != 0) || stars > 1 {
        return Err("package target has an invalid star shape".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_pattern_comparator_prefers_longer_prefix_then_suffix() {
        let entries = vec![
            ConfigEntry {
                key: "./features/*".to_owned(),
                value: ConfigValue::Null,
            },
            ConfigEntry {
                key: "./features/internal/*".to_owned(),
                value: ConfigValue::Null,
            },
            ConfigEntry {
                key: "./features/*.js".to_owned(),
                value: ConfigValue::Null,
            },
        ];
        assert_eq!(
            select_pattern(&entries, "./features/internal/x")
                .map(|(_, entry, _)| entry.key.as_str()),
            Some("./features/internal/*")
        );
        assert_eq!(
            select_pattern(&entries, "./features/x.js").map(|(_, entry, _)| entry.key.as_str()),
            Some("./features/*.js")
        );
    }

    #[test]
    fn package_target_rejects_encoded_and_structural_escape()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = RepoPath::from_portable("packages/lib")?;
        for target in [
            "./dist%2Findex.js",
            "./index.js?mode=x",
            "./index.js#x",
            ".\\index.js",
            "./../index.js",
            "./node_modules/x.js",
        ] {
            assert!(lower_target(&root, target, None).is_err(), "{target}");
        }
        Ok(())
    }
}
