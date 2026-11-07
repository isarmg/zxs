//! Directory snapshot rows are authoritative in the permanent native receipt.
//! The physical tree is a recoverable projection on the same metadata members.
use super::*;
use crate::disk::{DiskResources, MAX_METADATA};
use crate::engine::{PlanningContext, ValidatedConfig};
use crate::journal::PreparedMetadataObject;
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;
use std::path::PathBuf;

const FORMAT: &str = "recursive-backup-directory-namespace";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DirectoryRow {
    entry_id: String,
    path: PortablePath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Replica {
    identity: crate::DiskRegistrationIdentity,
    directories: Vec<DirectoryRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DirectoryNamespace {
    format: String,
    replicas: BTreeMap<String, Replica>,
}

/// A directory in a successfully published, still-current policy snapshot.
/// Later namespace edits do not silently rewrite this frozen path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedBackupDirectory {
    repository_id: String,
    history_tree_id: String,
    directory_id: String,
    path: PortablePath,
    operation_id: OperationId,
}
impl PublishedBackupDirectory {
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }
    pub fn history_tree_id(&self) -> &str {
        &self.history_tree_id
    }
    pub fn directory_id(&self) -> &str {
        &self.directory_id
    }
    pub fn path(&self) -> &PortablePath {
        &self.path
    }
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

/// Capture actual selected directories without inventing zero-byte file versions.
/// Independent child policies keep their own namespace and route requirements.
pub(super) fn capture(
    current: &CurrentNamespaces,
    state: &State,
    repository: &Repository,
    scope: &Scope,
    policy: &ProtectionPolicy,
    prefixes: &BTreeMap<String, String>,
) -> Result<json_codec::Value> {
    let mut result = DirectoryNamespace {
        format: FORMAT.into(),
        replicas: BTreeMap::new(),
    };
    let head = namespace(state, &repository.repository_id)?;
    let entries = current.branch_entries(&repository.main_branch_id, &head.commit_id)?;
    let previous = exact_policy(state, &repository.repository_id, scope);
    let mut rows = Vec::new();
    if scope.path.is_empty() {
        rows.push((scope.id.as_str(), scope.path.as_str()));
    }
    rows.extend(
        entries
            .iter()
            .filter(|entry| {
                entry.kind == NamespaceEntryKind::Directory
                    && within(&scope.path, entry.path())
                    && inherited_by_scope(
                        entries,
                        &scope.path,
                        previous,
                        effective_policy_in(
                            state,
                            entries,
                            &repository.repository_id,
                            None,
                            entry.path(),
                        ),
                    )
            })
            .map(|entry| (entry.entry_id.as_str(), entry.path())),
    );
    for (entry_id, source_path) in rows {
        let primary = configured_primary(state, &repository.repository_id, source_path);
        let route = current.backup_configuration_route(primary, policy.target_copies)?;
        let relative = relative_path(&scope.path, source_path, false);
        for slot in route.into_iter().skip(1) {
            let disk = current.backup_configuration_disk(&slot)?;
            if !disk.original_metadata_member {
                return Err(Error::new(
                    ErrorCode::OperationUnsupported,
                    "directory namespace publication currently requires metadata-member targets",
                ));
            }
            let prefix = &prefixes[&slot];
            let path = if relative.is_empty() {
                PortablePath::parse(prefix)?
            } else {
                PortablePath::parse(&backup_path(prefix, &relative)?)?
            };
            result
                .replicas
                .entry(slot)
                .or_insert_with(|| Replica {
                    identity: disk.identity.clone(),
                    directories: Vec::new(),
                })
                .directories
                .push(DirectoryRow {
                    entry_id: entry_id.into(),
                    path,
                });
        }
    }
    for replica in result.replicas.values_mut() {
        replica
            .directories
            .sort_by(|a, b| a.path.key().cmp(b.path.key()));
    }
    Ok(json_codec::to_value(result)?)
}

fn empty_namespace_receipt(receipt: &OperationResult) -> bool {
    receipt.kind == recursive::KIND
        && receipt.result["scope"]["kind"] == "directory"
        && receipt.result["file_count"] == "0"
}

fn receipt_namespace(receipt: &OperationResult) -> Result<Option<DirectoryNamespace>> {
    if !empty_namespace_receipt(receipt) {
        return Ok(None);
    }
    let value = receipt.result.get("directory_namespace").ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "recursive backup receipt lacks its directory namespace",
        )
    })?;
    let value: DirectoryNamespace = json_codec::from_value(value.clone())?;
    if value.format != FORMAT {
        return Err(Error::new(
            ErrorCode::PublicationInvalid,
            "unsupported directory namespace format",
        ));
    }
    Ok(Some(value))
}

fn target<'a>(
    config: &'a ValidatedConfig,
    replica: &Replica,
    slot: &str,
) -> Result<&'a DiskConfig> {
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::SlotNotFound,
                "directory backup target no longer belongs to this generation",
            )
        })?;
    if replica.identity.pool_id != config.pool_id
        || replica.identity.slot_id != disk.slot_id
        || replica.identity.disk_id != disk.disk_id
        || replica.identity.binding_generation != disk.binding_generation
    {
        return Err(Error::new(
            ErrorCode::PlacementInvalid,
            "directory backup target identity changed",
        ));
    }
    Ok(disk)
}

fn base_path(disk: &DiskConfig, receipt: &OperationResult) -> Result<PathBuf> {
    let history = receipt.history_tree_id.as_deref().ok_or_else(|| {
        Error::new(
            ErrorCode::PublicationInvalid,
            "directory publication needs a history identity",
        )
    })?;
    name_policy::validate_name(history)?;
    Ok(disk
        .root
        .join("histories")
        .join(history)
        .join("directory-backups")
        .join(receipt.operation_id.to_string()))
}

