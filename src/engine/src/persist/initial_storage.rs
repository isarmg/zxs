//! Preserve the existing Server activation seal digest bytes. The public
//! receipt hashed the decoded JSON value, not the typed seal's field order.
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct InitialSealDigest(blake3::Hash);

impl fmt::Display for InitialSealDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

pub(crate) fn seal_digest(bytes: &[u8]) -> crate::Result<InitialSealDigest> {
    let wire = storage_contract::strict_json(bytes)?;
    Ok(InitialSealDigest(blake3::hash(&json_codec::to_vec(&wire)?)))
}
