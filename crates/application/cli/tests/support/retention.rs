use std::path::Path;

use serde_json::Value;

use crate::support::{assert_status, field, run};

pub fn audit(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&output, 0);
    field(&output.stdout, "runId")
}

pub fn json(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}
