//! Real empty DISK databases have a registration, an empty native change
//! prefix and a byte commitment; a missing file never stands for that prefix.
use crate::Result;
use crate::disk::DiskRoot;
use crate::engine::disk_catalog::{
    DiskCatalogSnapshot, PreparedDiskScopes, database,
    history::{self, StoredHistory},
};
use crate::engine::scope_closure::{PreparedImage, Representation, Schema, ScopeClosureError};
use storage_contract::working_tree::{DiskRegistrationIdentity, MaterialCommitment};

pub(in crate::engine) struct GenesisDiskImages {
    current: DiskCatalogSnapshot,
    history: StoredHistory,
    current_image: PreparedImage,
    history_image: PreparedImage,
}
impl GenesisDiskImages {
    pub(in crate::engine) fn prepare(identity: DiskRegistrationIdentity) -> Result<Self> {
        let (current, history) = empty_rows(identity);
        let (current_bytes, history_bytes) = PreparedDiskScopes::encode_pair(&current, &history)?;
        Ok(Self {
            current,
            history,
            current_image: PreparedImage::new(Schema::DiskCurrent, current_bytes),
            history_image: PreparedImage::new(Schema::DiskHistory, history_bytes),
        })
    }

    pub(in crate::engine) fn bytes(&self) -> usize {
        self.current_image.bytes() + self.history_image.bytes()
    }

    pub(in crate::engine) fn current(&self) -> Result<(MaterialCommitment, Representation)> {
        Ok((
            MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
                "disk-current-catalog",
                &self.current,
            )?),
            self.current_image.representation(),
        ))
    }

    pub(in crate::engine) fn history(&self) -> Result<(MaterialCommitment, Representation)> {
        Ok((
            MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
                "disk-local-inventory-changes",
                &self.history,
            )?),
            self.history_image.representation(),
        ))
    }

    pub(in crate::engine) fn install(&self, root: &DiskRoot) -> Result<()> {
        self.current_image.install_verified(root, |bytes| {
            if database::read(bytes, &self.current.identity)? != self.current {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        })?;
        self.history_image.install_verified(root, |bytes| {
            if history::read_for_root_install(bytes, &self.current.identity)? != self.history {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        })
    }
}

fn empty_rows(identity: DiskRegistrationIdentity) -> (DiskCatalogSnapshot, StoredHistory) {
    let current = DiskCatalogSnapshot::empty(identity.clone());
    let history = StoredHistory::genesis(identity);
    (current, history)
}

/// Verify both the captured logical rows and the original bytes on this held
/// root. An equal SQL projection serialized today is not the captured image.
pub(in crate::engine) fn verify_original(
    root: &DiskRoot,
    identity: &DiskRegistrationIdentity,
    current_root: MaterialCommitment,
    current_image: Representation,
    history_root: MaterialCommitment,
    history_image: Representation,
) -> Result<()> {
    let (current, history) = empty_rows(identity.clone());
    if MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
        "disk-current-catalog",
        &current,
    )?) != current_root
        || MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
            "disk-local-inventory-changes",
            &history,
        )?) != history_root
    {
        return Err(ScopeClosureError::ImageMismatch.into());
    }
    current_image.verify(
        root,
        Schema::DiskCurrent,
        super::MAX_DATABASE_BYTES,
        |bytes| {
            if database::read(bytes, identity)? != current {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        },
    )?;
    history_image.verify(
        root,
        Schema::DiskHistory,
        super::MAX_DATABASE_BYTES,
        |bytes| {
            if history::read_for_root_install(bytes, identity)? != history {
                return Err(ScopeClosureError::ImageMismatch.into());
            }
            Ok(())
        },
    )
}
