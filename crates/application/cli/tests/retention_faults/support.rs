use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[path = "../support/mod.rs"]
mod process;

use crate::retention_plan_support::contains_exclusion;
use crate::retention_support::{audit, json};

pub use process::{assert_status, field, run};

pub const CRASH_EXIT_CODE: i32 = 93;
pub const INVALID_SELECTOR_EXIT_CODE: i32 = 94;
const CRASH_POINT_ENV: &str = "LUMIN_TEST_RETENTION_CRASH_POINT";
const CUTOFF: &str = "9000000000000";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableState {
    Prepared,
    PruningMovingPayloads,
    PruningReadyToCommit,
    Pruned,
}

impl DurableState {
    fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::PruningMovingPayloads | Self::PruningReadyToCommit => "pruning",
            Self::Pruned => "pruned",
        }
    }

    fn recoverable_state(self) -> Option<&'static str> {
        match self {
            Self::Prepared | Self::Pruned => None,
            Self::PruningMovingPayloads => Some("moving-payloads"),
            Self::PruningReadyToCommit => Some("ready-to-commit"),
        }
    }
}

enum Target {
    Run { target: String, retained: String },
    Gate { target: String },
}

pub struct Fixture {
    root: tempfile::TempDir,
    plan_id: String,
    physical_move_count: usize,
    target: Target,
}

impl Fixture {
    pub const PLAN_OPERATION_ID: &'static str = "retention-fault-plan";
    const CONFIRM_OPERATION_ID: &'static str = "retention-fault-confirm";

