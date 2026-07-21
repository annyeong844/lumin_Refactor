use lumin_evidence::{
    RetentionExclusionReason, RetentionItemKind, RetentionPlanExclusion, RetentionPlanItem,
    RetentionPlanRecord, RetentionPlanScope, RetentionPlanState,
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
    let cursor_payload: serde_json::Value = crate::cursor::decode_cursor_payload(cursor)?;
    assert_eq!(cursor_payload["schemaVersion"], "lumin-retention-cursor.v2");
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
fn retention_cursor_pages_exclusions_with_the_same_bound() -> Result<(), Box<dyn std::error::Error>>
{
    let mut plan = plan_with_items(0);
    plan.exclusions = (0..101)
        .map(|index| RetentionPlanExclusion {
            kind: RetentionItemKind::Run,
            record_id: format!("run_{index:016x}"),
            reason: RetentionExclusionReason::LatestCompleted,
        })
        .collect();
    let first = retention_plan_response(&plan, None)?;
    assert_eq!(first.total, 101);
    assert_eq!(first.returned, 100);
    assert!(first.items.is_empty());
    assert_eq!(first.exclusions.len(), 100);
    assert!(first.truncated);
    let cursor = first.next_cursor.as_deref().ok_or("cursor is missing")?;
    let second = retention_plan_response(&plan, Some(cursor))?;
    assert_eq!(second.total, 101);
    assert_eq!(second.returned, 1);
    assert!(second.items.is_empty());
    assert_eq!(second.exclusions.len(), 1);
    assert!(!second.truncated);
    assert_ne!(first.exclusions[99], second.exclusions[0]);
    Ok(())
}

#[test]
fn run_catalog_cursor_binds_repository_revision_and_anchor()
-> Result<(), Box<dyn std::error::Error>> {
    let runs = (0..101)
        .rev()
        .map(|index| RunCatalogItemDto {
            attempt_id: lumin_model::AttemptId::from_string(format!("attempt_{index:016x}")),
            run_id: RunId::from_string(format!("run_{index:016x}")),
            sequence: index,
        })
        .collect::<Vec<_>>();
    let repository_id = RepositoryId::from_string("repository-runs".to_owned());
    let first = run_catalog_response(
        repository_id.clone(),
        7,
        runs.len(),
        runs[..100].to_vec(),
        true,
    )?;
    assert_eq!(first.returned, 100);
    let cursor = first.next_cursor.as_deref().ok_or("cursor is missing")?;
    let cursor_payload: serde_json::Value = crate::cursor::decode_cursor_payload(cursor)?;
    assert_eq!(cursor_payload["schemaVersion"], "lumin-runs-cursor.v2");
    let decoded = decode_run_catalog_cursor(cursor)?;
    assert_eq!(decoded.repository_id, repository_id);
    assert_eq!(decoded.revision, 7);
    assert_eq!(decoded.last_run, runs[99]);
    let second = run_catalog_response(
        decoded.repository_id,
        decoded.revision,
        runs.len(),
        runs[100..].to_vec(),
        false,
    )?;
    assert_eq!(second.returned, 1);
    assert!(!second.truncated);
    assert!(second.next_cursor.is_none());
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
