use super::*;

const MODEL: &str = r#"
pub enum Limitation {
    Alpha { detail: String },
    Beta { detail: String },
}

define_limitation_registry! {
    Alpha => {
        owner: Js,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredEvidence,
    },
    Beta => {
        owner: Resolve,
        scope: OwningPackage,
        absence: PackageConsumers,
        gate: RequiredEvidence,
    },
}
"#;

#[test]
fn parses_single_source_registry() -> Result<(), Box<dyn std::error::Error>> {
    let (variants, rows) = parse_model_contract(MODEL).map_err(std::io::Error::other)?;
    assert_eq!(
        variants,
        BTreeSet::from(["Alpha".to_owned(), "Beta".to_owned()])
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].owner, "Js");
    Ok(())
}

#[test]
fn missing_model_row_is_a_violation() -> Result<(), Box<dyn std::error::Error>> {
    let (variants, mut rows) = parse_model_contract(MODEL).map_err(std::io::Error::other)?;
    let spec = spec_rows(&rows);
    rows.pop();
    let members = vec![member("lumin-js"), member("lumin-resolve")];
    let mut violations = Vec::new();
    validate_contract(&variants, &rows, &spec, &members, &mut violations);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("Limitation::Beta"))
    );
    Ok(())
}

#[test]
fn scope_absence_and_gate_drift_are_independently_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let (variants, rows) = parse_model_contract(MODEL).map_err(std::io::Error::other)?;
    let members = vec![member("lumin-js"), member("lumin-resolve")];
    for (dimension, mutate) in [
        ("SCOPE DRIFT", 0_usize),
        ("ABSENCE DRIFT", 1_usize),
        ("GATE DRIFT", 2_usize),
    ] {
        let mut spec = spec_rows(&rows);
        let alpha = spec
            .get_mut("Alpha")
            .ok_or_else(|| std::io::Error::other("Alpha spec row is missing"))?;
        match mutate {
            0 => alpha.scope = "WrongScope".to_owned(),
            1 => alpha.absence = "WrongAbsence".to_owned(),
            2 => alpha.gate = "WrongGate".to_owned(),
            _ => unreachable!(),
        }
        let mut violations = Vec::new();
        validate_contract(&variants, &rows, &spec, &members, &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains(dimension)),
            "missing {dimension}: {violations:?}"
        );
    }
    Ok(())
}

#[test]
fn constructor_owner_mismatch_is_a_violation() -> Result<(), Box<dyn std::error::Error>> {
    let syntax =
        syn::parse_file("fn build() { let _ = Limitation::Alpha { detail: String::new() }; }")?;
    let mut visitor = ConstructorVisitor::new("lumin-engine", "src/lib.rs");
    visitor.visit_file(&syntax);
    let registry_row = RegistryRow {
        variant: "Alpha".to_owned(),
        owner: "Js".to_owned(),
        scope: "Workspace".to_owned(),
        absence: "WorkspaceConsumers".to_owned(),
        gate: "RequiredEvidence".to_owned(),
    };
    let registry = BTreeMap::from([("Alpha".to_owned(), &registry_row)]);
    let mut violations = Vec::new();
    validate_constructors(
        &BTreeSet::from(["Alpha".to_owned()]),
        &registry,
        &visitor.constructors,
        &mut violations,
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("OWNER VIOLATION"))
    );
    Ok(())
}

#[test]
fn limitation_alias_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let syntax = syn::parse_file("use lumin_model::Limitation as Gap;")?;
    let mut visitor = ConstructorVisitor::new("lumin-engine", "src/lib.rs");
    visitor.visit_file(&syntax);
    assert_eq!(visitor.violations.len(), 1);
    assert!(visitor.violations[0].contains("aliases Limitation as Gap"));
    Ok(())
}

#[test]
fn limitation_variant_import_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let syntax = syn::parse_file("use lumin_model::Limitation::Alpha;")?;
    let mut visitor = ConstructorVisitor::new("lumin-engine", "src/lib.rs");
    visitor.visit_file(&syntax);
    assert_eq!(visitor.violations.len(), 1);
    assert!(visitor.violations[0].contains("VARIANT IMPORT"));
    Ok(())
}

#[test]
fn limitation_construction_inside_macro_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let syntax = syn::parse_file(
        "fn build() { let _ = vec![Limitation::Alpha { detail: String::new() }]; }",
    )?;
    let mut visitor = ConstructorVisitor::new("lumin-js", "src/lib.rs");
    visitor.visit_file(&syntax);
    assert_eq!(visitor.violations.len(), 1);
    assert!(visitor.violations[0].contains("MACRO REFERENCE"));
    Ok(())
}

#[test]
fn limitation_pattern_inside_macro_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let syntax = syn::parse_file(
        "fn inspect(value: Limitation) { let _ = matches!(value, Limitation::Alpha { .. }); }",
    )?;
    let mut visitor = ConstructorVisitor::new("lumin-js", "src/lib.rs");
    visitor.visit_file(&syntax);
    assert_eq!(visitor.violations.len(), 1);
    assert!(visitor.violations[0].contains("MACRO REFERENCE"));
    Ok(())
}

#[test]
fn limitation_type_alias_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let syntax = syn::parse_file("type Gap = lumin_model::Limitation;")?;
    let mut visitor = ConstructorVisitor::new("lumin-engine", "src/lib.rs");
    visitor.visit_file(&syntax);
    assert_eq!(visitor.violations.len(), 1);
    assert!(visitor.violations[0].contains("TYPE ALIAS"));
    Ok(())
}

fn member(name: &str) -> crate::metadata::WorkspaceMember {
    crate::metadata::WorkspaceMember {
        name: name.to_owned(),
        manifest_path: PathBuf::new(),
        src_root: PathBuf::new(),
    }
}

fn spec_rows(rows: &[RegistryRow]) -> BTreeMap<String, SpecRow> {
    rows.iter()
        .map(|row| {
            (
                row.variant.clone(),
                SpecRow {
                    owner: owner_package(&row.owner),
                    scope: row.scope.clone(),
                    absence: row.absence.clone(),
                    gate: row.gate.clone(),
                },
            )
        })
        .collect()
}
