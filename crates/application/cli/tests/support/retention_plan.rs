use serde_json::Value;

pub fn contains_exclusion(body: &Value, kind: &str, record_id: &str, reason: &str) -> bool {
    body.get("exclusions")
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records.iter().any(|record| {
                record.get("kind").and_then(Value::as_str) == Some(kind)
                    && record.get("recordId").and_then(Value::as_str) == Some(record_id)
                    && record.pointer("/reason/reason").and_then(Value::as_str) == Some(reason)
            })
        })
}
