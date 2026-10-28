use crate::{Error, Result};
use data_serialization::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use json_codec::{Map, Number, Value};
use std::fmt;

/// Reject duplicate keys before a Value or typed DTO can erase the evidence.
pub fn strict_json(bytes: &[u8]) -> Result<Value> {
    struct Strict(Value);
    impl<'de> Deserialize<'de> for Strict {
        fn deserialize<D: data_serialization::Deserializer<'de>>(
            d: D,
        ) -> std::result::Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Strict;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("integer-only JSON without duplicate fields")
                }
                fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::Bool(v)))
                }
                fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::Null))
                }
                fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::Number(v.into())))
                }
                fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::Number(v.into())))
                }
                fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Strict, E> {
                    Err(E::custom("floating point is not part of the ZXS contract"))
                }
                fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::String(v.into())))
                }
                fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Strict, E> {
                    Ok(Strict(Value::String(v)))
                }
                fn visit_seq<A: SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut v = Vec::new();
                    while let Some(e) = a.next_element::<Strict>()? {
                        v.push(e.0);
                    }
                    Ok(Strict(Value::Array(v)))
                }
                fn visit_map<A: MapAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<Strict, A::Error> {
                    let mut v = Map::new();
                    while let Some(k) = a.next_key::<String>()? {
                        if v.contains_key(&k) {
                            return Err(de::Error::custom(format!("duplicate field: {k}")));
                        }
                        v.insert(k, a.next_value::<Strict>()?.0);
                    }
                    Ok(Strict(Value::Object(v)))
                }
            }
            d.deserialize_any(V)
        }
    }
    let mut d = json_codec::Deserializer::from_slice(bytes);
    let v = Strict::deserialize(&mut d)?;
    d.end()?;
    Ok(v.0)
}

pub fn decode_strict<T: data_serialization::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(json_codec::from_value(strict_json(bytes)?)?)
}

/// JCS for the integer-only ZXS schema. UTF-16 ordering is significant for keys.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    crate::integer_json::validate(value)?;
    emit_canonical_json(&json_codec::to_value(value)?)
}

/// Canonical encoding of an existing JSON value without rebuilding its tree.
pub fn canonical_json_value(value: &Value) -> Result<Vec<u8>> {
    crate::integer_json::validate(value)?;
    emit_canonical_json(value)
}

fn ascii_keys_are_ordered(object: &Map<String, Value>) -> bool {
    // The default Map is a BTreeMap; preserve_order instead uses insertion
    // order. Check the actual iteration so feature unification cannot change
    // canonical bytes. ASCII byte order and UTF-16 order are identical.
    let mut previous: Option<&str> = None;
    object.keys().all(|key| {
        let ordered = key.is_ascii() && previous.is_none_or(|previous| previous < key.as_str());
        previous = Some(key);
        ordered
    })
}

