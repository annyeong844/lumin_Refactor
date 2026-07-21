use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ProtocolError;

pub(super) fn encode_cursor_payload(value: &impl Serialize) -> Result<String, ProtocolError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ProtocolError::Serialization(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn decode_cursor_payload<T: DeserializeOwned>(value: &str) -> Result<T, ProtocolError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ProtocolError::CursorEncoding)?;
    serde_json::from_slice(&bytes).map_err(|error| ProtocolError::CursorPayload(error.to_string()))
}
