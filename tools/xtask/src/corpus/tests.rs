use super::*;

#[test]
fn parse_default() -> Result<(), String> {
    let a = parse_args(&[])?;
    assert_eq!(a.mode, CorpusMode::Standard);
    assert_eq!(a.format, OutputFormat::Human);
    assert_eq!(a.row, None);
    Ok(())
}

#[test]
fn parse_det_json() -> Result<(), String> {
    let a = parse_args(&["--determinism".into(), "--format".into(), "json".into()])?;
    assert_eq!(a.mode, CorpusMode::Determinism);
    assert_eq!(a.format, OutputFormat::Json);
    assert_eq!(a.row, None);
    Ok(())
}

#[test]
fn parse_crash() -> Result<(), String> {
    let a = parse_args(&["--store-crash".into()])?;
    assert_eq!(a.mode, CorpusMode::StoreCrash);
    Ok(())
}

#[test]
fn conflicting() {
    assert!(parse_args(&["--determinism".into(), "--store-crash".into()]).is_err());
}

#[test]
fn unknown_arg() {
    assert!(parse_args(&["--bogus".into()]).is_err());
}

#[test]
fn parse_one_row() -> Result<(), String> {
    let args = parse_args(&["--row".into(), "plain-esm".into()])?;
    assert_eq!(args.row.as_deref(), Some("plain-esm"));
    Ok(())
}

#[test]
fn duplicate_or_missing_row_is_rejected() {
    assert!(parse_args(&["--row".into()]).is_err());
    assert!(
        parse_args(&[
            "--row".into(),
            "plain-esm".into(),
            "--row".into(),
            "vue-entry".into(),
        ])
        .is_err()
    );
}

#[test]
fn unknown_fmt() {
    assert!(parse_args(&["--format".into(), "yaml".into()]).is_err());
}

#[test]
fn missing_fmt() {
    assert!(parse_args(&["--format".into()]).is_err());
}

#[test]
fn registry_90() {
    assert_eq!(REGISTRY.len(), 90);
}

#[test]
fn registry_valid() -> Result<(), String> {
    validate_registry()
}

#[test]
fn registry_unique() {
    let mut s = std::collections::HashSet::new();
    for r in REGISTRY {
        assert!(s.insert(r.id), "dup {}", r.id);
    }
}

#[test]
fn registry_no_empty_invocations() {
    for r in REGISTRY {
        for mode in [
            CorpusMode::Standard,
            CorpusMode::Determinism,
            CorpusMode::StoreCrash,
        ] {
            if let Some(invs) = r.mode_invocations(mode) {
                for i in invs {
                    assert!(
                        !i.target.is_empty(),
                        "empty target in {} mode {}",
                        r.id,
                        mode
                    );
                    assert!(
                        !i.filter.is_empty(),
                        "empty filter in {} mode {}",
                        r.id,
                        mode
                    );
                }
            }
        }
    }
}

#[test]
fn standard_has_applicable_rows() {
    let count = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::Standard))
        .count();
    assert!(count > 50, "standard applicable: {count}");
}

#[test]
fn determinism_has_applicable_rows() {
    let count = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::Determinism))
        .count();
    assert!(count > 50, "determinism applicable: {count}");
}

#[test]
fn determinism_has_unmapped_rows() {
    let unmapped = REGISTRY
        .iter()
        .filter(|r| {
            r.is_applicable(CorpusMode::Determinism) && !r.is_mapped(CorpusMode::Determinism)
        })
        .count();
    assert!(unmapped > 0, "determinism should have unmapped rows, got 0");
}

#[test]
fn every_mapped_standard_row_has_a_paired_determinism_invocation() {
    let standard = REGISTRY
        .iter()
        .filter(|row| row.is_mapped(CorpusMode::Standard))
        .count();
    let determinism = REGISTRY
        .iter()
        .filter(|row| row.is_mapped(CorpusMode::Determinism))
        .count();
    assert_eq!(standard, 45);
    assert_eq!(determinism, standard);
}

#[test]
fn store_crash_has_applicable_rows() {
    let count = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::StoreCrash))
        .count();
    assert!(count >= 4, "store-crash applicable: {count}");
}

