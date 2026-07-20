use std::ops::Deref;

use redb::{Database, ReadTransaction, ReadableDatabase, WriteTransaction};

use crate::{StoreError, StoreGeneration, backend_error, io_error};

use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::store_header::{verify_store_header, verify_store_header_write};
use super::{NamespaceGuard, require_state_volume};

pub(crate) struct StoreDatabase<'guard> {
    guard: &'guard NamespaceGuard,
    entry: HeldEntry,
    database: Database,
    generation: StoreGeneration,
}

pub(crate) struct StoreReadTransaction<'database, 'guard> {
    read: ReadTransaction,
    _database: &'database StoreDatabase<'guard>,
}

pub(crate) struct StoreWriteTransaction<'database, 'guard> {
    write: WriteTransaction,
    database: &'database StoreDatabase<'guard>,
}

impl NamespaceGuard {
    pub(crate) fn open_database(&self) -> Result<StoreDatabase<'_>, StoreError> {
        self.validate_bound_entries()?;
        let entry = HeldEntry::open(
            &self.state.state_dir.join("lifecycle.store"),
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.store",
        )?;
        require_state_volume(&entry, &self.state_directory, "lifecycle.store")?;
        let database = Database::builder()
            .create_file(entry.file().try_clone().map_err(io_error)?)
            .map_err(backend_error)?;
        let generation = verify_store_header(&database, &self.state.binding)?;
        let database = StoreDatabase {
            guard: self,
            entry,
            database,
            generation,
        };
        database.validate_current()?;
        self.validate_bound_entries()?;
        Ok(database)
    }

    pub(crate) fn open_database_for_generation(
        &self,
        expected: StoreGeneration,
    ) -> Result<StoreDatabase<'_>, StoreError> {
        let database = self.open_database()?;
        database.require_generation(expected)?;
        Ok(database)
    }

    pub(crate) fn commit(
        &self,
        transaction: StoreWriteTransaction<'_, '_>,
    ) -> Result<(), StoreError> {
        let StoreWriteTransaction { write, database } = transaction;
        if !std::ptr::eq(self, database.guard) {
            return Err(StoreError::Integrity(
                "lifecycle transaction belongs to a different namespace guard".to_owned(),
            ));
        }
        self.validate_bound_entries()?;
        database.validate_current()?;
        verify_store_header_write(&write, &self.state.binding, database.generation)?;
        write.commit().map_err(backend_error)?;
        self.validate_bound_entries()?;
        database.validate_current()?;
        self.validate_bound_entries()
    }

    pub(crate) fn mutate_for_generation<T>(
        &self,
        generation: StoreGeneration,
        mutation: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.validate_generation(generation)?;
        let result = mutation();
        let validation = self.validate_generation(generation);
        match (result, validation) {
            (_, Err(error)) => Err(error),
            (result, Ok(())) => result,
        }
    }

    fn validate_generation(&self, generation: StoreGeneration) -> Result<(), StoreError> {
        self.validate_bound_entries()?;
        let database = self.open_database_for_generation(generation)?;
        drop(database);
        self.validate_bound_entries()
    }
}

impl<'guard> StoreDatabase<'guard> {
    pub(crate) fn generation(&self) -> StoreGeneration {
        self.generation
    }

    pub(crate) fn begin_read(&self) -> Result<StoreReadTransaction<'_, 'guard>, StoreError> {
        let read = self.database.begin_read().map_err(backend_error)?;
        Ok(StoreReadTransaction {
            read,
            _database: self,
        })
    }

    pub(crate) fn begin_write(&self) -> Result<StoreWriteTransaction<'_, 'guard>, StoreError> {
        let write = self.database.begin_write().map_err(backend_error)?;
        Ok(StoreWriteTransaction {
            write,
            database: self,
        })
    }

    fn require_generation(&self, expected: StoreGeneration) -> Result<(), StoreError> {
        if self.generation != expected {
            return Err(StoreError::StoreGenerationChanged {
                expected,
                observed: self.generation,
            });
        }
        Ok(())
    }

    fn validate_current(&self) -> Result<(), StoreError> {
        self.entry.validate_path(
            &self.guard.state.state_dir.join("lifecycle.store"),
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.store",
        )?;
        require_state_volume(&self.entry, &self.guard.state_directory, "lifecycle.store")?;
        let observed = verify_store_header(&self.database, &self.guard.state.binding)?;
        if observed != self.generation {
            return Err(StoreError::StoreGenerationChanged {
                expected: self.generation,
                observed,
            });
        }
        Ok(())
    }
}

impl Deref for StoreReadTransaction<'_, '_> {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.read
    }
}

impl Deref for StoreWriteTransaction<'_, '_> {
    type Target = WriteTransaction;

    fn deref(&self) -> &Self::Target {
        &self.write
    }
}
