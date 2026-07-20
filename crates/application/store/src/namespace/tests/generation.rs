use std::fs;

use crate::{AttemptEnvelope, AttemptState, StoreError, StoreGeneration, io_error, read_json};

use super::super::platform::HeldEntry;
use super::super::store_header::initialize_store;
use super::super::{NamespaceState, entry_exists, require_state_volume};
use super::open_store;

impl NamespaceState {
    pub(crate) fn replace_store_generation_for_test(&self) -> Result<StoreGeneration, StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let generation = database.generation().checked_next().ok_or_else(|| {
                StoreError::Integrity("lifecycle store generation overflow".to_owned())
            })?;
            drop(database);

            let current_path = self.state_dir.join("lifecycle.store");
            let replacement_path = self.state_dir.join("lifecycle.store.replacement-test");
            let displaced_path = self.state_dir.join("lifecycle.store.displaced-test");
            if entry_exists(&replacement_path)? || entry_exists(&displaced_path)? {
                return Err(StoreError::Integrity(
                    "generation replacement test paths are not empty".to_owned(),
                ));
            }
            let replacement =
                HeldEntry::create_new(&replacement_path, "test lifecycle.store replacement")?;
            require_state_volume(
                &replacement,
                &guard.state_directory,
                "test lifecycle.store replacement",
            )?;
            initialize_store(replacement, &self.binding, generation)?;
            guard.state_directory.sync_directory()?;

            fs::rename(&current_path, &displaced_path).map_err(io_error)?;
            if let Err(error) = fs::rename(&replacement_path, &current_path) {
                let restore = fs::rename(&displaced_path, &current_path);
                return match restore {
                    Ok(()) => Err(io_error(error)),
                    Err(restore_error) => Err(StoreError::Integrity(format!(
                        "test store replacement and restore failed: {error}; {restore_error}"
                    ))),
                };
            }
            guard.state_directory.sync_directory()?;
            fs::remove_file(displaced_path).map_err(io_error)?;
            guard.state_directory.sync_directory()?;

            let current = guard.open_database_for_generation(generation)?;
            drop(current);
            Ok(generation)
        })
    }
}

#[test]
fn old_generation_attempt_cannot_publish_a_terminal_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    assert_eq!(attempt.generation, StoreGeneration::INITIAL);

    let observed = store.namespace.replace_store_generation_for_test()?;
    assert!(matches!(
        store.fail_attempt(&attempt, "must remain running"),
        Err(StoreError::StoreGenerationChanged {
            expected,
            observed: actual,
        }) if expected == StoreGeneration::INITIAL && actual == observed
    ));

    let envelope: AttemptEnvelope = read_json(
        &root
            .path()
            .join(".lumin/attempts")
            .join(attempt.attempt_id.as_str())
            .join("attempt.json"),
    )?;
    assert!(matches!(envelope.state, AttemptState::Running));
    assert!(envelope.failure.is_none());
    Ok(())
}
