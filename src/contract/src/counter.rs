use data_serialization::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// A revision counter encoded as a full-width unsigned decimal string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct Generation(#[serialization(with = "crate::u64_string")] u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error("generation counter is exhausted")]
pub struct GenerationExhausted;

impl Generation {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Imported counters can occupy the full u64 range, including its final value.
    pub fn next(self) -> Result<Self, GenerationExhausted> {
        self.0.checked_add(1).map(Self).ok_or(GenerationExhausted)
    }
}

impl FromStr for Generation {
    type Err = crate::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::parse_u64_decimal(value).map(Self)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

/// A publication or slot counter encoded as a full-width unsigned decimal string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct Sequence(#[serialization(with = "crate::u64_string")] u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error("sequence counter is exhausted")]
pub struct SequenceExhausted;

impl Sequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    /// Imported counters can occupy the full u64 range, including its final value.
    pub fn next(self) -> Result<Self, SequenceExhausted> {
        self.0.checked_add(1).map(Self).ok_or(SequenceExhausted)
    }
}

impl FromStr for Sequence {
    type Err = crate::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::parse_u64_decimal(value).map(Self)
    }
}

impl fmt::Display for Sequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/counter.rs"]
mod tests;
