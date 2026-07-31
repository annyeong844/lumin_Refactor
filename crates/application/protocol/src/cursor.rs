use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::ProtocolError;

const CURSOR_BINDING_DOMAIN: &[u8] = b"lumin-cursor-binding.v1\0";
const CURSOR_BINDING_VERSION: u8 = 1;
const CURSOR_BINDING_LEN: usize = 32;
const CURSOR_HEADER_LEN: usize = CURSOR_BINDING_DOMAIN.len() + 1 + CURSOR_BINDING_LEN;

pub(super) fn encode_cursor_payload(value: &impl Serialize) -> Result<String, ProtocolError> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| ProtocolError::Serialization(error.to_string()))?;
    let binding = cursor_binding(&payload);
    let mut envelope = Vec::with_capacity(CURSOR_HEADER_LEN + payload.len());
    envelope.extend_from_slice(CURSOR_BINDING_DOMAIN);
    envelope.push(CURSOR_BINDING_VERSION);
    envelope.extend_from_slice(&binding);
    envelope.extend_from_slice(&payload);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

pub(super) fn decode_cursor_payload<T: DeserializeOwned>(value: &str) -> Result<T, ProtocolError> {
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ProtocolError::CursorEncoding)?;
    if envelope.len() < CURSOR_HEADER_LEN {
        return Err(ProtocolError::CursorPayload(
            "cursor content-binding envelope is truncated".to_owned(),
        ));
    }
    let version_offset = CURSOR_BINDING_DOMAIN.len();
    if !envelope.starts_with(CURSOR_BINDING_DOMAIN)
        || envelope[version_offset] != CURSOR_BINDING_VERSION
    {
        return Err(ProtocolError::CursorPayload(
            "cursor content-binding envelope is unsupported".to_owned(),
        ));
    }
    let binding_offset = version_offset + 1;
    let payload = &envelope[CURSOR_HEADER_LEN..];
    let expected = cursor_binding(payload);
    if envelope[binding_offset..CURSOR_HEADER_LEN] != expected {
        return Err(ProtocolError::CursorPayload(
            "cursor content binding does not match its payload".to_owned(),
        ));
    }
    serde_json::from_slice(payload).map_err(|error| ProtocolError::CursorPayload(error.to_string()))
}

fn cursor_binding(payload: &[u8]) -> [u8; CURSOR_BINDING_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(CURSOR_BINDING_DOMAIN);
    hasher.update([CURSOR_BINDING_VERSION]);
    hasher.update(payload);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_payload_change_rejects_the_issued_binding()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = encode_cursor_payload(&serde_json::json!({
            "schemaVersion": "cursor.v1",
            "lastId": "finding-a"
        }))?;
        let mut envelope = URL_SAFE_NO_PAD.decode(cursor)?;
        let mut payload: serde_json::Value =
            serde_json::from_slice(&envelope[CURSOR_HEADER_LEN..])?;
        payload["lastId"] = serde_json::Value::String("finding-b".to_owned());
        envelope.truncate(CURSOR_HEADER_LEN);
        envelope.extend_from_slice(&serde_json::to_vec(&payload)?);
        let tampered = URL_SAFE_NO_PAD.encode(envelope);

        assert!(matches!(
            decode_cursor_payload::<serde_json::Value>(&tampered),
            Err(ProtocolError::CursorPayload(message))
                if message.contains("content binding")
        ));
        Ok(())
    }
}
