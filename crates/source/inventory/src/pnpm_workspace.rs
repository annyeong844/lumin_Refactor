use std::collections::BTreeSet;

use lumin_model::{ConfigDocument, ConfigEntry, ConfigValue, RepoPath, digest_hex};
use saphyr_parser::{Event, Parser, ScalarStyle, Span, SpannedEventReceiver};

pub(crate) fn parse(path: RepoPath, bytes: &[u8]) -> Result<ConfigDocument, String> {
    let input = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = std::str::from_utf8(input)
        .map_err(|error| format!("pnpm workspace YAML must be UTF-8: {error}"))?;
    let mut receiver = RestrictedYamlReceiver::default();
    Parser::new_from_str(text)
        .load(&mut receiver, true)
        .map_err(|error| error.to_string())?;
    let root = receiver.finish()?;
    if root.as_object().is_none() {
        return Err("pnpm workspace YAML root must be a mapping".to_owned());
    }
    Ok(ConfigDocument {
        path,
        payload_sha256: digest_hex(bytes),
        root,
    })
}

#[derive(Default)]
struct RestrictedYamlReceiver {
    documents: usize,
    root: Option<ConfigValue>,
    stack: Vec<Container>,
    error: Option<String>,
}

enum Container {
    Sequence(Vec<ConfigValue>),
    Mapping {
        entries: Vec<ConfigEntry>,
        keys: BTreeSet<String>,
        pending_key: Option<String>,
    },
}

impl RestrictedYamlReceiver {
    fn finish(self) -> Result<ConfigValue, String> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.documents != 1 {
            return Err(format!(
                "pnpm workspace YAML must contain exactly one document, found {}",
                self.documents
            ));
        }
        if !self.stack.is_empty() {
            return Err("pnpm workspace YAML ended inside a collection".to_owned());
        }
        self.root
            .ok_or_else(|| "pnpm workspace YAML document is empty".to_owned())
    }

    fn reject(&mut self, span: Span, detail: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(format!(
                "{} at line {}, column {}",
                detail.into(),
                span.start.line(),
                span.start.col()
            ));
        }
    }

    fn reject_decorated_node(&mut self, anchor: usize, has_tag: bool, span: Span) -> bool {
        if anchor != 0 {
            self.reject(span, "YAML anchors are outside the restricted subset");
            return true;
        }
        if has_tag {
            self.reject(span, "YAML tags are outside the restricted subset");
            return true;
        }
        false
    }

    fn attach(&mut self, value: ConfigValue, span: Span) {
        if self.error.is_some() {
            return;
        }
        let Some(container) = self.stack.last_mut() else {
            if self.root.replace(value).is_some() {
                self.reject(span, "YAML document contains more than one root value");
            }
            return;
        };
        match container {
            Container::Sequence(values) => values.push(value),
            Container::Mapping {
                entries,
                keys,
                pending_key,
            } => {
                if let Some(key) = pending_key.take() {
                    entries.push(ConfigEntry { key, value });
                    return;
                }
                let ConfigValue::String(key) = value else {
                    self.reject(span, "YAML mapping keys must be strings");
                    return;
                };
                if key == "<<" {
                    self.reject(span, "YAML merge keys are outside the restricted subset");
                    return;
                }
                if !keys.insert(key.clone()) {
                    self.reject(span, format!("duplicate configuration key: {key}"));
                    return;
                }
                *pending_key = Some(key);
            }
        }
    }

    fn close_container(&mut self, span: Span, sequence: bool) {
        if self.error.is_some() {
            return;
        }
        let Some(container) = self.stack.pop() else {
            self.reject(span, "unexpected YAML collection end");
            return;
        };
        let value = match (container, sequence) {
            (Container::Sequence(values), true) => ConfigValue::Array(values),
            (
                Container::Mapping {
                    entries,
                    pending_key: None,
                    ..
                },
                false,
            ) => ConfigValue::Object(entries),
            (Container::Mapping { .. }, false) => {
                self.reject(span, "YAML mapping key is missing a value");
                return;
            }
            _ => {
                self.reject(span, "mismatched YAML collection end");
                return;
            }
        };
        self.attach(value, span);
    }
}