#[test]
fn spec_90() -> Result<(), String> {
    let ids = extract_spec_ids(include_str!("../../../../specs/001-foundation-slice.md"))?;
    assert_eq!(ids.len(), 90, "{ids:?}");
    Ok(())
}

#[test]
fn spec_order() -> Result<(), String> {
    let ids = extract_spec_ids(include_str!("../../../../specs/001-foundation-slice.md"))?;
    let reg: Vec<&str> = REGISTRY.iter().map(|r| r.id).collect();
    assert_eq!(ids, reg);
    Ok(())
}

#[test]
fn marker_ok() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "row-a\nrow-a\nrow-a\n").map_err(|e| e.to_string())?;
    validate_marker(&p, "row-a", 3)
}

#[test]
fn marker_excess_ok() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "row-a\nrow-a\nrow-a\nrow-a\n").map_err(|e| e.to_string())?;
    validate_marker(&p, "row-a", 3)
}

#[test]
fn marker_short() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "row-a\n").map_err(|e| e.to_string())?;
    assert!(validate_marker(&p, "row-a", 3).is_err());
    Ok(())
}

#[test]
fn marker_missing() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    assert!(validate_marker(&d.path().join("x"), "row-a", 1).is_err());
    Ok(())
}

#[test]
fn marker_wrong_id() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "wrong-id\nwrong-id\n").map_err(|e| e.to_string())?;
    assert!(validate_marker(&p, "row-a", 2).is_err());
    Ok(())
}

#[test]
fn marker_mixed_ids() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "row-a\nrow-b\n").map_err(|e| e.to_string())?;
    assert!(validate_marker(&p, "row-a", 2).is_err());
    Ok(())
}

#[test]
fn marker_empty_line() -> Result<(), String> {
    let d = tempfile::tempdir().map_err(|e| e.to_string())?;
    let p = d.path().join("m");
    fs::write(&p, "row-a\n\nrow-a\n").map_err(|e| e.to_string())?;
    assert!(validate_marker(&p, "row-a", 2).is_err());
    Ok(())
}

#[test]
fn feat_flags() {
    assert!(FeatureSet::None.cargo_features().is_empty());
    assert_eq!(
        FeatureSet::PublicationAndRetentionCrash
            .cargo_features()
            .len(),
        2
    );
}

#[test]
fn bnq_has_12_invocations() {
    let invocation_count = REGISTRY
        .iter()
        .find(|row| row.id == "bounded-nested-query")
        .and_then(|row| row.mode_invocations(CorpusMode::Standard))
        .map(<[_]>::len);
    assert_eq!(invocation_count, Some(12));
}

#[test]
fn mode_counts() {
    let std_applicable = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::Standard))
        .count();
    let det_applicable = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::Determinism))
        .count();
    let crash_applicable = REGISTRY
        .iter()
        .filter(|r| r.is_applicable(CorpusMode::StoreCrash))
        .count();
    let std_mapped = REGISTRY
        .iter()
        .filter(|r| r.is_mapped(CorpusMode::Standard))
        .count();
    let det_mapped = REGISTRY
        .iter()
        .filter(|r| r.is_mapped(CorpusMode::Determinism))
        .count();
    let crash_mapped = REGISTRY
        .iter()
        .filter(|r| r.is_mapped(CorpusMode::StoreCrash))
        .count();
    // All modes must have >0 applicable.
    assert!(std_applicable > 0);
    assert!(det_applicable > 0);
    assert!(crash_applicable > 0);
    // Report for task closeout (not actual assertion failure).
    eprintln!(
        "Standard:     applicable={std_applicable} mapped={std_mapped} unmapped={}",
        std_applicable - std_mapped
    );
    eprintln!(
        "Determinism:  applicable={det_applicable} mapped={det_mapped} unmapped={}",
        det_applicable - det_mapped
    );
    eprintln!(
        "StoreCrash:   applicable={crash_applicable} mapped={crash_mapped} unmapped={}",
        crash_applicable - crash_mapped
    );
}
