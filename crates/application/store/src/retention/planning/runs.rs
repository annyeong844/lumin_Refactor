mod orphans;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use lumin_evidence::{
    RetentionExclusionReason, RetentionItemKind, RetentionPlanExclusion, RetentionPlanItem,
    RunPinRecord,
};
use lumin_model::PinId;
use redb::WriteTransaction;

use crate::namespace::NamespaceGuard;
use crate::{AttemptEnvelope, AttemptState, RUN_CATALOG, RunCatalogRecord, StoreError, io_error};

use super::super::RUN_PINS;
use super::{PlanContents, raw_pointer, read_raw_records, retention_item_from_bytes};
use orphans::{collect_orphan, collect_run_orphans, directory_children};

pub(super) use orphans::directory_payload_identity;

pub(super) fn collect(
    guard: &NamespaceGuard,
    write: &WriteTransaction,
    before_unix_millis: u64,
) -> Result<PlanContents, StoreError> {
    RunCollector::load(guard, write, before_unix_millis)?.collect()
}

struct RunCollector<'a> {
    guard: &'a NamespaceGuard,
    before_unix_millis: u64,
    runs: BTreeMap<String, (RunCatalogRecord, Vec<u8>)>,
    pins: BTreeMap<String, (RunPinRecord, Vec<u8>)>,
    latest_attempt: Option<String>,
    latest_completed: Option<String>,
    active_pins: BTreeMap<String, Vec<PinId>>,
    items: Vec<RetentionPlanItem>,
    exclusions: Vec<RetentionPlanExclusion>,
    known_run_directories: BTreeSet<String>,
}

impl<'a> RunCollector<'a> {
    fn load(
        guard: &'a NamespaceGuard,
        write: &WriteTransaction,
        before_unix_millis: u64,
    ) -> Result<Self, StoreError> {
        let runs = read_raw_records::<RunCatalogRecord>(write, RUN_CATALOG, "run-catalog")?;
        let pins = read_raw_records::<RunPinRecord>(write, RUN_PINS, "run-pins")?;
        let active_pins = active_pins_by_run(&pins);
        Ok(Self {
            guard,
            before_unix_millis,
            runs,
            pins,
            latest_attempt: raw_pointer(write, "latest-attempt")?,
            latest_completed: raw_pointer(write, "latest-completed")?,
            active_pins,
            items: Vec::new(),
            exclusions: Vec::new(),
            known_run_directories: BTreeSet::new(),
        })
    }

    fn collect(mut self) -> Result<PlanContents, StoreError> {
        self.collect_attempt_directories()?;
        collect_run_orphans(
            self.guard,
            &self.runs,
            &self.known_run_directories,
            self.before_unix_millis,
            &mut self.items,
        )?;
        self.items.sort();
        self.exclusions.sort();
        Ok(PlanContents {
            items: self.items,
            exclusions: self.exclusions,
        })
    }

    fn collect_attempt_directories(&mut self) -> Result<(), StoreError> {
        let attempts_path = self
            .guard
            .managed_parent_path(crate::namespace::records::ManagedStateParentKind::Attempts);
        for child in directory_children(&attempts_path, "attempts")? {
            self.collect_attempt_directory(&attempts_path, child)?;
        }
        Ok(())
    }

    fn collect_attempt_directory(
        &mut self,
        attempts_path: &Path,
        child: String,
    ) -> Result<(), StoreError> {
        let path = attempts_path.join(&child);
        let envelope_bytes = match fs::read(path.join("attempt.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                collect_orphan(
                    &path,
                    format!("attempts/{child}"),
                    self.before_unix_millis,
                    &mut self.items,
                )?;
                return Ok(());
            }
            Err(error) => return Err(io_error(error)),
        };
        let envelope: AttemptEnvelope =
            serde_json::from_slice(&envelope_bytes).map_err(crate::serialization_error)?;
        if envelope.attempt_id.as_str() != child {
            return Err(StoreError::Integrity(format!(
                "attempt directory {child} disagrees with its envelope"
            )));
        }
        let Some(finished) = envelope.finished_unix_millis else {
            return Ok(());
        };
        if finished >= u128::from(self.before_unix_millis) {
            return Ok(());
        }
        let is_latest_attempt = self.latest_attempt.as_deref() == Some(child.as_str());
        match envelope.state {
            AttemptState::Failed => {
                self.collect_failed_attempt(child, &envelope, &envelope_bytes, is_latest_attempt)
            }
            AttemptState::Completed => self.collect_completed_attempt(
                &path,
                child,
                &envelope,
                &envelope_bytes,
                is_latest_attempt,
            ),
            AttemptState::Running => Err(StoreError::Integrity(format!(
                "running attempt {} has a terminal timestamp",
                envelope.attempt_id.as_str()
            ))),
        }
    }

    fn collect_failed_attempt(
        &mut self,
        child: String,
        envelope: &AttemptEnvelope,
        envelope_bytes: &[u8],
        is_latest_attempt: bool,
    ) -> Result<(), StoreError> {
        if is_latest_attempt {
            self.exclusions.push(exclusion(
                RetentionItemKind::Attempt,
                child,
                RetentionExclusionReason::LatestAttempt,
            ));
        } else {
            self.items.push(retention_item_from_bytes(
                RetentionItemKind::Attempt,
                envelope.sequence,
                child,
                envelope_bytes,
            ));
        }
        Ok(())
    }

    fn collect_completed_attempt(
        &mut self,
        path: &Path,
        child: String,
        envelope: &AttemptEnvelope,
        envelope_bytes: &[u8],
        is_latest_attempt: bool,
    ) -> Result<(), StoreError> {
        let run_id = envelope.run_id.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "completed attempt {} has no run ID",
                envelope.attempt_id.as_str()
            ))
        })?;
        let Some((run, run_bytes)) = self.runs.get(run_id.as_str()) else {
            collect_orphan(
                path,
                format!("attempts/{child}"),
                self.before_unix_millis,
                &mut self.items,
            )?;
            return Ok(());
        };
        validate_run_link(envelope, run)?;
        self.known_run_directories
            .insert(run_id.as_str().to_owned());
        if collect_protection_exclusions(
            &mut self.exclusions,
            is_latest_attempt,
            self.latest_completed.as_deref() == Some(run_id.as_str()),
            self.active_pins.get(run_id.as_str()),
            &child,
            run_id.as_str(),
        ) {
            return Ok(());
        }
        self.items.push(retention_item_from_bytes(
            RetentionItemKind::Attempt,
            envelope.sequence,
            child,
            envelope_bytes,
        ));
        self.items.push(retention_item_from_bytes(
            RetentionItemKind::Run,
            run.sequence,
            run_id.as_str().to_owned(),
            run_bytes,
        ));
        self.items.push(RetentionPlanItem {
            kind: RetentionItemKind::Evidence,
            owning_sequence: run.sequence,
            record_id: format!("run:{}/evidence", run_id.as_str()),
            identity_sha256: run.evidence_store_sha256.clone(),
            byte_count: run.evidence_store_size,
        });
        collect_inactive_pins(run, &self.pins, &mut self.items)
    }
}

