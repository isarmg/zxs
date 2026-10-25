//! Transport-independent client protocol. HTTP, credentials and I/O stay in adapters.
use json_codec::json;

mod operation;
pub use operation::{OperationStatus, operation_status};

/// Common JSON response cap, enforced while streaming and again when decoding.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, error_definition::Error)]
#[error("{code}: {message}")]
pub struct ResponseError {
    pub status: u16,
    pub code: crate::ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    #[source]
    pub source: Option<ResponseDecodeError>,
}
#[derive(Debug, error_definition::Error)]
pub enum ResponseDecodeError {
    #[error("invalid response JSON: {0}")]
    Json(#[source] json_codec::Error),
    #[error("invalid response error code: {0}")]
    Code(#[source] json_codec::Error),
}
impl ResponseError {
    pub fn new(status: u16, code: crate::ErrorCode, message: &str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retryable: status >= 500 || status == 429,
            request_id: None,
            source: None,
        }
    }
}

/// Only an explicit client-error response proves that a submitted mutation was rejected.
pub fn mutation_outcome_unknown(status: u16) -> bool {
    !(400..500).contains(&status)
}

/// Only the exact current logout response confirms server-side revocation.
pub fn logout_confirmed(value: &Value) -> bool {
    value == &json!({"logged_out":true})
}

/// Missing or extra fields cannot confirm a non-idempotent password change.
pub fn password_change_confirmed(value: &Value) -> bool {
    value == &json!({"changed":true,"reauthentication_required":true})
}

/// Missing IPC stage evidence must be treated conservatively.
pub fn publication_attempted(evidence: Option<bool>) -> bool {
    evidence != Some(false)
}

pub fn response_too_large(status: u16) -> ResponseError {
    ResponseError::new(
        status,
        crate::ErrorCode::ResponseTooLarge,
        "JSON response exceeds client limit",
    )
}

use data_serialization::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use json_codec::{Map, Number, Value};
use std::fmt;

struct ResponseJson(Value);

impl<'de> Deserialize<'de> for ResponseJson {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = ResponseJson;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("JSON without duplicate fields or non-finite numbers")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::Bool(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::Null))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::Number(value.into())))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::Number(value.into())))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<ResponseJson, E> {
                Number::from_f64(value)
                    .map(|number| ResponseJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("JSON numbers must be finite"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::String(value.into())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<ResponseJson, E> {
                Ok(ResponseJson(Value::String(value)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<ResponseJson, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = access.next_element::<ResponseJson>()? {
                    values.push(value.0);
                }
                Ok(ResponseJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<ResponseJson, A::Error> {
                let mut values = Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON field"));
                    }
                    values.insert(key, access.next_value::<ResponseJson>()?.0);
                }
                Ok(ResponseJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

fn strict_response_json(bytes: &[u8]) -> Result<Value, json_codec::Error> {
    let mut deserializer = json_codec::Deserializer::from_slice(bytes);
    let value = ResponseJson::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value.0)
}

/// Decode response-only JSON (telemetry may contain finite decimals). This does
/// not relax the integer-only parser used for persisted state and write bodies.
pub fn decode_response(status: u16, bytes: &[u8]) -> Result<Value, ResponseError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(response_too_large(status));
    }
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        strict_response_json(bytes).map_err(|source| {
            let mut error = ResponseError::new(
                status,
                crate::ErrorCode::InvalidResponse,
                "The response is not valid JSON",
            );
            error.source = Some(ResponseDecodeError::Json(source));
            error
        })?
    };
    if !(200..300).contains(&status) {
        let item = value.get("error").unwrap_or(&value);
        return Err(ResponseError {
            status,
            code: match item.get("code") {
                Some(code) => json_codec::from_value(code.clone()).map_err(|error| {
                    let mut failure = ResponseError::new(
                        status,
                        crate::ErrorCode::InvalidResponse,
                        "The response error code is invalid or unknown",
                    );
                    failure.source = Some(ResponseDecodeError::Code(error));
                    failure
                })?,
                None => crate::ErrorCode::HttpError,
            },
            message: item["message"]
                .as_str()
                .unwrap_or("The request failed")
                .into(),
            retryable: item["retryable"]
                .as_bool()
                .unwrap_or(status >= 500 || status == 429),
            request_id: item["request_id"].as_str().map(str::to_owned),
            source: None,
        });
    }
    Ok(value)
}

/// Bind a seal response to the requested workspace before adding caller-owned
/// fields and computing the canonical public commit envelope digest.
pub fn prepare_commit(
    mut seal: Value,
    workspace: &str,
    operation_id: &str,
    message: &str,
    expected_head: Option<&str>,
) -> crate::Result<Value> {
    if !seal.is_object() || seal["workspace_id"].as_str() != Some(workspace) {
        return Err(crate::Error::new(
            crate::ErrorCode::InvalidResponse,
            "seal response has an invalid workspace identity",
        ));
    }
    let object = seal.as_object_mut().expect("validated object");
    // Replace the digest and caller metadata; retain the frozen workspace head
    // when the caller has no explicit precondition override.
    object.remove("request_digest");
    object.insert("operation_id".into(), operation_id.into());
    object.insert("message".into(), message.into());
    if let Some(head) = expected_head.filter(|head| !head.is_empty()) {
        object.insert("expected_head".into(), head.into());
    }
    let digest = crate::request_digest(
        "POST",
        "/api/workspaces/{workspace}/commit",
        json!({"workspace":workspace}),
        json!({}),
        &seal,
    )?;
    seal["request_digest"] = digest.into();
    Ok(seal)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/client_protocol/tests.rs"]
mod tests;
