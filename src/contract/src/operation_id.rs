use crate::{Error, ErrorCode};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// Operation identity with canonical `op_` plus 32 lowercase hexadecimal wire encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct OperationId(uuid::Uuid);

impl OperationId {
    /// Generate a UUID v4 identity directly in its domain representation.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl FromStr for OperationId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hex) = value.strip_prefix("op_") else {
            return Err(Error::new(ErrorCode::InvalidId, "invalid op identifier"));
        };
        if hex.len() != 32
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::new(ErrorCode::InvalidId, "invalid op identifier"));
        }
        let mut bytes = [0; 16];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let digit = |value: u8| {
                if value <= b'9' {
                    value - b'0'
                } else {
                    value - b'a' + 10
                }
            };
            *byte = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(uuid::Uuid::from_bytes(bytes)))
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "op_{}", self.0.simple())
    }
}

impl Serialize for OperationId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(data_serialization::de::Error::custom)
    }
}
