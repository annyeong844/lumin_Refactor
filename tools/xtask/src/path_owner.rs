//! Path identity ownership checks for production crates.
//!
//! The model owns path/root identity types. Protocol owns their wire DTOs.
//! Native OS paths are lowered into canonical identities only by model codec
//! internals or inventory, which is the value authority for repository paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

const MODEL_OWNER: &str = "lumin-model";
const INVENTORY_OWNER: &str = "lumin-inventory";
const PROTOCOL_OWNER: &str = "lumin-protocol";
const MODEL_OR_INVENTORY: &[&str] = &[MODEL_OWNER, INVENTORY_OWNER];
const INVENTORY_ONLY: &[&str] = &[INVENTORY_OWNER];

const REQUIRED_DECLARATIONS: &[(&str, &str)] = &[
    ("RepoPath", MODEL_OWNER),
    ("PhysicalFileIdentity", MODEL_OWNER),
    ("PhysicalAliasWriteClosure", MODEL_OWNER),
    ("RepositoryRootPhysicalIdentity", MODEL_OWNER),
    ("RepositoryRootIdentity", MODEL_OWNER),
    ("RepositoryBinding", MODEL_OWNER),
    ("RepoPathDto", PROTOCOL_OWNER),
];

// `RepositoryRootDto` is contract-owned by protocol but lands with the
// separately tracked codec DTO/runtime slice. Until then, a declaration is
// optional but may not appear under another owner.
const OPTIONAL_DECLARATIONS: &[(&str, &str)] = &[("RepositoryRootDto", PROTOCOL_OWNER)];

#[derive(Debug, Default)]
pub struct PathOwnerResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

#[derive(Debug)]
struct Declaration {
    owner: String,
    file: String,
    kind: &'static str,
}

struct PathOwnerVisitor<'a> {
    owner: &'a str,
    file: &'a str,
    declarations: BTreeMap<String, Vec<Declaration>>,
    violations: Vec<String>,
}

impl<'a> PathOwnerVisitor<'a> {
    fn new(owner: &'a str, file: &'a str) -> Self {
        Self {
            owner,
            file,
            declarations: BTreeMap::new(),
            violations: Vec::new(),
        }
    }

    fn record_declaration(&mut self, ident: &syn::Ident, kind: &'static str) {
        let name = ident.to_string();
        let Some(expected_owner) = declaration_owner(&name) else {
            return;
        };
        self.declarations
            .entry(name.clone())
            .or_default()
            .push(Declaration {
                owner: self.owner.to_owned(),
                file: self.file.to_owned(),
                kind,
            });
        if self.owner != expected_owner {
            self.violations.push(format!(
                "PATH OWNER: {kind} {name} declared in {} ({}) but is owned by {expected_owner}",
                self.owner, self.file
            ));
        }
    }

    fn check_native_lowering_path(&mut self, expression: &syn::ExprPath) {
        let path = &expression.path;
        if path.segments.len() < 2 && path.leading_colon.is_none() && expression.qself.is_none() {
            return;
        }
        let Some(method) = path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
        else {
            return;
        };
        let allowed = match method.as_str() {
            "from_native_relative" => MODEL_OR_INVENTORY,
            "from_native_absolute" => INVENTORY_ONLY,
            _ => return,
        };
        if !allowed.contains(&self.owner) {
            self.violations.push(format!(
                "PATH OWNER: native identity lowering {method} referenced by {} ({}) outside {}",
                self.owner,
                self.file,
                allowed.join(" or ")
            ));
        }
    }
}

impl<'ast> Visit<'ast> for PathOwnerVisitor<'_> {
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

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_item_impl(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        self.record_declaration(&node.ident, "struct");
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        self.record_declaration(&node.ident, "enum");
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        self.record_declaration(&node.ident, "type");
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        self.record_declaration(&node.ident, "trait");
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast syn::ItemUnion) {
        if node.attrs.iter().any(is_cfg_test_only) {
            return;
        }
        self.record_declaration(&node.ident, "union");
        syn::visit::visit_item_union(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.check_native_lowering_path(node);
        syn::visit::visit_expr_path(self, node);
    }
}

fn declaration_owner(name: &str) -> Option<&'static str> {
    REQUIRED_DECLARATIONS
        .iter()
        .chain(OPTIONAL_DECLARATIONS)
        .find_map(|(candidate, owner)| (*candidate == name).then_some(*owner))
}