    pub fn runs_without_plan() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
        let target = audit(root.path())?;
        fs::write(root.path().join("lib.ts"), "export const second = 2;\n")?;
        let retained = audit(root.path())?;
        Ok(Self {
            root,
            plan_id: String::new(),
            physical_move_count: 0,
            target: Target::Run { target, retained },
        })
    }

    pub fn runs() -> Result<Self, Box<dyn std::error::Error>> {
        let fixture = Self::runs_without_plan()?;
        let prepared = run(fixture.root(), &Self::run_plan_arguments())?;
        assert_status(&prepared, 0);
        fixture.with_plan(&prepared.stdout)
    }

    pub fn gate() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(root.path().join("src/lib.ts"), "export const value = 1;\n")?;
        let opened = run(
            root.path(),
            &[
                "pre-write",
                "--operation-id",
                "retention-fault-gate-open",
                "--path",
                "src/lib.ts",
                "--jobs",
                "1",
            ],
        )?;
        assert_status(&opened, 0);
        let gate_id = field(&opened.stdout, "gateId")?;
        let abandoned = run(
            root.path(),
            &[
                "gate",
                "abandon",
                &gate_id,
                "--operation-id",
                "retention-fault-gate-abandon",
                "--reason",
                "retention fault fixture",
            ],
        )?;
        assert_status(&abandoned, 3);
        let prepared = run(root.path(), &Self::gate_plan_arguments())?;
        assert_status(&prepared, 0);
        let mut fixture = Self {
            root,
            plan_id: String::new(),
            physical_move_count: 0,
            target: Target::Gate { target: gate_id },
        };
        fixture.bind_plan(&prepared.stdout)?;
        Ok(fixture)
    }

    pub fn with_plan(mut self, prepared: &str) -> Result<Self, Box<dyn std::error::Error>> {
        self.bind_plan(prepared)?;
        Ok(self)
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn physical_move_count(&self) -> usize {
        self.physical_move_count
    }

    pub fn run_plan_arguments() -> [&'static str; 7] {
        [
            "runs",
            "prune",
            "plan",
            "--before",
            CUTOFF,
            "--operation-id",
            Self::PLAN_OPERATION_ID,
        ]
    }

    fn gate_plan_arguments() -> [&'static str; 7] {
        [
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            CUTOFF,
            "--operation-id",
            Self::PLAN_OPERATION_ID,
        ]
    }

    pub fn confirm_arguments(&self) -> [&str; 6] {
        let domain = match &self.target {
            Target::Run { .. } => "runs",
            Target::Gate { .. } => "gate",
        };
        [
            domain,
            "prune",
            "confirm",
            &self.plan_id,
            "--operation-id",
            Self::CONFIRM_OPERATION_ID,
        ]
    }

    pub fn crash_confirm(&self, point: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = run_with_crash(self.root(), &self.confirm_arguments(), point)?;
        assert_status(&output, CRASH_EXIT_CODE);
        Ok(())
    }

    pub fn assert_state(&self, expected: DurableState) -> Result<(), Box<dyn std::error::Error>> {
        let current_pending = expected != DurableState::Prepared && self.physical_move_count > 0;
        let committed_pending =
            (expected == DurableState::Pruned).then_some(self.physical_move_count > 0);
        self.assert_public_lookup_truth(expected, current_pending, committed_pending)
    }

    fn assert_public_lookup_truth(
        &self,
        expected: DurableState,
        current_pending: bool,
        committed_pending: Option<bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let before = self.logical_snapshot()?;
        let plan = self.plan()?;
        assert_eq!(
            plan.get("planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_eq!(
            plan.get("state").and_then(Value::as_str),
            Some(expected.label())
        );
        assert_eq!(
            plan.get("physicalReclamationPending")
                .and_then(Value::as_bool),
            Some(current_pending)
        );
        match expected {
            DurableState::Prepared => {
                self.assert_live_target()?;
                self.assert_operation_absent(Self::CONFIRM_OPERATION_ID)?;
            }
            DurableState::PruningMovingPayloads | DurableState::PruningReadyToCommit => {
                let tombstone = self.assert_tombstone(expected, current_pending)?;
                self.assert_operation(expected, &tombstone, committed_pending, None)?;
            }
            DurableState::Pruned => {
                let tombstone = self.assert_tombstone(expected, current_pending)?;
                self.assert_operation(
                    expected,
                    &tombstone,
                    committed_pending,
                    Some(current_pending),
                )?;
            }
        }
        assert_eq!(self.logical_snapshot()?, before);
        Ok(())
    }

    pub fn recover_and_assert_final_truth(&self) -> Result<(), Box<dyn std::error::Error>> {
        let confirmed = run(self.root(), &self.confirm_arguments())?;
        assert_status(&confirmed, 0);
        let confirmed_json = json(&confirmed.stdout)?;
        assert_eq!(
            confirmed_json
                .pointer("/result/status")
                .and_then(Value::as_str),
            Some("pruned")
        );
        let retry = run(self.root(), &self.confirm_arguments())?;
        assert_status(&retry, 0);
        assert_eq!(retry.stdout, confirmed.stdout);

        self.assert_public_lookup_truth(
            DurableState::Pruned,
            false,
            Some(self.physical_move_count > 0),
        )?;
        match &self.target {
            Target::Run { retained, .. } => self.assert_only_run(retained),
            Target::Gate { .. } => Ok(()),
        }
    }

    pub fn assert_operation_absent(
        &self,
        operation_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = run(self.root(), &["operation", "show", operation_id])?;
        assert_status(&output, 2);
        Ok(())
    }

    pub fn assert_run_catalog_before_pruning(&self) -> Result<(), Box<dyn std::error::Error>> {
        let Target::Run { target, retained } = &self.target else {
            return Err("run catalog requested for a gate fixture".into());
        };
        let output = run(self.root(), &["runs", "list"])?;
        assert_status(&output, 0);
        let body = json(&output.stdout)?;
        let ids = run_ids(&body)?;
        assert_eq!(ids, [retained.as_str(), target.as_str()]);
        Ok(())
    }

    fn bind_plan(&mut self, prepared: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.plan_id = json(prepared)?
            .pointer("/result/planId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("prepared result omitted planId")?;
        let plan = self.plan()?;
        self.physical_move_count = plan
            .get("items")
            .and_then(Value::as_array)
            .ok_or("retention plan omitted items")?
            .iter()
            .filter(|item| {
                matches!(
                    item.get("kind").and_then(Value::as_str),
                    Some("attempt" | "run" | "orphan-payload")
                )
            })
            .count();
        Ok(())
    }

    fn plan(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let domain = match &self.target {
            Target::Run { .. } => "runs",
            Target::Gate { .. } => "gate",
        };
        let output = run(
            self.root(),
            &[domain, "prune", "plan", "show", &self.plan_id],
        )?;
        assert_status(&output, 0);
        Ok(json(&output.stdout)?)
    }

    fn assert_live_target(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (arguments, identity_pointer, payload_arguments, payload_identity_pointer, target) =
            match &self.target {
                Target::Run { target, .. } => (
                    vec!["overview", "--run", target.as_str()],
                    "/scope/id",
                    vec!["findings", "--run", target.as_str(), "--area", "dead-code"],
                    "/scope/id",
                    target,
                ),
                Target::Gate { target } => (
                    vec!["gate", "show", target.as_str()],
                    "/gateId",
                    vec!["gate", "findings", target.as_str(), "--revision", "0"],
                    "/scope/gateId",
                    target,
                ),
            };
        let output = run(self.root(), &arguments)?;
        assert_status(&output, 0);
        assert_eq!(
            json(&output.stdout)?
                .pointer(identity_pointer)
                .and_then(Value::as_str),
            Some(target.as_str())
        );

        let payload = run(self.root(), &payload_arguments)?;
        assert_status(&payload, 0);
        let body = json(&payload.stdout)?;
        assert_eq!(
            body.get("schemaVersion").and_then(Value::as_str),
            Some("lumin.collection.v1")
        );
        assert_eq!(
            body.pointer(payload_identity_pointer)
                .and_then(Value::as_str),
            Some(target.as_str())
        );
        assert!(body.get("items").and_then(Value::as_array).is_some());
        Ok(())
    }

    fn assert_tombstone(
        &self,
        expected: DurableState,
        pending: bool,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let (arguments, payload_arguments, record_kind, target) = match &self.target {
            Target::Run { target, .. } => (
                vec!["overview", "--run", target.as_str()],
                vec!["findings", "--run", target.as_str(), "--area", "dead-code"],
                "run",
                target,
            ),
            Target::Gate { target } => (
                vec!["gate", "show", target.as_str()],
                vec!["gate", "findings", target.as_str(), "--revision", "0"],
                "gate",
                target,
            ),
        };
        let output = run(self.root(), &arguments)?;
        assert_status(&output, 0);
        let payload = run(self.root(), &payload_arguments)?;
        assert_status(&payload, 0);
        assert_eq!(payload.stdout, output.stdout);
        let body = json(&output.stdout)?;
        assert_eq!(
            body.get("status").and_then(Value::as_str),
            Some(expected.label())
        );
        assert_eq!(
            body.pointer("/tombstone/recordKind")
                .and_then(Value::as_str),
            Some(record_kind)
        );
        assert_eq!(
            body.pointer("/tombstone/recordId").and_then(Value::as_str),
            Some(target.as_str())
        );
        assert_eq!(
            body.pointer("/tombstone/planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_eq!(
            body.pointer("/tombstone/physicalReclamationPending")
                .and_then(Value::as_bool),
            Some(pending)
        );
        assert_eq!(
            body.pointer("/tombstone/recoverableState")
                .and_then(Value::as_str),
            expected.recoverable_state()
        );
        if expected == DurableState::Pruned {
            assert!(
                body.pointer("/tombstone/tombstoneIdentity")
                    .and_then(Value::as_str)
                    .is_some()
            );
        } else {
            assert!(body.pointer("/tombstone/tombstoneIdentity").is_none());
        }
        Ok(body)
    }

    fn assert_operation(
        &self,
        expected: DurableState,
        tombstone: &Value,
        committed_pending: Option<bool>,
        current_pending: Option<bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = run(
            self.root(),
            &["operation", "show", Self::CONFIRM_OPERATION_ID],
        )?;
        assert_status(&output, 0);
        let body = json(&output.stdout)?;
        assert_eq!(
            body.get("schemaVersion").and_then(Value::as_str),
            Some("lumin.retention-operation.v1")
        );
        assert_eq!(
            body.get("operationId").and_then(Value::as_str),
            Some(Self::CONFIRM_OPERATION_ID)
        );
        assert_eq!(
            body.pointer("/operation/operationId")
                .and_then(Value::as_str),
            Some(Self::CONFIRM_OPERATION_ID)
        );
        assert_eq!(
            body.pointer("/operation/kind").and_then(Value::as_str),
            Some(match &self.target {
                Target::Run { .. } => "run-prune-confirm",
                Target::Gate { .. } => "gate-prune-confirm",
            })
        );
        assert_eq!(
            body.pointer("/operation/status").and_then(Value::as_str),
            Some(if expected == DurableState::Pruned {
                "committed"
            } else {
                "pruning"
            })
        );
        assert_eq!(
            body.pointer("/operation/planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_eq!(
            body.pointer("/operation/result/kind")
                .and_then(Value::as_str),
            Some("retention")
        );
        assert_eq!(
            body.pointer("/operation/result/result/status")
                .and_then(Value::as_str),
            Some(expected.label())
        );
        assert_eq!(
            body.pointer("/operation/result/result/planId")
                .and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_eq!(
            body.pointer("/operation/result/result/recoverableState"),
            tombstone.pointer("/tombstone/recoverableState")
        );
        assert_eq!(
            body.pointer("/operation/result/result/tombstoneIdentity"),
            tombstone.pointer("/tombstone/tombstoneIdentity")
        );
        if let Some(pending) = committed_pending {
            assert_eq!(
                body.pointer("/operation/result/result/physicalReclamationPending")
                    .and_then(Value::as_bool),
                Some(pending)
            );
        }
        if let Some(pending) = current_pending {
            assert_eq!(
                body.get("currentPhysicalReclamationPending")
                    .and_then(Value::as_bool),
                Some(pending)
            );
        }
        Ok(())
    }

    fn logical_snapshot(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        lumin_engine::current_logical_store_snapshot_for_test(self.root()).map_err(Into::into)
    }

    fn assert_only_run(&self, retained: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = run(self.root(), &["runs", "list"])?;
        assert_status(&output, 0);
        assert_eq!(run_ids(&json(&output.stdout)?)?, [retained]);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TreeEntryKind {
    Directory,
    File(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeEntry {
    relative_path: PathBuf,
    physical_identity: lumin_model::PhysicalFileIdentity,
    kind: TreeEntryKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LatestPhysicalSnapshot {
    latest_identity: lumin_model::PhysicalFileIdentity,
    latest: Vec<u8>,
    attempts_identity: lumin_model::PhysicalFileIdentity,
    attempts: Vec<TreeEntry>,
    runs_identity: lumin_model::PhysicalFileIdentity,
    runs: Vec<TreeEntry>,
    trash_identity: lumin_model::PhysicalFileIdentity,
    trash: Vec<TreeEntry>,
}

impl LatestPhysicalSnapshot {
    fn capture(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let state = root.join(".lumin");
        let latest = state.join("latest.json");
        let attempts = state.join("attempts");
        let runs = state.join("runs");
        let trash = state.join("trash");
        Ok(Self {
            latest_identity: lumin_engine::state_entry_physical_identity_for_test(&latest)?,
            latest: fs::read(latest)?,
            attempts_identity: lumin_engine::state_entry_physical_identity_for_test(&attempts)?,
            attempts: tree_snapshot(&attempts)?,
            runs_identity: lumin_engine::state_entry_physical_identity_for_test(&runs)?,
            runs: tree_snapshot(&runs)?,
            trash_identity: lumin_engine::state_entry_physical_identity_for_test(&trash)?,
            trash: tree_snapshot(&trash)?,
        })
    }
}

struct LatestProtectedTruth {
    plan: String,
    overview: String,
    completed_run: String,
    runs: String,
    physical: LatestPhysicalSnapshot,
}

pub struct LatestProtectionFixture {
    root: tempfile::TempDir,
    plan_id: String,
    failed_attempt: String,
    completed_attempt: String,
    completed_run: String,
    newest_attempt: String,
    newest_run: String,
    protected: LatestProtectedTruth,
}

impl LatestProtectionFixture {
    const PLAN_OPERATION_ID: &'static str = "retention-latest-crash-plan";
    const CONFIRM_OPERATION_ID: &'static str = "retention-latest-crash-confirm";

    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
        let completed_run = audit(root.path())?;
        let completed_overview = successful_json(root.path(), &["overview"])?;
        let completed_attempt = required_string(&completed_overview, "/latestAttempt/attemptId")?;

        fs::write(root.path().join("lumin.json"), b"{\n")?;
        let failed = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&failed, 1);
        let failed_overview = successful_json(root.path(), &["overview"])?;
        let failed_attempt = required_string(&failed_overview, "/latestAttempt/attemptId")?;
        assert_eq!(
            failed_overview
                .pointer("/latestAttempt/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            failed_overview.pointer("/scope/id").and_then(Value::as_str),
            Some(completed_run.as_str())
        );

        let prepared = run(root.path(), &Self::plan_arguments())?;
        assert_status(&prepared, 0);
        let plan_id = required_string(&json(&prepared.stdout)?, "/result/planId")?;
        let initial_plan =
            successful_output(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
        assert_latest_exclusions(
            &initial_plan,
            &failed_attempt,
            &completed_attempt,
            &completed_run,
        )?;

        fs::remove_file(root.path().join("lumin.json"))?;
        fs::write(root.path().join("lib.ts"), "export const newest = 2;\n")?;
        let newest_run = audit(root.path())?;
        let overview = successful_output(root.path(), &["overview"])?;
        let overview_body = json(&overview)?;
        let newest_attempt = required_string(&overview_body, "/latestAttempt/attemptId")?;
        assert_eq!(
            overview_body.pointer("/scope/id").and_then(Value::as_str),
            Some(newest_run.as_str())
        );

        let plan = successful_output(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
        assert_eq!(plan, initial_plan);
        let completed_lookup =
            successful_output(root.path(), &["overview", "--run", completed_run.as_str()])?;
        let runs = successful_output(root.path(), &["runs", "list"])?;
        let physical = LatestPhysicalSnapshot::capture(root.path())?;

        let fixture = Self {
            root,
            plan_id,
            failed_attempt,
            completed_attempt,
            completed_run,
            newest_attempt,
            newest_run,
            protected: LatestProtectedTruth {
                plan,
                overview,
                completed_run: completed_lookup,
                runs,
                physical,
            },
        };
        fixture.assert_confirmation_operation_absent()?;
        fixture.assert_protected_truth()?;
        Ok(fixture)
    }

    pub fn crash_confirm(&self, point: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = run_with_crash(self.root(), &self.confirm_arguments(), point)?;
        assert_status(&output, CRASH_EXIT_CODE);
        Ok(())
    }

    pub fn logical_snapshot(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = lumin_engine::current_logical_store_snapshot_for_test(self.root())?;
        serde_json::from_slice(&bytes).map_err(Into::into)
    }

    pub fn assert_protected_truth(&self) -> Result<(), Box<dyn std::error::Error>> {
        let plan = successful_output(
            self.root(),
            &["runs", "prune", "plan", "show", &self.plan_id],
        )?;
        assert_eq!(plan, self.protected.plan);
        assert_latest_exclusions(
            &plan,
            &self.failed_attempt,
            &self.completed_attempt,
            &self.completed_run,
        )?;

        let overview = successful_output(self.root(), &["overview"])?;
        assert_eq!(overview, self.protected.overview);
        let overview_body = json(&overview)?;
        assert_eq!(
            overview_body
                .pointer("/latestAttempt/attemptId")
                .and_then(Value::as_str),
            Some(self.newest_attempt.as_str())
        );
        assert_eq!(
            overview_body.pointer("/scope/id").and_then(Value::as_str),
            Some(self.newest_run.as_str())
        );

        let completed = successful_output(
            self.root(),
            &["overview", "--run", self.completed_run.as_str()],
        )?;
        assert_eq!(completed, self.protected.completed_run);
        let runs = successful_output(self.root(), &["runs", "list"])?;
        assert_eq!(runs, self.protected.runs);
        assert_eq!(
            LatestPhysicalSnapshot::capture(self.root())?,
            self.protected.physical
        );
        assert!(
            !self
                .root()
                .join(".lumin/trash")
                .join(&self.plan_id)
                .exists()
        );
        Ok(())
    }

    pub fn assert_confirmation_operation_absent(&self) -> Result<(), Box<dyn std::error::Error>> {
        let operation = run(
            self.root(),
            &["operation", "show", Self::CONFIRM_OPERATION_ID],
        )?;
        assert_status(&operation, 2);
        Ok(())
    }

    pub fn assert_stale_operation(&self) -> Result<String, Box<dyn std::error::Error>> {
        let operation = successful_output(
            self.root(),
            &["operation", "show", Self::CONFIRM_OPERATION_ID],
        )?;
        let body = json(&operation)?;
        assert_eq!(
            body.pointer("/operation/status").and_then(Value::as_str),
            Some("stale")
        );
        assert_eq!(
            body.pointer("/operation/result/result/planId")
                .and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_changed_inputs(&body, "/operation/result/result/changedInputs")?;
        Ok(operation)
    }

    pub fn retry_and_assert_stale(&self) -> Result<(), Box<dyn std::error::Error>> {
        let stale = run(self.root(), &self.confirm_arguments())?;
        assert_status(&stale, 5);
        let body = json(&stale.stdout)?;
        assert_eq!(
            body.pointer("/result/status").and_then(Value::as_str),
            Some("stale")
        );
        assert_eq!(
            body.pointer("/result/planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_changed_inputs(&body, "/result/changedInputs")?;

        let operation = self.assert_stale_operation()?;
        let logical = self.logical_snapshot()?;
        let retry = run(self.root(), &self.confirm_arguments())?;
        assert_status(&retry, 5);
        assert_eq!(retry.stdout, stale.stdout);
        assert_eq!(retry.stderr, stale.stderr);
        assert_eq!(self.assert_stale_operation()?, operation);
        assert_eq!(self.logical_snapshot()?, logical);
        Ok(())
    }

    pub fn assert_only_stale_operation_delta(
        &self,
        before: &Value,
        after: &Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let before_operations = before
            .get("retention_operations")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                std::io::Error::other("logical snapshot omitted retention operations")
            })?;
        assert!(!before_operations.contains_key(Self::CONFIRM_OPERATION_ID));

        let mut without_stale = after.clone();
        let after_operations = without_stale
            .get_mut("retention_operations")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                std::io::Error::other("logical snapshot omitted retention operations")
            })?;
        let encoded = after_operations
            .remove(Self::CONFIRM_OPERATION_ID)
            .ok_or_else(|| {
                std::io::Error::other("stale confirmation operation was not committed")
            })?;
        assert_eq!(&without_stale, before);

        let bytes = encoded
            .as_array()
            .ok_or_else(|| std::io::Error::other("retention operation snapshot is not bytes"))?
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .and_then(|byte| u8::try_from(byte).ok())
                    .ok_or_else(|| std::io::Error::other("retention operation byte is invalid"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let operation: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            operation.get("operationId").and_then(Value::as_str),
            Some(Self::CONFIRM_OPERATION_ID)
        );
        assert_eq!(
            operation.get("status").and_then(Value::as_str),
            Some("stale")
        );
        assert_eq!(
            operation.get("planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_changed_inputs(&operation, "/result/result/changedInputs")
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    fn plan_arguments() -> [&'static str; 7] {
        [
            "runs",
            "prune",
            "plan",
            "--before",
            CUTOFF,
            "--operation-id",
            Self::PLAN_OPERATION_ID,
        ]
    }

    fn confirm_arguments(&self) -> [&str; 6] {
        [
            "runs",
            "prune",
            "confirm",
            &self.plan_id,
            "--operation-id",
            Self::CONFIRM_OPERATION_ID,
        ]
    }
}

fn successful_output(
    root: &Path,
    arguments: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(root, arguments)?;
    assert_status(&output, 0);
    Ok(output.stdout)
}

fn successful_json(root: &Path, arguments: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(json(&successful_output(root, arguments)?)?)
}

fn required_string(value: &Value, pointer: &str) -> Result<String, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")).into())
}

fn assert_latest_exclusions(
    output: &str,
    failed_attempt: &str,
    completed_attempt: &str,
    completed_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = json(output)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("prepared"));
    assert!(
        body.get("items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert_eq!(
        body.get("exclusions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );
    assert!(contains_exclusion(
        &body,
        "attempt",
        failed_attempt,
        "latest-attempt"
    ));
    assert!(contains_exclusion(
        &body,
        "attempt",
        completed_attempt,
        "latest-completed"
    ));
    assert!(contains_exclusion(
        &body,
        "run",
        completed_run,
        "latest-completed"
    ));
    Ok(())
}

fn assert_changed_inputs(body: &Value, pointer: &str) -> Result<(), Box<dyn std::error::Error>> {
    let inputs = body
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| std::io::Error::other("changed input is not a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(inputs, ["plan-items", "plan-exclusions"]);
    Ok(())
}

fn tree_snapshot(root: &Path) -> Result<Vec<TreeEntry>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    collect_tree(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(entries)
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative_path = path.strip_prefix(root)?.to_path_buf();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other(format!(
                "state snapshot encountered a link at {}",
                path.display()
            ))
            .into());
        }
        if metadata.is_dir() {
            entries.push(TreeEntry {
                relative_path,
                physical_identity: lumin_engine::state_entry_physical_identity_for_test(&path)?,
                kind: TreeEntryKind::Directory,
            });
            collect_tree(root, &path, entries)?;
        } else if metadata.is_file() {
            entries.push(TreeEntry {
                relative_path,
                physical_identity: lumin_engine::state_entry_physical_identity_for_test(&path)?,
                kind: TreeEntryKind::File(fs::read(&path)?),
            });
        } else {
            return Err(std::io::Error::other(format!(
                "state snapshot encountered an unsupported entry at {}",
                path.display()
            ))
            .into());
        }
    }
    Ok(())
}

pub fn run_with_crash(
    root: &Path,
    arguments: &[&str],
    point: &str,
) -> Result<process::ProcessResult, Box<dyn std::error::Error>> {
    process::run_with_env(root, arguments, &[(CRASH_POINT_ENV, point)])
}

fn run_ids(body: &Value) -> Result<Vec<&str>, Box<dyn std::error::Error>> {
    let runs = body
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?;
    runs.iter()
        .map(|run| {
            run.get("runId")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("run catalog item omitted runId"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}
