//! Validated component spellings retain the exact admitted bytes.
use crate::{Error, Result, lookup_key, validate_lookup_name, validate_name, validate_slot};

/// A creation name whose spelling satisfies the fixed-Unicode NFC policy.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Box<str>);

impl Name {
    pub fn parse(name: &str) -> Result<Self> {
        validate_name(name)?;
        Ok(Self(name.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> String {
        lookup_key(self.as_str())
    }
}

/// An existing-name reference, including canonical-equivalent NFD spelling.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LookupName(Box<str>);

impl LookupName {
    pub fn parse(name: &str) -> Result<Self> {
        validate_lookup_name(name)?;
        Ok(Self(name.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> String {
        lookup_key(self.as_str())
    }
}

impl From<Name> for LookupName {
    fn from(name: Name) -> Self {
        Self(name.0)
    }
}

impl From<&Name> for LookupName {
    fn from(name: &Name) -> Self {
        Self(name.0.clone())
    }
}

impl TryFrom<LookupName> for Name {
    type Error = Error;

    fn try_from(name: LookupName) -> Result<Self> {
        use crate::unicode::UnicodeNormalization;

        if !crate::unicode::is_nfc(name.as_str()) {
            return Err(Error::NotNormalized(name.as_str().nfc().collect()));
        }
        Ok(Self(name.0))
    }
}

/// An ASCII storage slot identifier, preserving its case and spelling.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(Box<str>);

impl SlotId {
    pub fn parse(slot: &str) -> Result<Self> {
        validate_slot(slot)?;
        Ok(Self(slot.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! text_traits {
    ($name:ident) => {
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = Error;

            fn from_str(raw: &str) -> Result<Self> {
                Self::parse(raw)
            }
        }

        impl data_serialization::Serialize for $name {
            fn serialize<S: data_serialization::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> data_serialization::Deserialize<'de> for $name {
            fn deserialize<D: data_serialization::Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let raw = <String as data_serialization::Deserialize>::deserialize(deserializer)?;
                Self::parse(&raw).map_err(data_serialization::de::Error::custom)
            }
        }
    };
}

text_traits!(Name);
text_traits!(LookupName);
text_traits!(SlotId);
