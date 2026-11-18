//! Exact canonical byte and size admission for the new typed objects. The
//! radix pages continue to use the original metadata_map encoder and domains.
use crate::persist::layout;
use crate::{
    Result,
    disk::{DiskRoot, read_file_bytes},
    engine::scope_closure::ScopeClosureError,
};
use data_serialization::{Serialize, de::DeserializeOwned};
use storage_contract::working_tree::MaterialCommitment;

pub(super) use layout::ScopeObjectDomain as Domain;
impl Domain {
    fn maximum(self) -> usize {
        match self {
            Self::Closure | Self::DirectoryRepository => 8 * 1024,
            Self::Disk | Self::System | Self::EmptyInventoryPage => 64 * 1024,
        }
    }
}

pub(super) struct PreparedObject {
    domain: Domain,
    hash: MaterialCommitment,
    bytes: Vec<u8>,
}
impl PreparedObject {
    pub(super) fn new(domain: Domain, value: &impl Serialize) -> Result<Self> {
        let (bytes, digest) =
            storage_contract::encode_canonical_object_digest(domain.name(), value)?;
        if bytes.len() > domain.maximum() {
            return Err(ScopeClosureError::Budget.into());
        }
        Ok(Self {
            domain,
            hash: MaterialCommitment::from_digest(digest),
            bytes,
        })
    }
    pub(super) fn hash(&self) -> MaterialCommitment {
        self.hash
    }
    pub(super) fn bytes(&self) -> usize {
        self.bytes.len()
    }
    pub(super) fn install(&self, root: &DiskRoot) -> Result<()> {
        let path = layout::scope_descriptor(self.domain, self.hash);
        root.write_relative_atomic(&path, &self.bytes, false)?;
        let held = root
            .open_optional_read_relative(&path)?
            .ok_or(ScopeClosureError::GenesisMismatch)?;
        if read_file_bytes(held, self.bytes.len())? != self.bytes {
            return Err(ScopeClosureError::GenesisMismatch.into());
        }
        Ok(())
    }
}

pub(super) fn load<T: DeserializeOwned + Serialize>(
    root: &DiskRoot,
    domain: Domain,
    hash: MaterialCommitment,
) -> Result<T> {
    let path = layout::scope_descriptor(domain, hash);
    let file = root
        .open_optional_read_relative(&path)?
        .ok_or(ScopeClosureError::GenesisMismatch)?;
    let bytes = read_file_bytes(file, domain.maximum())?;
    let value: T = storage_contract::decode_strict(&bytes)?;
    let admitted = PreparedObject::new(domain, &value)?;
    if admitted.hash != hash || admitted.bytes != bytes {
        return Err(ScopeClosureError::GenesisMismatch.into());
    }
    Ok(value)
}
