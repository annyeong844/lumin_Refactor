#[path = "support/retention.rs"]
mod retention_support;
#[path = "retention_faults/support.rs"]
mod support;

use support::{DurableState, Fixture, assert_status, run, run_with_crash};

#[test]
fn plan_commit_death_leaves_no_partial_plan_or_operation() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::runs_without_plan()?;
    let crashed = run_with_crash(
        fixture.root(),
        &Fixture::run_plan_arguments(),
        "before-prepared-commit",
    )?;
    assert_status(&crashed, support::CRASH_EXIT_CODE);
    fixture.assert_operation_absent(Fixture::PLAN_OPERATION_ID)?;
    fixture.assert_run_catalog_before_pruning()?;

    let prepared = run(fixture.root(), &Fixture::run_plan_arguments())?;
    assert_status(&prepared, 0);
    let fixture = fixture.with_plan(&prepared.stdout)?;
    fixture.assert_state(DurableState::Prepared)?;
    Ok(())
}

#[test]
fn run_retention_recovers_every_physical_crash_boundary() -> Result<(), Box<dyn std::error::Error>>
{
    let sample = Fixture::runs()?;
    let physical_moves = sample.physical_move_count();
    assert!(physical_moves > 0);
    drop(sample);

    for (point, expected) in run_crash_points(physical_moves) {
        println!("retention crash point: {point}");
        let fixture = Fixture::runs()?;
        fixture.crash_confirm(&point)?;
        fixture.assert_state(expected)?;
        fixture.recover_and_assert_final_truth()?;
    }
    Ok(())
}

#[test]
fn gate_retention_recovers_logical_crash_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for (point, expected) in [
        ("before-pruning-commit", DurableState::Prepared),
        ("after-pruning-commit", DurableState::Pruning),
        ("after-pruned-commit", DurableState::Pruned),
    ] {
        let fixture = Fixture::gate()?;
        fixture.crash_confirm(point)?;
        fixture.assert_state(expected)?;
        fixture.recover_and_assert_final_truth()?;
    }
    Ok(())
}

#[test]
fn unknown_crash_selector_fails_before_retention_mutation() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::runs()?;
    let output = run_with_crash(
        fixture.root(),
        &fixture.confirm_arguments(),
        "not-a-retention-crash-point",
    )?;
    assert_status(&output, support::INVALID_SELECTOR_EXIT_CODE);
    fixture.assert_state(DurableState::Prepared)?;
    Ok(())
}

fn run_crash_points(physical_moves: usize) -> Vec<(String, DurableState)> {
    let mut points = vec![
        ("before-pruning-commit".to_owned(), DurableState::Prepared),
        ("after-pruning-commit".to_owned(), DurableState::Pruning),
    ];
    points.extend(
        (0..physical_moves)
            .map(|index| (format!("after-payload-move-{index}"), DurableState::Pruning)),
    );
    points.push(("after-moves-committed".to_owned(), DurableState::Pruning));
    points.push(("after-pruned-commit".to_owned(), DurableState::Pruned));
    points.extend(
        (0..physical_moves)
            .map(|index| (format!("after-reclaim-child-{index}"), DurableState::Pruned)),
    );
    points.extend([
        (
            "after-reclaim-payloads-flushed".to_owned(),
            DurableState::Pruned,
        ),
        (
            "after-reclaim-anchor-removed".to_owned(),
            DurableState::Pruned,
        ),
        (
            "after-reclaim-directory-removed".to_owned(),
            DurableState::Pruned,
        ),
        (
            "after-reclaim-parent-flushed".to_owned(),
            DurableState::Pruned,
        ),
    ]);
    points
}
