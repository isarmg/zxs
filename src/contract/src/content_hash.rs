use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// BLAKE3 of the complete file body, without an object-domain prefix.
/// This does not represent a checksum-manifest or canonical-object hash.
/// Parsing requires exactly 64 lowercase hexadecimal characters; malformed
/// external text returns `ParseContentHashError`. Serialization preserves that text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ContentHash([u8; 32]);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error("content hash must be exactly 64 lowercase hexadecimal characters")]
pub struct ParseContentHashError;

impl ContentHash {
    /// Retain an internally computed body digest without a text round trip.
    pub fn from_digest(digest: blake3::Hash) -> Self {
        Self(*digest.as_bytes())
    }

    /// The body commitment used by full-byte integrity verification.
    pub fn digest(self) -> blake3::Hash {
        blake3::Hash::from_bytes(self.0)
    }
}

impl FromStr for ContentHash {
    type Err = ParseContentHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ParseContentHashError);
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            let digit = |value: u8| {
                if value <= b'9' {
                    value - b'0'
                } else {
                    value - b'a' + 10
                }
            };
            *byte = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.digest().to_hex().as_str())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(data_serialization::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/content_hash.rs"]
mod tests;