fn emit_canonical_json(value: &Value) -> Result<Vec<u8>> {
    fn number(n: &Number, out: &mut Vec<u8>) -> Result<()> {
        const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
        let safe = n
            .as_i64()
            .is_some_and(|v| v.unsigned_abs() <= MAX_SAFE_INTEGER)
            || n.as_u64().is_some_and(|v| v <= MAX_SAFE_INTEGER);
        if !safe {
            return Err(Error::new(
                crate::ErrorCode::InvalidJson,
                "JSON numbers must be safe integers; encode u64 fields as decimal strings",
            ));
        }
        json_codec::to_writer(out, n)?;
        Ok(())
    }
    fn member(index: usize, key: &str, value: &Value, out: &mut Vec<u8>) -> Result<()> {
        if index > 0 {
            out.push(b',');
        }
        json_codec::to_writer(&mut *out, key)?;
        out.push(b':');
        emit(value, out)
    }
    fn emit(v: &Value, out: &mut Vec<u8>) -> Result<()> {
        match v {
            Value::Null => out.extend_from_slice(b"null"),
            Value::Bool(true) => out.extend_from_slice(b"true"),
            Value::Bool(false) => out.extend_from_slice(b"false"),
            Value::String(s) => json_codec::to_writer(&mut *out, s)?,
            Value::Number(n) => number(n, out)?,
            Value::Array(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    emit(v, out)?;
                }
                out.push(b']');
            }
            Value::Object(o) => {
                out.push(b'{');
                if ascii_keys_are_ordered(o) {
                    for (i, (key, value)) in o.iter().enumerate() {
                        member(i, key, value, out)?;
                    }
                } else {
                    let mut keys = o.keys().collect::<Vec<_>>();
                    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                    for (i, key) in keys.into_iter().enumerate() {
                        member(i, key, &o[key], out)?;
                    }
                }
                out.push(b'}');
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    emit(value, &mut out)?;
    Ok(out)
}
pub fn object_hash<T: Serialize + ?Sized>(kind: &str, value: &T) -> Result<String> {
    encode_hashed_object(kind, value).map(|(_, hash)| hash)
}

/// Canonical bytes and their domain-separated digest from one encoding.
/// Returning the bytes with their hash avoids serializing immutable replicas twice.
pub fn encode_hashed_object<T: Serialize + ?Sized>(
    kind: &str,
    value: &T,
) -> Result<(Vec<u8>, String)> {
    validate_object_domain(kind)?;
    Ok(hash_canonical_bytes(kind, canonical_json(value)?))
}

/// Domain-separated digest of an existing JSON value without rebuilding it.
pub fn object_hash_value(kind: &str, value: &Value) -> Result<String> {
    encode_hashed_value(kind, value).map(|(_, hash)| hash)
}

/// Canonical bytes and digest of an existing JSON value without rebuilding it.
pub fn encode_hashed_value(kind: &str, value: &Value) -> Result<(Vec<u8>, String)> {
    validate_object_domain(kind)?;
    Ok(hash_canonical_bytes(kind, canonical_json_value(value)?))
}

fn validate_object_domain(kind: &str) -> Result<()> {
    if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        return Err(Error::new(
            crate::ErrorCode::InvalidObjectType,
            "invalid metadata domain",
        ));
    }
    Ok(())
}

fn hash_canonical_bytes(kind: &str, bytes: Vec<u8>) -> (Vec<u8>, String) {
    let mut h = blake3::Hasher::new();
    h.update(format!("zxs:{kind}\0").as_bytes());
    h.update(&bytes);
    (bytes, h.finalize().to_hex().to_string())
}

/// Digest for a JSON mutation request; transport credentials never enter it.
pub fn request_digest(
    method: &str,
    route: &str,
    path_parameters: Value,
    query: Value,
    body: &Value,
) -> Result<String> {
    let mut body = body.clone();
    body.as_object_mut()
        .ok_or_else(|| {
            Error::new(
                crate::ErrorCode::InvalidRequest,
                "request body must be an object",
            )
        })?
        .remove("request_digest");
    object_hash_value(
        "request",
        &json_codec::json!({"method":method.to_ascii_uppercase(),"route":route,"path_parameters":path_parameters,"query":query,"body":body}),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ChecksumManifest {
    pub chunk_bytes: u32,
    #[serialization(with = "crate::u64_string")]
    pub logical_size: u64,
    pub chunks: Vec<[u8; 32]>,
}
/// Supported by checksum encoding, configuration admission and package readers.
pub const SUPPORTED_CHECKSUM_CHUNK_BYTES: [u32; 4] = [65_536, 262_144, 1_048_576, 4_194_304];

/// Fixed semantic header: magic, algorithm, chunk size, logical size and count.
pub const CHECKSUM_HEADER_BYTES: usize = 30;
const CHECKSUM_MAGIC: &[u8; 8] = b"ZXSCHKS\0";

impl ChecksumManifest {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let capacity = self
            .chunks
            .len()
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(CHECKSUM_HEADER_BYTES))
            .ok_or_else(|| {
                Error::new(
                    crate::ErrorCode::ChecksumFormatInvalid,
                    "checksum manifest length overflow",
                )
            })?;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(CHECKSUM_MAGIC);
        out.extend(1u16.to_le_bytes());
        out.extend(self.chunk_bytes.to_le_bytes());
        out.extend(self.logical_size.to_le_bytes());
        out.extend((self.chunks.len() as u64).to_le_bytes());
        for h in &self.chunks {
            out.extend(h);
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let bad = || {
            Error::new(
                crate::ErrorCode::ChecksumFormatInvalid,
                "invalid checksum manifest",
            )
        };
        if bytes.len() < CHECKSUM_HEADER_BYTES
            || &bytes[..8] != CHECKSUM_MAGIC
            || bytes[8..10] != 1u16.to_le_bytes()
        {
            return Err(bad());
        }
        let chunk_bytes = u32::from_le_bytes(
            bytes[10..14]
                .try_into()
                .expect("invariant: validated checksum header contains chunk size"),
        );
        let logical_size = u64::from_le_bytes(
            bytes[14..22]
                .try_into()
                .expect("invariant: validated checksum header contains logical size"),
        );
        let count = u64::from_le_bytes(
            bytes[22..30]
                .try_into()
                .expect("invariant: validated checksum header contains chunk count"),
        );
        let payload_len = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(32))
            .and_then(|bytes| bytes.checked_add(CHECKSUM_HEADER_BYTES));
        if payload_len != Some(bytes.len()) {
            return Err(bad());
        }
        let chunks = bytes[CHECKSUM_HEADER_BYTES..].as_chunks::<32>().0.to_vec();
        let m = Self {
            chunk_bytes,
            logical_size,
            chunks,
        };
        m.validate()?;
        Ok(m)
    }
    fn validate(&self) -> Result<()> {
        if !SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&self.chunk_bytes)
            || self.chunks.len() as u64 != self.logical_size.div_ceil(u64::from(self.chunk_bytes))
        {
            return Err(Error::new(
                crate::ErrorCode::ChecksumFormatInvalid,
                "checksum chunk count or size mismatch",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/encoding/tests.rs"]
mod tests;