/// This does not create anything. Every target is already one of the fixed
/// candidate metadata members, so ordinary held seals and full publication
/// barriers also cover a request with no file payloads.
pub(crate) fn preflight(context: &PlanningContext<'_>, receipt: &OperationResult) -> Result<()> {
    let Some(namespace) = receipt_namespace(receipt)? else {
        return Ok(());
    };
    if namespace.replicas.is_empty() {
        return Ok(());
    }
    let record = PreparedMetadataObject::new("directory-backup", receipt, MAX_METADATA)?;
    for (slot, replica) in &namespace.replicas {
        let disk = target(context.config, replica, slot)?;
        if !disk.metadata_member {
            return Err(Error::new(
                ErrorCode::OperationUnsupported,
                "new directory publication requires current metadata-member targets",
            ));
        }
        if replica.directories.len() > context.config.max_entries {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "directory backup exceeds its entry budget",
            ));
        }
        let components = replica.directories.iter().try_fold(0_u64, |sum, row| {
            sum.checked_add(row.path.as_str().split('/').count() as u64 + 8)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "directory projection size overflow",
                    )
                })
        })?;
        let reserve = components
            .checked_mul(4096)
            .and_then(|size| size.checked_add(record.bytes().len() as u64))
            .and_then(|size| size.checked_add(context.config.metadata_reserve_bytes))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "directory projection budget overflow",
                )
            })?;
        if context.disk_resources.get(disk)?.root.available()? < reserve {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "directory backup target lacks its metadata reserve",
            ));
        }
    }
    Ok(())
}

/// Only the verified decided/published candidate calls this projection writer.
/// Ordinary failed planning and uncommitted receipts never create a directory.
/// Repeated calls establish ancestor barriers again after an interrupted fsync.
pub(crate) fn materialize(
    config: &ValidatedConfig,
    resources: &DiskResources,
    receipt: &OperationResult,
) -> Result<()> {
    let Some(namespace) = receipt_namespace(receipt)? else {
        return Ok(());
    };
    if namespace.replicas.is_empty() {
        return Ok(());
    }
    let record = PreparedMetadataObject::new("directory-backup", receipt, MAX_METADATA)?;
    for (slot, replica) in &namespace.replicas {
        // A published prefix may outlive deregistration or a slot rebind.
        // Never project its old identity onto another disk or reopen a stopped one.
        let Some(disk) = config.disks.iter().find(|disk| {
            disk.slot_id == *slot
                && disk.disk_id == replica.identity.disk_id
                && disk.binding_generation == replica.identity.binding_generation
                && config.pool_id == replica.identity.pool_id
        }) else {
            continue;
        };
        let Some(handle) = resources.available(disk) else {
            continue;
        };
        let base = base_path(disk, receipt)?;
        for row in &replica.directories {
            handle
                .root
                .private_dir(&base.join("namespace").join(row.path.as_str()))?;
        }
        handle
            .root
            .write_atomic(&base.join("record.json"), record.bytes(), false)?;
    }
    Ok(())
}

fn current_namespace(
    state: &State,
    receipt: &OperationResult,
) -> Result<Option<(DirectoryNamespace, ProtectionPolicy)>> {
    if !empty_namespace_receipt(receipt) {
        return Ok(None);
    }
    let policy: ProtectionPolicy = json_codec::from_value(receipt.result["policy"].clone())?;
    if state.protection_policies.get(&policy.policy_id) != Some(&policy) {
        return Ok(None);
    }
    Ok(receipt_namespace(receipt)?.map(|namespace| (namespace, policy)))
}

/// Only the fully replayed published prefix repairs current projection caches.
/// Do not replay historical physical trees on every old operation.
pub(crate) fn repair_current(
    config: &ValidatedConfig,
    resources: &DiskResources,
    state: &State,
) -> Result<()> {
    for receipt in state.operations.values() {
        if current_namespace(state, receipt)?.is_some() {
            materialize(config, resources, receipt)?;
        }
    }
    Ok(())
}

impl Engine {
    /// Read frozen directories from permanent published receipts, including
    /// directories with no file versions. Callers must apply their own UI scope.
    ///
    /// # Errors
    /// Fails while current registry admission is unavailable, or if stored
    /// directory evidence uses an unsupported format or exceeds the read budget.
    pub fn published_backup_directories(
        &self,
        slot: &str,
    ) -> Result<Vec<PublishedBackupDirectory>> {
        self.admitted_current_namespaces()?.ok_or_else(|| {
            Error::new(
                ErrorCode::OperationUnsupported,
                "directory backup reads require current registry admission",
            )
        })?;
        let disk = self.disk(slot)?;
        let mut entries = Vec::new();
        for receipt in self.state().operations.values() {
            let Some((namespace, policy)) = current_namespace(self.state(), receipt)? else {
                continue;
            };
            let Some(replica) = namespace.replicas.get(&disk.slot_id) else {
                continue;
            };
            if replica.identity.pool_id != self.config().pool_id
                || replica.identity.disk_id != disk.disk_id
                || replica.identity.binding_generation != disk.binding_generation
            {
                continue;
            }
            let history_tree_id = receipt.history_tree_id.as_ref().ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "published directory history is missing",
                )
            })?;
            for row in &replica.directories {
                if entries.len() >= self.config().max_entries {
                    return Err(Error::new(
                        ErrorCode::ResourceLimit,
                        "published directory read exceeds its entry budget",
                    ));
                }
                entries.push(PublishedBackupDirectory {
                    repository_id: policy.repository_id.clone(),
                    history_tree_id: history_tree_id.clone(),
                    directory_id: row.entry_id.clone(),
                    path: row.path.clone(),
                    operation_id: receipt.operation_id,
                });
            }
        }
        Ok(entries)
    }
}
