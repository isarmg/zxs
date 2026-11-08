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
const FOLLOW_FORMAT: &str = "recursive-directory-namespace-follow";
pub(crate) const FILE_FOLLOW_KIND: &str = "file.namespace.recursive-directories";
pub(crate) const BATCH_FOLLOW_KIND: &str = "directory.namespace.batch.recursive-directories";

pub(crate) const FILE_ROUTE_KIND: &str = "file.namespace.recursive-routes";
pub(crate) const BATCH_ROUTE_KIND: &str = "directory.namespace.batch.recursive-routes";

pub(crate) fn follows_routes(kind: &str) -> bool {
    matches!(kind, FILE_ROUTE_KIND | BATCH_ROUTE_KIND)
}

pub(crate) fn follows_directories(kind: &str) -> bool {
    matches!(
        kind,
        FILE_FOLLOW_KIND | BATCH_FOLLOW_KIND | FILE_ROUTE_KIND | BATCH_ROUTE_KIND
    )
}

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
pub(crate) struct DirectoryNamespace {
    format: String,
    replicas: BTreeMap<String, Replica>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DirectoryFollowChange {
    pub(crate) source_operation_id: OperationId,
    pub(crate) policy: ProtectionPolicy,
    pub(crate) namespace: DirectoryNamespace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DirectoryFollowPlan {
    format: String,
    pub(crate) policies: BTreeMap<String, DirectoryFollowChange>,
}

impl DirectoryFollowPlan {
    pub(crate) fn new() -> Self {
        Self {
            format: FOLLOW_FORMAT.into(),
            policies: BTreeMap::new(),
        }
    }

    pub(crate) fn from_receipt(receipt: &OperationResult) -> Result<Self> {
        let plan: Self =
            json_codec::from_value(receipt.result["recursive_directory_follow"].clone())?;
        if plan.format != FOLLOW_FORMAT {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "unsupported directory follow format",
            ));
        }
        Ok(plan)
    }
}

impl DirectoryNamespace {
    pub(crate) fn entry_ids(&self) -> impl Iterator<Item = &String> {
        self.replicas
            .values()
            .flat_map(|replica| replica.directories.iter().map(|row| &row.entry_id))
    }

    /// Stable IDs and original roots bind each changed path to its old snapshot.
    /// Unselected/new directories are not silently captured by a rename.
    pub(crate) fn follow(
        &self,
        moves: &BTreeMap<String, (String, String)>,
        roots: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let mut next = self.clone();
        for (slot, replica) in &mut next.replicas {
            let root = roots.get(slot).ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "directory follow target lacks its captured root",
                )
            })?;
            for row in &mut replica.directories {
                if let Some((before, after)) = moves.get(&row.entry_id) {
                    if row.path.as_str() != format!("{root}/{before}") {
                        return Err(Error::new(
                            ErrorCode::ScopeChanged,
                            "directory backup no longer matches its primary path; configure it again",
                        ));
                    }
                    row.path = PortablePath::parse(&format!("{root}/{after}"))?;
                }
            }
            replica
                .directories
                .sort_by(|a, b| a.path.key().cmp(b.path.key()));
        }
        Ok(next)
    }
}

/// A directory in a successfully published, still-current policy snapshot.
/// Native follow publications supersede the current snapshot without editing old receipts.
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

fn directory_namespace_receipt(receipt: &OperationResult) -> bool {
    receipt.kind == recursive::TREE_KIND
        || (receipt.kind == recursive::KIND
            && receipt.result["scope"]["kind"] == "directory"
            && receipt.result["file_count"] == "0")
}

