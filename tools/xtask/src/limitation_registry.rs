//! Exhaustive limitation registry and fact-owner enforcement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::parse::{Parse, ParseStream};
use syn::visit::Visit;
use syn::{Ident, Token, braced};

const MODEL_SOURCE: &str = "crates/foundation/model/src/facts.rs";
const SLICE_SPEC: &str = "specs/001-foundation-slice.md";

#[derive(Debug, Default)]
pub(crate) struct LimitationRegistryResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

#[derive(Clone, Debug)]
struct RegistryRow {
    variant: String,
    owner: String,
    scope: String,
    absence: String,
    gate: String,
}

#[derive(Clone, Debug)]
struct SpecRow {
    owner: String,
}

#[derive(Clone, Debug)]
struct Constructor {
    owner: String,
    file: String,
}

pub(crate) fn check_limitation_registry(
    production_members: &[crate::metadata::WorkspaceMember],
    workspace_root: &Path,
) -> LimitationRegistryResult {
    let mut result = LimitationRegistryResult::default();
    let model_source = match read_text(workspace_root, MODEL_SOURCE) {
        Ok(source) => source,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    let (model_variants, registry_rows) = match parse_model_contract(&model_source) {
        Ok(contract) => contract,
        Err(error) => {
            result
                .tool_errors
                .push(format!("limitation registry model parse: {error}"));
            return result;
        }
    };
    let spec_source = match read_text(workspace_root, SLICE_SPEC) {
        Ok(source) => source,
        Err(error) => {
            result.tool_errors.push(error);
            return result;
        }
    };
    let spec_rows = match parse_spec_rows(&spec_source) {
        Ok(rows) => rows,
        Err(error) => {
            result
                .tool_errors
                .push(format!("limitation registry spec parse: {error}"));
            return result;
        }
    };

    let registry_by_variant = validate_contract(
        &model_variants,
        &registry_rows,
        &spec_rows,
        production_members,
        &mut result.violations,
    );
    let constructors = scan_constructors(production_members, workspace_root, &mut result);
    validate_constructors(
        &model_variants,
        &registry_by_variant,
        &constructors,
        &mut result.violations,
    );

    result.violations.sort();
    result.violations.dedup();
    result.tool_errors.sort();
    result.tool_errors.dedup();
    result
}

fn read_text(workspace_root: &Path, relative: &str) -> Result<String, String> {
    let path = workspace_path(workspace_root, relative);
    std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn parse_model_contract(source: &str) -> Result<(BTreeSet<String>, Vec<RegistryRow>), String> {
    let syntax = syn::parse_file(source).map_err(|error| error.to_string())?;
    let limitation = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Limitation" => Some(item),
            _ => None,
        })
        .ok_or_else(|| "Limitation enum is missing".to_owned())?;
    let model_variants = limitation
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect::<BTreeSet<_>>();
    let registry_macro = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Macro(item)
                if item
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "define_limitation_registry") =>
            {
                Some(&item.mac)
            }
            _ => None,
        })
        .ok_or_else(|| "define_limitation_registry! invocation is missing".to_owned())?;
    let parsed = syn::parse2::<RegistryInput>(registry_macro.tokens.clone())
        .map_err(|error| error.to_string())?;
    Ok((model_variants, parsed.rows))
}

struct RegistryInput {
    rows: Vec<RegistryRow>,
}

impl Parse for RegistryInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut rows = Vec::new();
        while !input.is_empty() {
            let variant = input.parse::<Ident>()?.to_string();
            input.parse::<Token![=>]>()?;
            let content;
            braced!(content in input);
            let mut fields = BTreeMap::new();
            while !content.is_empty() {
                let key = content.parse::<Ident>()?.to_string();
                content.parse::<Token![:]>()?;
                let value = content.parse::<Ident>()?.to_string();
                if fields.insert(key.clone(), value).is_some() {
                    return Err(content.error(format!("duplicate registry field {key}")));
                }
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }
            let field = |name: &str| {
                fields
                    .get(name)
                    .cloned()
                    .ok_or_else(|| input.error(format!("missing registry field {name}")))
            };
            if fields.len() != 4 {
                return Err(input.error("registry row must contain exactly four fields"));
            }
            rows.push(RegistryRow {
                variant,
                owner: field("owner")?,
                scope: field("scope")?,
                absence: field("absence")?,
                gate: field("gate")?,
            });
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self { rows })
    }
}