pub fn scan_path_ownership(
    members: &[crate::metadata::WorkspaceMember],
    workspace_root: &Path,
) -> PathOwnerResult {
    let mut result = PathOwnerResult::default();
    let mut declarations: BTreeMap<String, Vec<Declaration>> = BTreeMap::new();

    for member in members {
        if !member.src_root.exists() {
            result.tool_errors.push(format!(
                "path-owner src root missing for {}: {}",
                member.name,
                crate::metadata::relative_display(workspace_root, &member.src_root)
            ));
            continue;
        }
        let files = match collect_rs_files(&member.src_root) {
            Ok(files) => files,
            Err(error) => {
                result.tool_errors.push(format!(
                    "path-owner failed to read src root for {} ({}): {error}",
                    member.name,
                    crate::metadata::relative_display(workspace_root, &member.src_root)
                ));
                continue;
            }
        };
        for file_path in files {
            let file = crate::metadata::relative_display(workspace_root, &file_path);
            let source = match std::fs::read_to_string(&file_path) {
                Ok(source) => source,
                Err(error) => {
                    result
                        .tool_errors
                        .push(format!("path-owner cannot read {file}: {error}"));
                    continue;
                }
            };
            let syntax = match syn::parse_file(&source) {
                Ok(syntax) => syntax,
                Err(error) => {
                    result
                        .tool_errors
                        .push(format!("path-owner syn parse error in {file}: {error}"));
                    continue;
                }
            };
            let mut visitor = PathOwnerVisitor::new(&member.name, &file);
            visitor.visit_file(&syntax);
            result.violations.extend(visitor.violations);
            for (name, found) in visitor.declarations {
                declarations.entry(name).or_default().extend(found);
            }
        }
    }

    for (name, expected_owner) in REQUIRED_DECLARATIONS {
        match declarations
            .get(*name)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            [] => result.violations.push(format!(
                "PATH OWNER: required {name} declaration missing from {expected_owner}"
            )),
            [declaration] if declaration.owner == *expected_owner => {}
            found => {
                let locations = found
                    .iter()
                    .map(|item| format!("{} {} in {} ({})", item.kind, name, item.owner, item.file))
                    .collect::<Vec<_>>()
                    .join(", ");
                result.violations.push(format!(
                    "PATH OWNER: expected exactly one {name} declaration in {expected_owner}; found {locations}"
                ));
            }
        }
    }

    result.violations.sort();
    result.violations.dedup();
    result.tool_errors.sort();
    result.tool_errors.dedup();
    result
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
        syn::Meta::List(list) if list.path.is_ident("all") => {
            syn::parse2::<CfgAllArgs>(list.tokens)
                .map(|args| {
                    args.items
                        .iter()
                        .any(|item| matches!(item, syn::Meta::Path(path) if path.is_ident("test")))
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

struct CfgAllArgs {
    items: Vec<syn::Meta>,
}

impl syn::parse::Parse for CfgAllArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let items =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        Ok(Self {
            items: items.into_iter().collect(),
        })
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
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            collect_rs_files_recursive(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().is_none_or(|name| name != "tests.rs")
        {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit<'a>(owner: &'a str, source: &str) -> Result<PathOwnerVisitor<'a>, syn::Error> {
        let syntax = syn::parse_file(source)?;
        let mut visitor = PathOwnerVisitor::new(owner, "src/lib.rs");
        visitor.visit_file(&syntax);
        Ok(visitor)
    }

    #[test]
    fn rejects_path_identity_declaration_outside_its_owner() -> Result<(), syn::Error> {
        let visitor = visit("lumin-engine", "pub struct RepoPath { bytes: Vec<u8> }")?;
        assert_eq!(visitor.violations.len(), 1);
        assert!(visitor.violations[0].contains("owned by lumin-model"));
        Ok(())
    }

    #[test]
    fn accepts_path_identity_declaration_in_its_owner() -> Result<(), syn::Error> {
        let visitor = visit("lumin-model", "pub struct RepoPath { bytes: Vec<u8> }")?;
        assert!(visitor.violations.is_empty());
        assert_eq!(visitor.declarations["RepoPath"].len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_native_relative_lowering_outside_model_and_inventory() -> Result<(), syn::Error> {
        let visitor = visit(
            "lumin-engine",
            "fn lower(path: &std::path::Path) { let _ = Alias::from_native_relative(path); }",
        )?;
        assert_eq!(visitor.violations.len(), 1);
        assert!(visitor.violations[0].contains("from_native_relative"));
        Ok(())
    }

    #[test]
    fn rejects_native_root_lowering_outside_inventory() -> Result<(), syn::Error> {
        let visitor = visit(
            "lumin-model",
            "fn lower(path: &std::path::Path, physical: P) { let _ = RepositoryRootIdentity::from_native_absolute(path, physical); }",
        )?;
        assert_eq!(visitor.violations.len(), 1);
        assert!(visitor.violations[0].contains("from_native_absolute"));
        Ok(())
    }

    #[test]
    fn rejects_qualified_self_native_lowering_outside_inventory() -> Result<(), syn::Error> {
        let visitor = visit(
            "lumin-engine",
            "fn lower(path: &std::path::Path) { let _ = <RepoPath>::from_native_relative(path); }",
        )?;
        assert_eq!(visitor.violations.len(), 1);
        assert!(visitor.violations[0].contains("from_native_relative"));
        Ok(())
    }

    #[test]
    fn accepts_native_lowering_in_inventory() -> Result<(), syn::Error> {
        let visitor = visit(
            "lumin-inventory",
            "fn lower(path: &std::path::Path, physical: P) { let _ = RepoPath::from_native_relative(path); let _ = Root::from_native_absolute(path, physical); }",
        )?;
        assert!(visitor.violations.is_empty());
        Ok(())
    }

    #[test]
    fn ignores_test_only_declarations_and_calls() -> Result<(), syn::Error> {
        let visitor = visit(
            "lumin-engine",
            "#[cfg(test)] mod tests { struct RepoPath; fn test() { let _ = RepoPath::from_native_relative(todo!()); } }",
        )?;
        assert!(visitor.violations.is_empty());
        assert!(visitor.declarations.is_empty());
        Ok(())
    }
}
