use lumin_evidence::{
    RetentionItemKind, RetentionPlanItem, RetentionPlanRecord, RetentionPlanScope,
    RetentionPlanState,
};
use lumin_model::{RepositoryId, RetentionContentIdentity, RetentionPlanId, RunId};

use super::*;

#[test]
fn retention_cursor_resumes_after_exact_immutable_item() -> Result<(), Box<dyn std::error::Error>> {
    let plan = plan_with_items(101);
    let first = retention_plan_response(&plan, None)?;
    assert_eq!(first.returned, 100);
    assert!(first.truncated);
    let cursor = first.next_cursor.as_deref().ok_or("cursor is missing")?;
    let second = retention_plan_response(&plan, Some(cursor))?;
    assert_eq!(second.returned, 1);
    assert!(!second.truncated);
    assert_ne!(first.items[99], second.items[0]);

    let mut changed = plan;
    changed.content_identity =
        RetentionContentIdentity::from_string("retention_content_changed".to_owned());
    assert!(matches!(
        retention_plan_response(&changed, Some(cursor)),
        Err(ProtocolError::CursorScopeMismatch)
    ));
    Ok(())
}

#[test]
fn run_catalog_cursor_rejects_a_changed_revision() -> Result<(), Box<dyn std::error::Error>> {
    let runs = (0..101)
        .rev()
        .map(|index| RunCatalogItemDto {
            attempt_id: lumin_model::AttemptId::from_string(format!("attempt_{index:016x}")),
            run_id: RunId::from_string(format!("run_{index:016x}")),
            sequence: index,
        })
        .collect::<Vec<_>>();
    let first = run_catalog_response(7, &runs, None)?;
    assert_eq!(first.returned, 100);
    let cursor = first.next_cursor.as_deref().ok_or("cursor is missing")?;
    let second = run_catalog_response(7, &runs, Some(cursor))?;
    assert_eq!(second.returned, 1);
    assert!(matches!(
        run_catalog_response(8, &runs, Some(cursor)),
        Err(ProtocolError::CursorStale)
    ));
    Ok(())
}

fn plan_with_items(count: usize) -> RetentionPlanRecord {
    RetentionPlanRecord {
        schema_version: "lumin-retention-plan.v1".to_owned(),
        repository_id: RepositoryId::from_string("repository-test".to_owned()),
        plan_id: RetentionPlanId::from_string("retention_plan_test".to_owned()),
        content_identity: RetentionContentIdentity::from_string(
            "retention_content_test".to_owned(),
        ),
        scope: RetentionPlanScope::Runs {
            before_unix_millis: 1,
        },
        created_unix_millis: 1,
        catalog_revision: 1,
        state: RetentionPlanState::Prepared,
        items: (0..count)
            .map(|index| RetentionPlanItem {
                kind: RetentionItemKind::Run,
                owning_sequence: index as u64,
                record_id: format!("run_{index:016x}"),
                identity_sha256: format!("identity-{index:03}"),
                byte_count: index as u64,
            })
            .collect(),
        exclusions: Vec::new(),
        confirmation_operation_id: None,
        recoverable_state: None,
        tombstone_identity: None,
        physical_reclamation_pending: false,
    }
}
