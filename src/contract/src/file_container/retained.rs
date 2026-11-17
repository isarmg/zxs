//! Retained-container naming evidence. A decoded name is neither an identity
//! nor authority to create/open an entry; the owner still admits its held root.
use crate::{DiskId, OperationId, PoolId, Timestamp};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use name_policy::Name;
use std::{fmt, str::FromStr};

pub const RETAINED_PREFIX_BYTES: usize = 16;
pub const RETAINED_DISK_COUNT: u16 = 36 * 36;
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

#[derive(Debug, error_definition::Error)]
pub enum RetainedNameError {
    #[error("retained disk number must be exactly two lowercase base-36 digits")]
    DiskNumber,
    #[error("retained container prefix is not the fixed sixteen-byte representation")]
    Prefix,
    #[error("retained original name is invalid: {source}")]
    OriginalName {
        #[source]
        source: name_policy::Error,
    },
    #[error("sampling a retained container name: {source}")]
    Entropy {
        #[source]
        source: system_entropy::Error,
    },
    #[error("retained random suffix sampling exhausted its bounded entropy draws")]
    RandomDraws,
}

/// A display number assigned once by the pool's permanent original operations.
/// It is not derived from a slot, an enumeration index or truncated DiskId.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct RetainedDiskNumber(u16);

impl RetainedDiskNumber {
    pub const fn new(number: u16) -> Option<Self> {
        if number < RETAINED_DISK_COUNT {
            Some(Self(number))
        } else {
            None
        }
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for RetainedDiskNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}{}",
            char::from(ALPHABET[usize::from(self.0 / 36)]),
            char::from(ALPHABET[usize::from(self.0 % 36)])
        )
    }
}

fn digit(byte: u8) -> Option<u16> {
    match byte {
        b'0'..=b'9' => Some(u16::from(byte - b'0')),
        b'a'..=b'z' => Some(u16::from(byte - b'a') + 10),
        _ => None,
    }
}

impl FromStr for RetainedDiskNumber {
    type Err = RetainedNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let [first, second] = value.as_bytes() else {
            return Err(RetainedNameError::DiskNumber);
        };
        let first = digit(*first).ok_or(RetainedNameError::DiskNumber)?;
        let second = digit(*second).ok_or(RetainedNameError::DiskNumber)?;
        Ok(Self(first * 36 + second))
    }
}

impl Serialize for RetainedDiskNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RetainedDiskNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedKind {
    Repository,
    File,
}

impl RetainedKind {
    fn byte(self) -> u8 {
        match self {
            Self::Repository => b'0',
            Self::File => b'1',
        }
    }
}

/// The generated component may exceed PortableName's 240-byte user-name cap.
/// Only its suffix is a PortableName; actual NAME_MAX remains a held-root gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedName {
    encoded: String,
    number: RetainedDiskNumber,
    kind: RetainedKind,
}

fn valid_stamp(stamp: &[u8]) -> bool {
    let [year1, year2, month, day, hour, minute1, minute2] = stamp else {
        return false;
    };
    year1.is_ascii_digit()
        && year2.is_ascii_digit()
        && digit(*month).is_some_and(|value| (1..=12).contains(&value))
        && digit(*day).is_some_and(|value| (1..=31).contains(&value))
        && digit(*hour).is_some_and(|value| value <= 23)
        && (b'0'..=b'5').contains(minute1)
        && minute2.is_ascii_digit()
}

impl RetainedName {
    pub fn parse(value: &str) -> Result<Self, RetainedNameError> {
        let prefix = value
            .as_bytes()
            .get(..RETAINED_PREFIX_BYTES)
            .ok_or(RetainedNameError::Prefix)?;
        if !prefix.is_ascii()
            || prefix[15] != b'-'
            || !valid_stamp(&prefix[3..10])
            || !prefix[10..15].iter().all(|byte| digit(*byte).is_some())
        {
            return Err(RetainedNameError::Prefix);
        }
        let number = value[..2].parse()?;
        let kind = match prefix[2] {
            b'0' => RetainedKind::Repository,
            b'1' => RetainedKind::File,
            _ => return Err(RetainedNameError::Prefix),
        };
        Name::parse(&value[RETAINED_PREFIX_BYTES..])
            .map_err(|source| RetainedNameError::OriginalName { source })?;
        Ok(Self {
            encoded: value.into(),
            number,
            kind,
        })
    }

    /// Sample only for a new, unrecorded original operation. Collision checking,
    /// exclusive installation and persistence of the exact result are mandatory.
    pub fn sample(
        number: RetainedDiskNumber,
        kind: RetainedKind,
        utc_stamp: &str,
        original: &Name,
    ) -> Result<Self, RetainedNameError> {
        if !valid_stamp(utc_stamp.as_bytes()) {
            return Err(RetainedNameError::Prefix);
        }
        let mut random = String::with_capacity(5);
        // Rejection avoids a biased alphabet. This bounded sampler is not the
        // uniqueness proof: the producer must still hold and check its target.
        for _ in 0..16 {
            let mut bytes = [0; 8];
            system_entropy::fill(&mut bytes)
                .map_err(|source| RetainedNameError::Entropy { source })?;
            for byte in bytes.into_iter().filter(|byte| *byte < 252) {
                random.push(char::from(ALPHABET[usize::from(byte % 36)]));
                if random.len() == 5 {
                    return Ok(Self {
                        encoded: format!(
                            "{number}{}{utc_stamp}{random}-{original}",
                            char::from(kind.byte())
                        ),
                        number,
                        kind,
                    });
                }
            }
        }
        Err(RetainedNameError::RandomDraws)
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    pub fn original_name(&self) -> &str {
        &self.encoded[RETAINED_PREFIX_BYTES..]
    }

    pub fn utc_stamp(&self) -> &str {
        &self.encoded[3..10]
    }

    pub fn number(&self) -> RetainedDiskNumber {
        self.number
    }

    pub fn kind(&self) -> RetainedKind {
        self.kind
    }
}

impl Serialize for RetainedName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RetainedName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(deserializer)?)
            .map_err(data_serialization::de::Error::custom)
    }
}

/// One shared pool-level fact. Later operations repeat this exact original
/// registration; retirement of a worktree/container must not release its number.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RetainedDiskRegistration {
    pub pool: PoolId,
    pub disk: DiskId,
    pub number: RetainedDiskNumber,
    pub operation: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RetainedAllocation {
    pub registration: RetainedDiskRegistration,
    pub created_at: Timestamp,
    pub name: RetainedName,
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/retained_names.rs"]
mod tests;