fn parse_spec_rows(source: &str) -> Result<BTreeMap<String, SpecRow>, String> {
    let mut rows = BTreeMap::new();
    for line in source.lines() {
        let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
        if cells.len() < 7 {
            continue;
        }
        let Some(variant) = first_code_span(cells[1]) else {
            continue;
        };
        let Some(owner) = first_code_span(cells[2]) else {
            return Err(format!("registry row {variant} has no coded fact owner"));
        };
        if cells[3].is_empty() || cells[4].is_empty() || cells[5].is_empty() {
            return Err(format!(
                "registry row {variant} is missing scope, absence, or gate relevance"
            ));
        }
        if cells[3..=5].iter().any(|cell| cell.contains("GateEffect")) {
            return Err(format!(
                "registry row {variant} assigns lifecycle GateEffect directly"
            ));
        }
        if rows
            .insert(
                variant.to_owned(),
                SpecRow {
                    owner: owner.to_owned(),
                },
            )
            .is_some()
        {
            return Err(format!("duplicate limitation registry row {variant}"));
        }
    }
    if rows.is_empty() {
        return Err("no limitation registry rows found".to_owned());
    }
    Ok(rows)
}

fn first_code_span(value: &str) -> Option<&str> {
    let start = value.find('`')? + 1;
    let tail = &value[start..];
    let end = tail.find('`')?;
    Some(&tail[..end])
}

fn validate_contract<'a>(
    model_variants: &BTreeSet<String>,
    registry_rows: &'a [RegistryRow],
    spec_rows: &BTreeMap<String, SpecRow>,
    production_members: &[crate::metadata::WorkspaceMember],
    violations: &mut Vec<String>,
) -> BTreeMap<String, &'a RegistryRow> {
    let production_names = production_members
        .iter()
        .map(|member| member.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut registry = BTreeMap::new();
    for row in registry_rows {
        if registry.insert(row.variant.clone(), row).is_some() {
            violations.push(format!(
                "LIMITATION REGISTRY DUPLICATE: {} appears more than once",
                row.variant
            ));
            continue;
        }
        let expected_owner = owner_package(&row.owner);
        if !production_names.contains(expected_owner.as_str()) {
            violations.push(format!(
                "LIMITATION REGISTRY OWNER: {} names unknown owner {} ({expected_owner})",
                row.variant, row.owner
            ));
        }
        match spec_rows.get(&row.variant) {
            Some(spec) if spec.owner == expected_owner => {}
            Some(spec) => violations.push(format!(
                "LIMITATION REGISTRY OWNER DRIFT: {} is {} in model but {} in {SLICE_SPEC}",
                row.variant, expected_owner, spec.owner
            )),
            None => violations.push(format!(
                "LIMITATION REGISTRY CONTRACT: {} has no row in {SLICE_SPEC}",
                row.variant
            )),
        }
        if row.scope == "GateEffect" || row.absence == "GateEffect" || row.gate == "GateEffect" {
            violations.push(format!(
                "LIMITATION REGISTRY EFFECT LEAK: {} assigns GateEffect directly",
                row.variant
            ));
        }
    }

    let registry_variants = registry.keys().cloned().collect::<BTreeSet<_>>();
    for variant in model_variants.difference(&registry_variants) {
        violations.push(format!(
            "LIMITATION REGISTRY MISSING: Limitation::{variant} has no static row"
        ));
    }
    for variant in registry_variants.difference(model_variants) {
        violations.push(format!(
            "LIMITATION REGISTRY STALE: {variant} has no Limitation enum variant"
        ));
    }
    registry
}

fn owner_package(owner: &str) -> String {
    let mut kebab = String::new();
    for (index, character) in owner.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                kebab.push('-');
            }
            kebab.push(character.to_ascii_lowercase());
        } else {
            kebab.push(character);
        }
    }
    format!("lumin-{kebab}")
}

fn scan_constructors(
    production_members: &[crate::metadata::WorkspaceMember],
    workspace_root: &Path,
    result: &mut LimitationRegistryResult,
) -> BTreeMap<String, Vec<Constructor>> {
    let mut constructors = BTreeMap::<String, Vec<Constructor>>::new();
    for member in production_members {
        let files = match collect_rs_files(&member.src_root) {
            Ok(files) => files,
            Err(error) => {
                result.tool_errors.push(format!(
                    "limitation registry cannot read {}: {error}",
                    member.src_root.display()
                ));
                continue;
            }
        };
        for path in files {
            let file = crate::metadata::relative_display(workspace_root, &path);
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) => {
                    result
                        .tool_errors
                        .push(format!("limitation registry cannot read {file}: {error}"));
                    continue;
                }
            };
            let syntax = match syn::parse_file(&source) {
                Ok(syntax) => syntax,
                Err(error) => {
                    result.tool_errors.push(format!(
                        "limitation registry syn parse error in {file}: {error}"
                    ));
                    continue;
                }
            };
            let mut visitor = ConstructorVisitor::new(&member.name, &file);
            visitor.visit_file(&syntax);
            result.violations.extend(visitor.violations);
            for (variant, found) in visitor.constructors {
                constructors.entry(variant).or_default().extend(found);
            }
        }
    }
    constructors
}