impl<'input> SpannedEventReceiver<'input> for RestrictedYamlReceiver {
    fn on_event(&mut self, event: Event<'input>, span: Span) {
        if self.error.is_some() {
            return;
        }
        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd => {}
            Event::DocumentStart(_) => {
                self.documents += 1;
                if self.documents > 1 {
                    self.reject(span, "multi-document YAML is outside the restricted subset");
                }
            }
            Event::DocumentEnd => {
                if !self.stack.is_empty() {
                    self.reject(span, "YAML document ended inside a collection");
                }
            }
            Event::Alias(_) => {
                self.reject(span, "YAML aliases are outside the restricted subset");
            }
            Event::Scalar(value, style, anchor, tag) => {
                if self.reject_decorated_node(anchor, tag.is_some(), span) {
                    return;
                }
                match lower_scalar(value.as_ref(), style) {
                    Ok(value) => self.attach(value, span),
                    Err(detail) => self.reject(span, detail),
                }
            }
            Event::SequenceStart(anchor, tag) => {
                if !self.reject_decorated_node(anchor, tag.is_some(), span) {
                    self.stack.push(Container::Sequence(Vec::new()));
                }
            }
            Event::SequenceEnd => self.close_container(span, true),
            Event::MappingStart(anchor, tag) => {
                if !self.reject_decorated_node(anchor, tag.is_some(), span) {
                    self.stack.push(Container::Mapping {
                        entries: Vec::new(),
                        keys: BTreeSet::new(),
                        pending_key: None,
                    });
                }
            }
            Event::MappingEnd => self.close_container(span, false),
        }
    }
}

fn lower_scalar(value: &str, style: ScalarStyle) -> Result<ConfigValue, String> {
    match style {
        ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted => {
            Ok(ConfigValue::String(value.to_owned()))
        }
        ScalarStyle::Literal | ScalarStyle::Folded => {
            Err("block scalar styles are outside the restricted subset".to_owned())
        }
        ScalarStyle::Plain => lower_plain_scalar(value),
    }
}

fn lower_plain_scalar(value: &str) -> Result<ConfigValue, String> {
    match value {
        "null" => return Ok(ConfigValue::Null),
        "true" => return Ok(ConfigValue::Boolean(true)),
        "false" => return Ok(ConfigValue::Boolean(false)),
        _ => {}
    }
    let lowercase = value.to_ascii_lowercase();
    if matches!(lowercase.as_str(), ".nan" | ".inf" | "+.inf" | "-.inf") {
        return Err("non-finite YAML numbers are outside the restricted subset".to_owned());
    }
    let unsigned = value
        .strip_prefix(['+', '-'])
        .unwrap_or(value)
        .to_ascii_lowercase();
    if unsigned.starts_with("0b") {
        return Err("binary YAML scalars are outside the restricted subset".to_owned());
    }
    if looks_like_timestamp(value) {
        return Err("YAML timestamps are outside the restricted subset".to_owned());
    }
    if value.parse::<serde_json::Number>().is_ok() {
        return Ok(ConfigValue::Number(value.to_owned()));
    }
    Ok(ConfigValue::String(value.to_owned()))
}

fn looks_like_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && (bytes.len() == 10 || matches!(bytes[10], b'T' | b't' | b' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_supported_block_and_flow_values_in_source_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = parse(
            RepoPath::from_portable("pnpm-workspace.yaml")?,
            br#"packages: [packages/*, tools/**]
packageConfigs:
  - match: [project-1, project-2]
    saveExact: true
catalog: {react: "19", retries: 3, absent: null}
"#,
        )?;
        let entries = document
            .root
            .as_object()
            .ok_or_else(|| std::io::Error::other("mapping expected"))?;
        assert_eq!(entries[0].key, "packages");
        assert_eq!(entries[1].key, "packageConfigs");
        assert_eq!(entries[2].key, "catalog");
        assert!(entries[1].value.get("missing").is_none());
        assert!(matches!(
            entries[2].value.get("retries"),
            Some(ConfigValue::Number(value)) if value == "3"
        ));
        assert!(matches!(
            entries[2].value.get("absent"),
            Some(ConfigValue::Null)
        ));
        Ok(())
    }

    #[test]
    fn rejects_yaml_features_outside_the_restricted_subset()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("pnpm-workspace.yaml")?;
        for input in [
            "packages: []\npackages: []\n",
            "packages: &members []\n",
            "packages: *members\n",
            "packages: !!seq []\n",
            "base: &base {packages: []}\n<<: *base\n",
            "packages: []\n---\npackages: []\n",
            "created: 2026-07-19\n",
            "value: .inf\n",
            "value: 0b1010\n",
            "value: |\n  text\n",
        ] {
            assert!(parse(path.clone(), input.as_bytes()).is_err(), "{input}");
        }
        Ok(())
    }
}
