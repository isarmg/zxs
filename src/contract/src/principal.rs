use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// The nonempty audit identity whose original text participates in request digests.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Principal(Box<str>);

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
#[error("operation principal is required")]
pub struct EmptyPrincipal;

impl Principal {
    pub fn new(value: Box<str>) -> Result<Self, EmptyPrincipal> {
        if value.is_empty() {
            return Err(EmptyPrincipal);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Principal {
    type Err = EmptyPrincipal;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.into())
    }
}

impl fmt::Display for Principal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for Principal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Principal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Box::<str>::deserialize(deserializer)?;
        Self::new(value).map_err(data_serialization::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/principal.rs"]
mod tests;