pub(crate) fn captured_directory_namespace(
    receipt: &OperationResult,
) -> Result<Option<DirectoryNamespace>> {
    if !directory_namespace_receipt(receipt) {
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

fn receipt_namespaces(
    receipt: &OperationResult,
) -> Result<Vec<(ProtectionPolicy, DirectoryNamespace)>> {
    if let Some(namespace) = captured_directory_namespace(receipt)? {
        return Ok(vec![(
            json_codec::from_value(receipt.result["policy"].clone())?,
            namespace,
        )]);
    }
    if follows_directories(&receipt.kind) {
        return Ok(DirectoryFollowPlan::from_receipt(receipt)?
            .policies
            .into_values()
            .map(|change| (change.policy, change.namespace))
            .collect());
    }
    Ok(Vec::new())
}

fn combined(namespaces: Vec<(ProtectionPolicy, DirectoryNamespace)>) -> Result<DirectoryNamespace> {
    let mut result = DirectoryNamespace {
        format: FORMAT.into(),
        replicas: BTreeMap::new(),
    };
    for (_, namespace) in namespaces {
        for (slot, replica) in namespace.replicas {
            if let Some(existing) = result.replicas.get_mut(&slot) {
                if existing.identity != replica.identity {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "directory publications disagree on target identity",
                    ));
                }
                existing.directories.extend(replica.directories);
            } else {
                result.replicas.insert(slot, replica);
            }
        }
    }
    for replica in result.replicas.values_mut() {
        replica.directories.sort_by(|a, b| {
            a.entry_id
                .cmp(&b.entry_id)
                .then_with(|| a.path.key().cmp(b.path.key()))
        });
        replica
            .directories
            .dedup_by(|a, b| a.entry_id == b.entry_id && a.path.key() == b.path.key());
        replica
            .directories
            .sort_by(|a, b| a.path.key().cmp(b.path.key()));
    }
    Ok(result)
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
    let namespace = combined(receipt_namespaces(receipt)?)?;
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
    let namespace = combined(receipt_namespaces(receipt)?)?;
    materialize_namespace(config, resources, receipt, namespace)
}

fn materialize_namespace(
    config: &ValidatedConfig,
    resources: &DiskResources,
    receipt: &OperationResult,
    namespace: DirectoryNamespace,
) -> Result<()> {
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

fn current_namespaces(
    state: &State,
    receipt: &OperationResult,
) -> Result<Vec<(ProtectionPolicy, DirectoryNamespace)>> {
    Ok(receipt_namespaces(receipt)?
        .into_iter()
        .filter(|(policy, _)| state.protection_policies.get(&policy.policy_id) == Some(policy))
        .collect())
}

/// Only the fully replayed published prefix repairs current projection caches.
/// Do not replay historical physical trees on every old operation.
pub(crate) fn repair_current(
    config: &ValidatedConfig,
    resources: &DiskResources,
    state: &State,
) -> Result<()> {
    for receipt in state.operations.values() {
        let namespace = combined(current_namespaces(state, receipt)?)?;
        materialize_namespace(config, resources, receipt, namespace)?;
    }
    Ok(())
}

impl Engine {
    /// Read current directories from permanent published receipts, including
    /// directories with no file versions. Callers must apply their own UI scope.
    ///
    /// # Errors
    /// Fails while current registry admission is unavailable, or if stored
    /// directory evidence uses an unsupported format or exceeds the read budget.
    /// An admitted initial pool has no native directory publications and returns
    /// no rows; an awaiting current publication remains an error.
    pub fn published_backup_directories(
        &self,
        slot: &str,
    ) -> Result<Vec<PublishedBackupDirectory>> {
        let current = self.admitted_current_namespaces()?;
        let disk = self.disk(slot)?;
        // Native recursive publication requires CurrentNamespaces. Initial is
        // a distinct admitted state, not a fallback for pending/corrupt current.
        if current.is_none() {
            return Ok(Vec::new());
        }
        let mut entries = BTreeMap::new();
        for receipt in self.state().operations.values() {
            for (policy, namespace) in current_namespaces(self.state(), receipt)? {
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
                    let key = (
                        policy.repository_id.clone(),
                        history_tree_id.clone(),
                        row.entry_id.clone(),
                        row.path.key().to_owned(),
                    );
                    if let Some((sequence, _)) = entries.get(&key) {
                        if *sequence >= receipt.sequence {
                            continue;
                        }
                    } else if entries.len() >= self.config().max_entries {
                        return Err(Error::new(
                            ErrorCode::ResourceLimit,
                            "published directory read exceeds its distinct-entry budget",
                        ));
                    }
                    // This is a logical listing, not a proof inventory. Keep the
                    // newest representative; every source receipt stays permanent.
                    entries.insert(
                        key,
                        (
                            receipt.sequence,
                            PublishedBackupDirectory {
                                repository_id: policy.repository_id.clone(),
                                history_tree_id: history_tree_id.clone(),
                                directory_id: row.entry_id.clone(),
                                path: row.path.clone(),
                                operation_id: receipt.operation_id,
                            },
                        ),
                    );
                }
            }
        }
        let mut entries: Vec<_> = entries.into_values().map(|(_, entry)| entry).collect();
        entries.sort_by(|a, b| {
            a.path.key().cmp(b.path.key()).then_with(|| {
                (&a.repository_id, &a.history_tree_id, &a.directory_id).cmp(&(
                    &b.repository_id,
                    &b.history_tree_id,
                    &b.directory_id,
                ))
            })
        });
        Ok(entries)
    }
}
