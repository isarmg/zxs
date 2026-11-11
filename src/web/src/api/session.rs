//! Authentication response admission. Secrets never enter the public UI model.
use data_serialization::{Deserialize, Deserializer, de};
use json_codec::{Value, error::Category};
use std::num::NonZeroU64;
use storage_contract::Principal;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct LoginResponse {
    pub principal: Principal,
    pub csrf_token: SessionSecret,
    pub token: SessionSecret,
    #[serialization(rename = "expires_at", deserialize_with = "positive_expiry")]
    _expires_at: NonZeroU64,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SessionResponse {
    pub principal: Principal,
    pub csrf_token: SessionSecret,
}

// Deliberately no Debug: both session and CSRF tokens are secrets.
pub(super) struct SessionSecret(pub(super) String);

impl<'de> Deserialize<'de> for SessionSecret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() {
            return Err(de::Error::custom("session secret is empty"));
        }
        Ok(Self(value))
    }
}

fn positive_expiry<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NonZeroU64, D::Error> {
    let value = String::deserialize(deserializer)?;
    let expiry = storage_contract::parse_u64_decimal(&value).map_err(de::Error::custom)?;
    NonZeroU64::new(expiry).ok_or_else(|| de::Error::custom("session expiry is zero"))
}

/// Retain only the decoder category: its raw message can contain credentials
/// or attacker-controlled field names, including through Debug and source().
#[derive(Debug, error_definition::Error)]
#[error("authentication response decoding failed ({category:?})")]
pub struct DecodeError {
    category: Category,
}

pub(super) fn decode<T: de::DeserializeOwned>(value: Value) -> Result<T, DecodeError> {
    let Value::Object(object) = value else {
        return Err(DecodeError {
            category: Category::Data,
        });
    };
    T::deserialize(object).map_err(|source| DecodeError {
        category: source.classify(),
    })
}
