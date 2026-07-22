use crate::{AttemptEnvelope, AttemptState, StoreError, StoreGeneration, read_json};

use super::open_store;

#[test]
fn old_generation_attempt_cannot_publish_a_terminal_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    assert_eq!(attempt.generation(), StoreGeneration::INITIAL);

    let observed = store.migrate_lifecycle_store()?;
    assert!(matches!(
        store.fail_attempt(&mut attempt, "must remain running"),
        Err(StoreError::StoreGenerationChanged {
            expected,
            observed: actual,
        }) if expected == StoreGeneration::INITIAL && actual == observed
    ));

    let envelope: AttemptEnvelope = read_json(
        &root
            .path()
            .join(".lumin/attempts")
            .join(attempt.attempt_id().as_str())
            .join("attempt.json"),
    )?;
    assert!(matches!(envelope.state, AttemptState::Running));
    assert!(envelope.failure.is_none());

    let attempt_id = attempt.attempt_id().clone();
    drop(attempt);
    drop(store);

    let reopened = open_store(root.path())?;
    let envelope: AttemptEnvelope = read_json(
        &root
            .path()
            .join(".lumin/attempts")
            .join(attempt_id.as_str())
            .join("attempt.json"),
    )?;
    assert!(matches!(envelope.state, AttemptState::Interrupted));
    assert_eq!(
        envelope.failure.as_deref(),
        Some("attempt owner process exited before terminal publication")
    );
    assert!(reopened.begin_attempt().is_ok());
    Ok(())
}
