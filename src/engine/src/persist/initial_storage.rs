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

impl InitialSealDigest {
    /// Reject an inconsistent *recorded* initial seal. This comparison does not
    /// observe a disk, produce an original proof, or authorize creation/open.
    /// Only compare after the existing original intent, ready and completion
    /// are actually present. Later membership bindings are not first storage.
    ///
    /// # Errors
    /// Reports encoding errors without converting them into a matching digest.
    pub fn matches_recorded_initial_selection(
        config: &crate::ValidatedConfig,
        disk: storage_contract::DiskId,
        root: storage_contract::working_tree::LocalEntityIdentity,
        recorded: &str,
    ) -> crate::Result<bool> {
        if config.generation().is_some() {
            return Ok(false);
        }
        let id = disk.to_string();
        let Some(disk) = config.disks.iter().find(|disk| disk.disk_id == id) else {
            return Ok(false);
        };
        let root = platform_io::fs_safety::linux::FileIdentity {
            device: root.device(), inode: root.inode(),
        };
        let seal = crate::disk::seal_for_identity(config, disk, root);
        Ok(seal_digest(&storage_contract::canonical_json(&seal)?)?.to_string() == recorded)
    }
}