fn collect_protection_exclusions(
    exclusions: &mut Vec<RetentionPlanExclusion>,
    is_latest_attempt: bool,
    is_latest_completed: bool,
    active_pin_ids: Option<&Vec<PinId>>,
    attempt_id: &str,
    run_id: &str,
) -> bool {
    if is_latest_attempt {
        exclusions.push(exclusion(
            RetentionItemKind::Attempt,
            attempt_id.to_owned(),
            RetentionExclusionReason::LatestAttempt,
        ));
    }
    if is_latest_completed {
        exclusions.push(exclusion(
            RetentionItemKind::Run,
            run_id.to_owned(),
            RetentionExclusionReason::LatestCompleted,
        ));
        exclusions.push(exclusion(
            RetentionItemKind::Attempt,
            attempt_id.to_owned(),
            RetentionExclusionReason::LatestCompleted,
        ));
    }
    if let Some(pin_ids) = active_pin_ids {
        let reason = RetentionExclusionReason::ActivePin {
            pin_ids: pin_ids.clone(),
        };
        exclusions.push(exclusion(
            RetentionItemKind::Run,
            run_id.to_owned(),
            reason.clone(),
        ));
        exclusions.push(exclusion(
            RetentionItemKind::Attempt,
            attempt_id.to_owned(),
            reason,
        ));
    }
    is_latest_attempt || is_latest_completed || active_pin_ids.is_some()
}

fn active_pins_by_run(
    pins: &BTreeMap<String, (RunPinRecord, Vec<u8>)>,
) -> BTreeMap<String, Vec<PinId>> {
    let mut active = BTreeMap::<String, Vec<PinId>>::new();
    for (pin, _) in pins.values() {
        if pin.is_active() {
            active
                .entry(pin.run_id.as_str().to_owned())
                .or_default()
                .push(pin.pin_id.clone());
        }
    }
    for pin_ids in active.values_mut() {
        pin_ids.sort();
    }
    active
}

fn collect_inactive_pins(
    run: &RunCatalogRecord,
    pins: &BTreeMap<String, (RunPinRecord, Vec<u8>)>,
    items: &mut Vec<RetentionPlanItem>,
) -> Result<(), StoreError> {
    for (key, (pin, bytes)) in pins {
        if pin.run_id == run.run_id && !pin.is_active() {
            if key != pin.pin_id.as_str() {
                return Err(StoreError::Integrity(format!(
                    "run pin key {key} disagrees with its record"
                )));
            }
            items.push(retention_item_from_bytes(
                RetentionItemKind::PinOrReference,
                run.sequence,
                key.clone(),
                bytes,
            ));
        }
    }
    Ok(())
}

fn validate_run_link(envelope: &AttemptEnvelope, run: &RunCatalogRecord) -> Result<(), StoreError> {
    if run.attempt_id != envelope.attempt_id
        || run.sequence != envelope.sequence
        || envelope.run_id.as_ref() != Some(&run.run_id)
    {
        return Err(StoreError::Integrity(format!(
            "attempt {} disagrees with run {}",
            envelope.attempt_id.as_str(),
            run.run_id.as_str()
        )));
    }
    Ok(())
}

fn exclusion(
    kind: RetentionItemKind,
    record_id: String,
    reason: RetentionExclusionReason,
) -> RetentionPlanExclusion {
    RetentionPlanExclusion {
        kind,
        record_id,
        reason,
    }
}
