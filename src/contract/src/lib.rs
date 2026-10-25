//! Portable ZXS wire and disk contracts. No server or platform dependencies.
#![forbid(unsafe_code)]
pub mod api_path;
pub mod client_protocol;
pub mod encoding;
mod integer_json;
pub mod model;
mod shared_records;
pub mod text;
mod workspace_capacity;
pub use api_path::validate_api_path;
pub use encoding::*;
pub use model::*;
pub use shared_records::SharedRecords;
pub use text::{MAX_FILE_TAGS, MAX_NOTE_BYTES, MAX_TAG_BYTES, validate_note, validate_tag};
pub use workspace_capacity::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, error_definition::Error)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
    pub fn code(&self) -> &str {
        &self.code
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        let code = match error.raw_os_error() {
            Some(28) => "NO_SPACE",
            _ => "IO_ERROR",
        };
        Self::new(code, error.to_string())
    }
}
impl From<json_codec::Error> for Error {
    fn from(error: json_codec::Error) -> Self {
        Self::new("INVALID_JSON", error.to_string())
    }
}
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
pub fn validate_id(value: &str, prefix: &str) -> Result<()> {
    let valid = value.strip_prefix(&format!("{prefix}_")).is_some_and(|v| {
        v.len() == 32
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            "INVALID_ID",
            format!("invalid {prefix} identifier"),
        ))
    }
}

/// Parse the one wire representation of a full-width unsigned integer.
/// JSON numbers, signs, whitespace, leading zeroes, and overflow are rejected.
pub fn parse_u64_decimal(value: &str) -> Result<u64> {
    if value.is_empty()
        || value.len() > 20
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::new(
            "INVALID_INTEGER",
            "expected a canonical unsigned decimal string",
        ));
    }
    value
        .parse()
        .map_err(|_| Error::new("INVALID_INTEGER", "unsigned decimal exceeds u64"))
}

pub mod u64_string {
    use data_serialization::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        crate::parse_u64_decimal(&value).map_err(data_serialization::de::Error::custom)
    }
}

pub mod optional_u64_string {
    use data_serialization::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                crate::parse_u64_decimal(&value).map_err(data_serialization::de::Error::custom)
            })
            .transpose()
    }
}

pub mod u64_string_map {
    use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    pub fn serialize<S: Serializer>(
        value: &BTreeMap<String, u64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, u64>, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, value)| {
                crate::parse_u64_decimal(&value)
                    .map(|value| (key, value))
                    .map_err(data_serialization::de::Error::custom)
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/lib/tests.rs"]
mod tests;
