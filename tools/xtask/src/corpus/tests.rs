use super::*;

#[test]
fn parse_default() -> Result<(), String> {
    let a = parse_args(&[])?;
    assert_eq!(a.mode, CorpusMode::Standard);
    assert_eq!(a.format, OutputFormat::Human);
    assert_eq!(a.row, None);
    assert_eq!(a.selection, CorpusSelection::AllApplicable);
    assert_eq!(a.row_jobs, 1);
    assert_eq!((a.row_shard_index, a.row_shard_count), (0, 1));
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
fn mapped_only_is_aggregate_only() -> Result<(), String> {
    let args = parse_args(&["--determinism".into(), "--mapped-only".into()])?;
    assert_eq!(args.mode, CorpusMode::Determinism);
    assert_eq!(args.selection, CorpusSelection::MappedOnly);
    assert!(parse_args(&["--mapped-only".into(), "--mapped-only".into()]).is_err());
    assert!(parse_args(&["--mapped-only".into(), "--row".into(), "plain-esm".into(),]).is_err());
    Ok(())
}

#[test]
fn row_jobs_are_explicit_and_bounded() -> Result<(), String> {
    let args = parse_args(&["--mapped-only".into(), "--row-jobs".into(), "8".into()])?;
    assert_eq!(args.row_jobs, 8);
    for invalid in ["0", "9", "many"] {
        assert!(parse_args(&["--row-jobs".into(), invalid.into()]).is_err());
    }
    assert!(
        parse_args(&[
            "--row-jobs".into(),
            "2".into(),
            "--row-jobs".into(),
            "3".into(),
        ])
        .is_err()
    );
    Ok(())
}

#[test]
fn row_shards_are_paired_bounded_and_exact() -> Result<(), String> {
    let args = parse_args(&[
        "--mapped-only".into(),
        "--row-shard-index".into(),
        "8".into(),
        "--row-shard-count".into(),
        "9".into(),
    ])?;
    assert_eq!((args.row_shard_index, args.row_shard_count), (8, 9));
    for invalid in [
        vec!["--row-shard-index", "0"],
        vec!["--row-shard-count", "4"],
        vec!["--row-shard-index", "9", "--row-shard-count", "9"],
        vec!["--row-shard-index", "0", "--row-shard-count", "17"],
    ] {
        assert!(parse_args(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
    }
    Ok(())
}

#[test]
fn mapped_only_selects_every_and_only_mapped_row() {
    for mode in [CorpusMode::Standard, CorpusMode::Determinism] {
        let args = CorpusArgs {
            mode,
            format: OutputFormat::Human,
            row: None,
            selection: CorpusSelection::MappedOnly,
            row_jobs: 1,
            row_shard_index: 0,
            row_shard_count: 1,
        };
        let selected = selected_rows(&args);
        let expected = REGISTRY.iter().filter(|row| row.is_mapped(mode)).count();
        assert_eq!(selected.len(), expected);
        assert!(!selected.is_empty());
        assert!(selected.iter().all(|row| row.is_mapped(mode)));
    }
}

#[test]
fn all_applicable_selection_retains_unmapped_rows() {
    for mode in [CorpusMode::Standard, CorpusMode::Determinism] {
        let args = CorpusArgs {
            mode,
            format: OutputFormat::Human,
            row: None,
            selection: CorpusSelection::AllApplicable,
            row_jobs: 1,
            row_shard_index: 0,
            row_shard_count: 1,
        };
        let selected = selected_rows(&args);
        let expected = REGISTRY
            .iter()
            .filter(|row| row.is_applicable(mode))
            .count();
        assert_eq!(selected.len(), expected);
        assert!(selected.iter().any(|row| !row.is_mapped(mode)));
    }
}

#[test]
fn ci_row_shards_cover_every_mapped_row_exactly_once() {
    for (mode, row_shard_count) in [(CorpusMode::Standard, 4), (CorpusMode::Determinism, 8)] {
        let mut observed = Vec::new();
        for row_shard_index in 0..row_shard_count {
            let args = CorpusArgs {
                mode,
                format: OutputFormat::Human,
                row: None,
                selection: CorpusSelection::MappedOnly,
                row_jobs: 4,
                row_shard_index,
                row_shard_count,
            };
            observed.extend(selected_rows(&args).into_iter().map(|row| row.id));
        }
        observed.sort_unstable();
        let mut expected = REGISTRY
            .iter()
            .filter(|row| row.is_mapped(mode))
            .map(|row| row.id)
            .collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(observed, expected);
    }
}

#[test]
fn ci_row_shards_balance_declared_work_deterministically() {
    for (mode, row_shard_count) in [(CorpusMode::Standard, 4), (CorpusMode::Determinism, 8)] {
        let loads = (0..row_shard_count)
            .map(|row_shard_index| {
                let args = CorpusArgs {
                    mode,
                    format: OutputFormat::Human,
                    row: None,
                    selection: CorpusSelection::MappedOnly,
                    row_jobs: 4,
                    row_shard_index,
                    row_shard_count,
                };
                let first = selected_rows(&args);
                let second = selected_rows(&args);
                assert_eq!(
                    first.iter().map(|row| row.id).collect::<Vec<_>>(),
                    second.iter().map(|row| row.id).collect::<Vec<_>>(),
                );
                first
                    .iter()
                    .map(|row| shard_weight(row, mode))
                    .sum::<usize>()
            })
            .collect::<Vec<_>>();
        let least = loads.iter().copied().fold(usize::MAX, usize::min);
        let most = loads.iter().copied().fold(0, usize::max);
        let heaviest_row = REGISTRY
            .iter()
            .filter(|row| row.is_mapped(mode))
            .map(|row| shard_weight(row, mode))
            .fold(1, usize::max);
        assert!(
            most - least <= heaviest_row,
            "unbalanced {mode} invocation loads: {loads:?}",
        );
        let expected = match mode {
            CorpusMode::Standard => vec![37, 37, 36, 36],
            CorpusMode::Determinism => vec![64, 21, 21, 21, 21, 21, 20, 20],
            CorpusMode::StoreCrash => unreachable!("CI does not shard store-crash rows"),
        };
        assert_eq!(loads, expected, "{mode} shard assignment changed");
    }

    let dedicated = selected_rows(&CorpusArgs {
        mode: CorpusMode::Determinism,
        format: OutputFormat::Human,
        row: None,
        selection: CorpusSelection::MappedOnly,
        row_jobs: 4,
        row_shard_index: 0,
        row_shard_count: 8,
    });
    assert_eq!(
        dedicated.iter().map(|row| row.id).collect::<Vec<_>>(),
        ["retention-plan-pagination"],
    );
}

#[test]
fn parallel_execution_preserves_registry_order_after_overtake() -> Result<(), String> {
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let (completed, observed) = mpsc::channel();
    thread::scope(|scope| {
        let work_gate = std::sync::Arc::clone(&gate);
        let work_completed = completed.clone();
        let execution = scope.spawn(move || {
            run_parallel_ordered(3, 3, move |index| {
                if index == 0 {
                    let (lock, condition) = &*work_gate;
                    let mut released = lock.lock().map_err(|error| error.to_string())?;
                    while !*released {
                        released = condition
                            .wait(released)
                            .map_err(|error| error.to_string())?;
                    }
                }
                work_completed
                    .send(index)
                    .map_err(|error| error.to_string())?;
                Ok(index)
            })
        });

        let mut overtakers = vec![
            observed.recv().map_err(|error| error.to_string())?,
            observed.recv().map_err(|error| error.to_string())?,
        ];
        overtakers.sort_unstable();
        assert_eq!(overtakers, [1, 2]);
        let (lock, condition) = &*gate;
        *lock.lock().map_err(|error| error.to_string())? = true;
        condition.notify_all();

        let ordered = execution
            .join()
            .map_err(|_| "ordered execution test worker panicked".to_owned())??;
        assert_eq!(ordered, [0, 1, 2]);
        Ok::<(), String>(())
    })?;
    Ok(())
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
fn registry_91() {
    assert_eq!(REGISTRY.len(), 91);
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
    assert_eq!(standard, 71);
    assert_eq!(determinism, standard);
}

#[test]
fn collection_ordering_remains_unmapped_without_perturbed_traversal_fixture() {
    let mapped_modes = REGISTRY
        .iter()
        .find(|row| row.id == "collection-ordering")
        .map(|row| {
            (
                row.is_mapped(CorpusMode::Standard),
                row.is_mapped(CorpusMode::Determinism),
            )
        });
    assert_eq!(mapped_modes, Some((false, false)));
}

#[test]
fn structural_rows_require_architecture_check() {
    let actual: Vec<&str> = REGISTRY
        .iter()
        .filter(|row| !row.required_checks.is_empty())
        .map(|row| row.id)
        .collect();
    assert_eq!(
        actual,
        vec![
            "resolver-config-registry-artifact",
            "pnpm-workspace-registry-and-precedence",
            "limitation-scope-exhaustiveness",
            "capability-availability-authority",
            "gate-lifecycle-effects",
        ]
    );
    for row in REGISTRY
        .iter()
        .filter(|row| !row.required_checks.is_empty())
    {
        assert_eq!(
            row.required_checks,
            &[RequiredCheck::ArchitectureCheck],
            "{}",
            row.id,
        );
    }
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
fn spec_91() -> Result<(), String> {
    let ids = extract_spec_ids(include_str!("../../../../specs/001-foundation-slice.md"))?;
    assert_eq!(ids.len(), 91, "{ids:?}");
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
        FeatureSet::LifecycleAndPublicationCrash.cargo_features(),
        &["lifecycle-test-fault", "publication-test-crash"]
    );
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