fn validate_constructors(
    model_variants: &BTreeSet<String>,
    registry: &BTreeMap<String, &RegistryRow>,
    constructors: &BTreeMap<String, Vec<Constructor>>,
    violations: &mut Vec<String>,
) {
    for variant in model_variants {
        if !constructors.contains_key(variant) {
            violations.push(format!(
                "LIMITATION REGISTRY UNUSED: Limitation::{variant} has no production constructor"
            ));
        }
    }
    for (variant, found) in constructors {
        let Some(row) = registry.get(variant) else {
            violations.push(format!(
                "LIMITATION REGISTRY UNMAPPED CONSTRUCTOR: Limitation::{variant} is emitted without a row"
            ));
            continue;
        };
        let expected_owner = owner_package(&row.owner);
        for constructor in found {
            if constructor.owner != expected_owner {
                violations.push(format!(
                    "LIMITATION REGISTRY OWNER VIOLATION: Limitation::{variant} emitted by {} in {} but owned by {expected_owner}",
                    constructor.owner, constructor.file
                ));
            }
        }
    }
}

struct ConstructorVisitor<'a> {
    owner: &'a str,
    file: &'a str,
    constructors: BTreeMap<String, Vec<Constructor>>,
    violations: Vec<String>,
}

impl<'a> ConstructorVisitor<'a> {
    fn new(owner: &'a str, file: &'a str) -> Self {
        Self {
            owner,
            file,
            constructors: BTreeMap::new(),
            violations: Vec::new(),
        }
    }
}

impl<'ast> Visit<'ast> for ConstructorVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        collect_limitation_aliases(&node.tree, &mut self.violations, self.owner, self.file);
        syn::visit::visit_item_use(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        if matches!(
            node.ty.as_ref(),
            syn::Type::Path(path)
                if path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Limitation")
        ) {
            self.violations.push(format!(
                "LIMITATION REGISTRY TYPE ALIAS: {} aliases Limitation as {} in {}",
                self.owner, node.ident, self.file
            ));
        }
        syn::visit::visit_item_type(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if token_stream_mentions_limitation(&node.tokens.to_string()) {
            let macro_name = node.path.segments.last().map_or_else(
                || "<unknown>".to_owned(),
                |segment| segment.ident.to_string(),
            );
            self.violations.push(format!(
                "LIMITATION REGISTRY MACRO REFERENCE: {} references Limitation inside {}! in {}; bind or match it outside the macro so ownership remains inspectable",
                self.owner,
                macro_name,
                self.file
            ));
        }
        syn::visit::visit_macro(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        let segments = node.path.segments.iter().collect::<Vec<_>>();
        if segments.len() >= 2 && segments[segments.len() - 2].ident == "Limitation" {
            let variant = segments[segments.len() - 1].ident.to_string();
            self.constructors
                .entry(variant)
                .or_default()
                .push(Constructor {
                    owner: self.owner.to_owned(),
                    file: self.file.to_owned(),
                });
        }
        syn::visit::visit_expr_struct(self, node);
    }
}

fn collect_limitation_aliases(
    tree: &syn::UseTree,
    violations: &mut Vec<String>,
    owner: &str,
    file: &str,
) {
    match tree {
        syn::UseTree::Rename(rename) if rename.ident == "Limitation" => violations.push(format!(
            "LIMITATION REGISTRY ALIAS: {owner} aliases Limitation as {} in {file}",
            rename.rename
        )),
        syn::UseTree::Path(path) if path.ident == "Limitation" => violations.push(format!(
            "LIMITATION REGISTRY VARIANT IMPORT: {owner} imports a Limitation variant in {file}; construct through Limitation::Variant"
        )),
        syn::UseTree::Path(path) => collect_limitation_aliases(&path.tree, violations, owner, file),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_limitation_aliases(item, violations, owner, file);
            }
        }
        syn::UseTree::Name(_) | syn::UseTree::Rename(_) | syn::UseTree::Glob(_) => {}
    }
}

fn token_stream_mentions_limitation(tokens: &str) -> bool {
    tokens.contains("Limitation ::")
}

fn is_cfg_test_only(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let Ok(meta) = attr.parse_args::<syn::Meta>() else {
        return false;
    };
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => list
            .tokens
            .to_string()
            .split([',', ' ', '(', ')'])
            .any(|part| part == "test"),
        _ => false,
    }
}

fn collect_rs_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    collect_rs_files_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_rs_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "tests" {
                collect_rs_files_recursive(&path, files)?;
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") && name != "tests.rs"
        {
            files.push(path);
        }
    }
    Ok(())
}

fn workspace_path(workspace_root: &Path, relative: &str) -> PathBuf {
    workspace_root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests;
