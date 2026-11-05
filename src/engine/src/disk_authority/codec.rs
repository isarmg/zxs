use super::{Digest, Error, Result};
use data_serialization::{Serialize, de::DeserializeOwned};

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    // Value's object map is ordered. The independent fixture tests also compare
    // exact canonical bytes, not merely logical equality.
    Ok(json_codec::to_vec(&json_codec::to_value(value)?)?)
}

pub(crate) fn decode<T: DeserializeOwned + Serialize>(raw: &[u8]) -> Result<T> {
    let value = json_codec::from_slice(raw)?;
    if encode(&value)? != raw {
        return Err(Error::Invalid("noncanonical or duplicate-key JSON"));
    }
    Ok(value)
}

pub(crate) fn address(kind: &str, bytes: &[u8]) -> Digest {
    let mut hash = blake3::Hasher::new();
    hash.update(format!("zxs-independent-disk:{kind}\0").as_bytes());
    hash.update(bytes);
    Digest(*hash.finalize().as_bytes())
}
