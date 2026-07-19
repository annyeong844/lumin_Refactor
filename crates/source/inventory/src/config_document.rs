use std::collections::BTreeSet;
use std::fmt;

use lumin_model::{ConfigDocument, ConfigEntry, ConfigSyntax, ConfigValue, RepoPath, digest_hex};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

pub(crate) fn parse(
    path: RepoPath,
    bytes: &[u8],
    syntax: ConfigSyntax,
) -> Result<ConfigDocument, String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let normalized;
    let input = match syntax {
        ConfigSyntax::StrictJson => bytes,
        ConfigSyntax::Jsonc => {
            normalized = normalize_jsonc(bytes)?;
            normalized.as_slice()
        }
    };
    let parsed: ParsedValue = serde_json::from_slice(input).map_err(|error| error.to_string())?;
    Ok(ConfigDocument {
        path,
        payload_sha256: digest_hex(bytes),
        root: parsed.into_model(),
    })
}

#[derive(Debug)]
enum ParsedValue {
    Null,
    Boolean(bool),
    Number(String),
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

impl ParsedValue {
    fn into_model(self) -> ConfigValue {
        match self {
            Self::Null => ConfigValue::Null,
            Self::Boolean(value) => ConfigValue::Boolean(value),
            Self::Number(value) => ConfigValue::Number(value),
            Self::String(value) => ConfigValue::String(value),
            Self::Array(values) => {
                ConfigValue::Array(values.into_iter().map(ParsedValue::into_model).collect())
            }
            Self::Object(entries) => ConfigValue::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| ConfigEntry {
                        key,
                        value: value.into_model(),
                    })
                    .collect(),
            ),
        }
    }
}

impl<'de> Deserialize<'de> for ParsedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ParsedValueVisitor)
    }
}

struct ParsedValueVisitor;

impl<'de> Visitor<'de> for ParsedValueVisitor {
    type Value = ParsedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON configuration value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedValue::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedValue::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(ParsedValue::Boolean(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(ParsedValue::Number(value.to_string()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(ParsedValue::Number(value.to_string()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.is_finite() {
            Ok(ParsedValue::Number(value.to_string()))
        } else {
            Err(E::custom("configuration number must be finite"))
        }
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(ParsedValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(ParsedValue::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(ParsedValue::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate configuration key: {key}"
                )));
            }
            entries.push((key, map.next_value()?));
        }
        Ok(ParsedValue::Object(entries))
    }
}

fn normalize_jsonc(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = bytes.to_vec();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < output.len() {
        let byte = output[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        if byte == b'/' && output.get(index + 1) == Some(&b'/') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            while index < output.len() && output[index] != b'\n' && output[index] != b'\r' {
                output[index] = b' ';
                index += 1;
            }
            continue;
        }
        if byte == b'/' && output.get(index + 1) == Some(&b'*') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            let mut closed = false;
            while index < output.len() {
                if output[index] == b'*' && output.get(index + 1) == Some(&b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 2;
                    closed = true;
                    break;
                }
                if output[index] != b'\n' && output[index] != b'\r' {
                    output[index] = b' ';
                }
                index += 1;
            }
            if !closed {
                return Err("unterminated JSONC block comment".to_owned());
            }
            continue;
        }
        index += 1;
    }
    if in_string {
        return Err("unterminated JSONC string".to_owned());
    }

    index = 0;
    in_string = false;
    escaped = false;
    while index < output.len() {
        let byte = output[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b',' {
            let mut next = index + 1;
            while output.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            if matches!(output.get(next), Some(b'}' | b']')) {
                output[index] = b' ';
            }
        }
        index += 1;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_preserves_order_and_accepts_comments_and_trailing_commas()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = parse(
            RepoPath::from_portable("tsconfig.json")?,
            br#"{
                // profile owner
                "compilerOptions": { "moduleResolution": "bundler", },
                "extends": "./base",
            }"#,
            ConfigSyntax::Jsonc,
        )?;
        let entries = document
            .root
            .as_object()
            .ok_or_else(|| std::io::Error::other("object expected"))?;
        assert_eq!(entries[0].key, "compilerOptions");
        assert_eq!(entries[1].key, "extends");
        Ok(())
    }

    #[test]
    fn duplicate_keys_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let result = parse(
            RepoPath::from_portable("package.json")?,
            br#"{"name":"a","name":"b"}"#,
            ConfigSyntax::StrictJson,
        );
        assert!(result.is_err());
        Ok(())
    }
}
