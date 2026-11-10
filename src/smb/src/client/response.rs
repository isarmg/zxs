use crate::protocol::Response;
use data_serialization::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use json_codec::{Map, Value, value::RawValue};
use std::fmt;

struct Envelope<'a> {
    fields: Map<String, Value>,
    result: Option<&'a RawValue>,
}

impl<'de> Deserialize<'de> for Envelope<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = Envelope<'de>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a response object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Map::new();
                let mut result = None;
                while let Some(key) = map.next_key::<String>()? {
                    let raw = map.next_value::<&RawValue>()?;
                    let value = if key == "result" {
                        result = Some(raw);
                        Value::Null
                    } else {
                        json_codec::from_str(raw.get())
                            .expect("invariant: strict JSON field remains valid")
                    };
                    // Match strict_json's Map order even when preserve_order is
                    // enabled by another consumer's dependency features.
                    fields.insert(key, value);
                }
                Ok(Envelope { fields, result })
            }
        }
        deserializer.deserialize_map(EnvelopeVisitor)
    }
}

/// Retain strict envelope admission while borrowing the result for typed decoding.
pub(super) fn decode(payload: &[u8]) -> storage_contract::Result<(Response, Option<&RawValue>)> {
    let validated = storage_contract::StrictJson::parse(payload)?;
    if payload
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return storage_contract::decode_strict(payload).map(|response| (response, None));
    }
    let Envelope { fields, result } = validated.decode()?;
    Ok((json_codec::from_value(Value::Object(fields))?, result))
}

pub(super) fn decode_result<R: data_serialization::de::DeserializeOwned>(
    value: Value,
    raw: Option<&RawValue>,
) -> json_codec::Result<R> {
    let Some(raw) = raw else {
        return json_codec::from_value(value);
    };
    match json_codec::from_str(raw.get()) {
        Ok(result) => Ok(result),
        Err(_) => {
            // Keep the original Value diagnostic and source for rejected DTOs,
            // including the active Map order. Successful typed payloads avoid
            // the per-element Value tree.
            let value = json_codec::from_str(raw.get())
                .expect("invariant: strict JSON result remains valid");
            json_codec::from_value(value)
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/smb/unit/client/response.rs"]
pub(crate) mod tests;
