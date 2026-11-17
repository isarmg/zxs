//! Plaintext input ownership, separate from password policy and authorization.
use data_serialization::{Deserialize, Deserializer};
use std::fmt;
use zeroize::Zeroizing;

/// An untrusted password whose owned buffer is erased when its owner is dropped.
///
/// No normalization or policy check happens here: administrator and Samba inputs
/// have different policies and existing error/admission ordering must be preserved.
/// HTTP body and decoder intermediates may still have other copies of the input.
/// This type protects its own buffer, not every copy made by a transport or caller.
///
/// Plaintext cannot be serialized or printed accidentally. Only the caller at a
/// credential-verification or password-client boundary should call `expose`.
///
/// ```compile_fail,E0277
/// let password = authentication::SecretPassword::from("not-public");
/// let _ = json_codec::to_string(&password);
/// ```
pub struct SecretPassword(Zeroizing<String>);

impl SecretPassword {
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl From<String> for SecretPassword {
    fn from(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl From<&str> for SecretPassword {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl fmt::Debug for SecretPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretPassword([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for SecretPassword {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
#[path = "../../../test/auth/unit/secret.rs"]
mod tests;
