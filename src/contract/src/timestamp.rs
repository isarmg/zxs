use data_serialization::{Deserialize, Serialize};
use std::{
    fmt,
    str::FromStr,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

/// Whole seconds since the Unix epoch, encoded as an unsigned decimal string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct Timestamp(#[serialization(with = "crate::u64_string")] u64);

#[derive(Debug, error_definition::Error)]
pub enum TimestampError {
    #[error("system clock precedes the Unix epoch")]
    BeforeUnixEpoch {
        #[source]
        source: SystemTimeError,
    },
}

impl Timestamp {
    pub const fn from_unix_seconds(seconds: u64) -> Self {
        Self(seconds)
    }

    pub const fn unix_seconds(self) -> u64 {
        self.0
    }

    pub fn now() -> Result<Self, TimestampError> {
        Self::try_from(SystemTime::now())
    }
}

impl TryFrom<SystemTime> for Timestamp {
    type Error = TimestampError;

    fn try_from(time: SystemTime) -> Result<Self, Self::Error> {
        let elapsed = time
            .duration_since(UNIX_EPOCH)
            .map_err(|source| TimestampError::BeforeUnixEpoch { source })?;
        Ok(Self::from_unix_seconds(elapsed.as_secs()))
    }
}

impl FromStr for Timestamp {
    type Err = crate::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::parse_u64_decimal(value).map(Self)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/timestamp.rs"]
mod tests;
