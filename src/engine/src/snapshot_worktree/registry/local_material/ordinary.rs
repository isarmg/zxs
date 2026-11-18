//! The ordinary mkdir and snapshot-host producers share this in-memory
//! identity index. Pending from either producer is not an online registration.
use super::*;
use storage_contract::{DiskId, working_tree::RootBinding};

pub(crate) struct OrdinaryOriginal<'a> {
    pub(crate) repository: RepositoryId, pub(crate) branch: storage_contract::BranchId,
    pub(crate) binding: &'a NamespaceDirectoryBinding, pub(crate) pending: Option<OperationId>,
}
impl ReceiptIndex<'_> {
    pub(crate) fn ordinary_originals(&self, root: &RootBinding)
        -> Result<BTreeMap<DirectoryId, OrdinaryOriginal<'_>>> {
        let disk: DiskId = root.registration.disk_id.parse().map_err(|_| RegistrationError::Identity)?;
        // Non-owner occurrences require their own selected source-chain and
        // stable replica proof. Never erase them to claim an owner-only disk.
        if self.source.namespace_directories.values().any(|record|
            record.owner_disk != disk && record.replicas.contains_key(&disk)
        ) { return Err(RegistrationError::Source); }
        let mut out = BTreeMap::new();
        for (&(root_id, entry), registered) in &self.directories {
            if root_id != root.root_id { continue; }
            let binding = &registered.binding;
            let dir = &binding.directory;
            let repo = self.source.repositories.get(&registered.repository.to_string()).ok_or(RegistrationError::Source)?;
            let branch = repo.main_branch_id.parse().map_err(|_| RegistrationError::Identity)?;
            if dir.root_id != root_id || binding.entry != entry || dir.entity.inode() == 0 || out.len() >= 65_536
                || self.directory_ids.get(&dir.directory_id) != Some(&(root_id, entry))
                || self.directory_entities.get(&(root_id, dir.entity)) != Some(&entry)
                || !self.source.operations.contains_key(&dir.operation_id) {
                return Err(RegistrationError::Source);
            }
            let row = OrdinaryOriginal { repository: registered.repository, branch, binding,
                pending: (!registered.complete).then_some(registered.reservation) };
            if out.insert(dir.directory_id, row).is_some() { return Err(RegistrationError::Identity); }
        }
        Ok(out)
    }
}
