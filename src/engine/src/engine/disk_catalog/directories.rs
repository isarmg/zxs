//! Explicit recursive-directory placements, including directories with no file
//! version. No primary namespace or repository scope is inferred from this data.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskCatalogDirectory {
    pub repository_id: String,
    pub history_tree_id: String,
    pub directory_id: String,
    pub owner_path: name_policy::PortablePath,
    /// The original retained receipt selected at this precise State.
    pub operation_id: OperationId,
}
impl DiskCatalogDirectory {
    pub(super) fn key(&self) -> (&str, &str, &str, &str) {
        (
            &self.repository_id,
            &self.history_tree_id,
            &self.directory_id,
            self.owner_path.key(),
        )
    }

    pub(super) fn validate(&self) -> Result<()> {
        storage_contract::validate_id(&self.repository_id, "repo")?;
        storage_contract::validate_id(&self.history_tree_id, "history")?;
        if self.directory_id != self.repository_id {
            storage_contract::validate_id(&self.directory_id, "entry")?;
        }
        Ok(())
    }
}

pub(super) fn snapshot(
    state: &State,
    identity: &DiskRegistrationIdentity,
    versions: usize,
    bytes: &mut usize,
) -> Result<Vec<DiskCatalogDirectory>> {
    let source = crate::protection::published_directories_at(state, identity, MAX_ROWS - versions)?;
    let mut rows = Vec::with_capacity(source.len());
    for directory in source {
        let row = DiskCatalogDirectory {
            repository_id: directory.repository_id().into(),
            history_tree_id: directory.history_tree_id().into(),
            directory_id: directory.directory_id().into(),
            owner_path: directory.path().clone(),
            operation_id: directory.operation_id(),
        };
        let length = crate::disk::encode_metadata(&row, MAX_RECORD_BYTES)?.len();
        if length > MAX_RECORD_BYTES - *bytes {
            return Err(budget());
        }
        *bytes += length;
        rows.push(row);
    }
    Ok(rows)
}
