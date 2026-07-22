use std::fs;
use std::path::Path;

use serde_json::Value;

#[path = "../support/mod.rs"]
mod process;

use crate::retention_support::{audit, json};

pub use process::{assert_status, field, run};

pub const CRASH_EXIT_CODE: i32 = 93;
pub const INVALID_SELECTOR_EXIT_CODE: i32 = 94;
const CRASH_POINT_ENV: &str = "LUMIN_TEST_RETENTION_CRASH_POINT";
const CUTOFF: &str = "9000000000000";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableState {
    Prepared,
    Pruning,
    Pruned,
}

impl DurableState {
    fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Pruning => "pruning",
            Self::Pruned => "pruned",
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
        assert_status(&abandoned, 0);
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
        let plan = self.plan()?;
        assert_eq!(
            plan.get("state").and_then(Value::as_str),
            Some(expected.label())
        );
        match expected {
            DurableState::Prepared => {
                self.assert_live_target()?;
                self.assert_operation_absent(Self::CONFIRM_OPERATION_ID)
            }
            DurableState::Pruning => {
                self.assert_tombstone("pruning", self.physical_move_count > 0)?;
                self.assert_operation("pruning", "pruning", None)
            }
            DurableState::Pruned => {
                let pending = self.physical_move_count > 0;
                self.assert_tombstone("pruned", pending)?;
                self.assert_operation("committed", "pruned", Some(pending))
            }
        }
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

        let plan = self.plan()?;
        assert_eq!(plan.get("state").and_then(Value::as_str), Some("pruned"));
        assert_eq!(
            plan.get("physicalReclamationPending")
                .and_then(Value::as_bool),
            Some(false)
        );
        self.assert_tombstone("pruned", false)?;
        let pending = matches!(self.target, Target::Run { .. });
        self.assert_operation("committed", "pruned", Some(pending))?;
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
        let (arguments, identity_pointer, target) = match &self.target {
            Target::Run { target, .. } => (
                vec!["overview", "--run", target.as_str()],
                "/scope/id",
                target,
            ),
            Target::Gate { target } => (vec!["gate", "show", target.as_str()], "/gateId", target),
        };
        let output = run(self.root(), &arguments)?;
        assert_status(&output, 0);
        assert_eq!(
            json(&output.stdout)?
                .pointer(identity_pointer)
                .and_then(Value::as_str),
            Some(target.as_str())
        );
        Ok(())
    }

    fn assert_tombstone(
        &self,
        status: &str,
        pending: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let arguments = match &self.target {
            Target::Run { target, .. } => vec!["overview", "--run", target],
            Target::Gate { target } => vec!["gate", "show", target],
        };
        let output = run(self.root(), &arguments)?;
        assert_status(&output, 0);
        let body = json(&output.stdout)?;
        assert_eq!(body.get("status").and_then(Value::as_str), Some(status));
        assert_eq!(
            body.pointer("/tombstone/planId").and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        assert_eq!(
            body.pointer("/tombstone/physicalReclamationPending")
                .and_then(Value::as_bool),
            Some(pending)
        );
        Ok(())
    }

    fn assert_operation(
        &self,
        operation_status: &str,
        result_status: &str,
        pending: Option<bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = run(
            self.root(),
            &["operation", "show", Self::CONFIRM_OPERATION_ID],
        )?;
        assert_status(&output, 0);
        let body = json(&output.stdout)?;
        assert_eq!(
            body.pointer("/operation/status").and_then(Value::as_str),
            Some(operation_status)
        );
        assert_eq!(
            body.pointer("/operation/result/result/status")
                .and_then(Value::as_str),
            Some(result_status)
        );
        assert_eq!(
            body.pointer("/operation/result/result/planId")
                .and_then(Value::as_str),
            Some(self.plan_id.as_str())
        );
        if let Some(pending) = pending {
            assert_eq!(
                body.pointer("/operation/result/result/physicalReclamationPending")
                    .and_then(Value::as_bool),
                Some(pending)
            );
        }
        Ok(())
    }

    fn assert_only_run(&self, retained: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = run(self.root(), &["runs", "list"])?;
        assert_status(&output, 0);
        assert_eq!(run_ids(&json(&output.stdout)?)?, [retained]);
        Ok(())
    }
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
