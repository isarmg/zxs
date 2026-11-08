//! Native recursive path writes use a frozen, replayable source-name capture.
use super::*;
use crate::{DiskRegistrationIdentity, DiskRegistryCommitment, RegistrationMode};
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;

pub(crate) const KIND: &str = "backup.configure-recursive";
pub(crate) const TREE_KIND: &str = "backup.configure-recursive-tree";
const FORMAT: &str = "recursive-backup-source-paths";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecursiveBackupMode {
    Files,
    DirectoryTree,
}

impl RecursiveBackupMode {
    fn kind(self) -> &'static str {
        match self {
            Self::Files => KIND,
            Self::DirectoryTree => TREE_KIND,
        }
    }

    pub(crate) fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            KIND => Some(Self::Files),
            TREE_KIND => Some(Self::DirectoryTree),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct RecursivePaths<'a> {
    pub(super) capture: &'a RecursiveBackupCapture,
    pub(super) mode: RecursiveBackupMode,
}

/// Transport/persisted request evidence, not a write capability. The live
/// boundary rechecks every identity and name before a new operation is planned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RecursiveBackupCapture {
    format: String,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    source_business_generation: u64,
    registry: DiskRegistryCommitment,
    disks: BTreeMap<String, RecursiveBackupDisk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RecursiveBackupDisk {
    identity: DiskRegistrationIdentity,
    level: u32,
    #[serialization(deserialize_with = "required_upstream")]
    upstream_slot_id: Option<String>,
    name: String,
}

fn required_upstream<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

/// Frozen input for explicit recursive re-protection using full source paths.
/// `configure_recursive_backups` retains its file-only/nonempty scope semantics;
/// `configure_recursive_backup_tree` explicitly captures current directories too.
/// Published directory snapshots require metadata-member targets in both modes.
#[derive(Clone, Copy, Debug)]
pub struct RecursiveBackupConfigurationRequest<'a> {
    pub repository_id: &'a str,
    pub path: &'a str,
    pub expected_scope_id: &'a str,
    pub target_copies: u32,
    pub expected_generation: u64,
    pub capture: &'a RecursiveBackupCapture,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

fn changed() -> Error {
    Error::new(
        ErrorCode::ScopeChanged,
        "recursive backup source namespace, registry or disk names changed; capture again",
    )
}

fn unavailable() -> Error {
    Error::new(
        ErrorCode::OperationUnsupported,
        "recursive backup writes require an admitted current disk registry",
    )
}

impl Engine {
    /// Capture enabled managed disk identities and their actual configured root
    /// names at one publication. Submit this unchanged with the write request.
    /// A retry reuses the original capture; it never samples today's names.
    pub fn recursive_backup_capture(&self) -> Result<RecursiveBackupCapture> {
        self.admitted_current_namespaces()?
            .ok_or_else(unavailable)?;
        let registry = self
            .current_disk_registry_commitment()
            .expect("invariant: admitted current namespaces own a registry")
            .clone();
        let mut disks = BTreeMap::new();
        for registration in self
            .registered_disks()
            .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
        {
            let disk = self.disk(registration.slot_id)?;
            if registration.pool_id != self.config().pool_id
                || registration.disk_id != disk.disk_id
                || registration.binding_generation != disk.binding_generation
                || registration.level != disk.level
                || registration.upstream_slot_id != disk.upstream_slot_id.as_deref()
            {
                return Err(changed());
            }
            let name = disk
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidBackupPrefix,
                        "recursive source disk needs a UTF-8 directory basename",
                    )
                })?;
            name_policy::validate_name(name)?;
            disks.insert(
                disk.slot_id.clone(),
                RecursiveBackupDisk {
                    identity: registration.identity(),
                    level: registration.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    name: name.into(),
                },
            );
        }
        Ok(RecursiveBackupCapture {
            format: FORMAT.into(),
            source_sequence: self.state().sequence,
            source_business_generation: self.state().business_generation,
            registry,
            disks,
        })
    }

    /// Publish actual file-container path updates through the normal full-copy
    /// transaction. The permanent native request includes the exact capture.
    ///
    /// # Errors
    /// Rejects stale captures, stopped/rebound routes, ambiguous source names,
    /// unsupported data-only directory targets and the ordinary policy/copy/namespace errors.
    pub fn configure_recursive_backups(
        &mut self,
        request: RecursiveBackupConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        self.configure_recursive_snapshot(request, RecursiveBackupMode::Files)
    }

    /// Explicitly publish one current directory snapshot, including its files
    /// and empty descendants, through the same full-copy transaction.
    /// Independent child policies retain their own snapshots and placements.
    ///
    /// This is separate from `configure_recursive_backups`: directory targets
    /// require metadata-member publication evidence, whereas ordinary file
    /// backups may use data-only targets. Old operation receipts are unchanged.
    ///
    /// # Errors
    /// Rejects file scopes, non-metadata directory targets, stale captures and
    /// ordinary backup-policy, copy, namespace and publication failures.
    pub fn configure_recursive_backup_tree(
        &mut self,
        request: RecursiveBackupConfigurationRequest<'_>,
    ) -> Result<OperationResult> {
        self.configure_recursive_snapshot(request, RecursiveBackupMode::DirectoryTree)
    }

    fn configure_recursive_snapshot(
        &mut self,
        request: RecursiveBackupConfigurationRequest<'_>,
        mode: RecursiveBackupMode,
    ) -> Result<OperationResult> {
        let input = request_json(request);
        self.mutate(
            mode.kind(),
            request.principal,
            request.operation_id,
            input,
            |engine, state, pending| {
                // mutate resolves permanent retries before this live boundary.
                // Cold replay validates the stored capture against its original
                // registry/predecessor instead of a current mount basename.
                if engine.recursive_backup_capture()? != *request.capture {
                    return Err(changed());
                }
                let current = engine
                    .admitted_current_namespaces()?
                    .ok_or_else(unavailable)?;
                let plan = current.plan_recursive_backup_configuration(
                    engine.state(),
                    request,
                    mode,
                    || Ok(new_id("policy")?),
                )?;
                apply_current_plan(engine, state, pending, plan)
            },
        )
    }
}

pub(crate) fn request_json(request: RecursiveBackupConfigurationRequest<'_>) -> json_codec::Value {
    json!({
        "repository_id": request.repository_id,
        "path": request.path,
        "expected_scope_id": request.expected_scope_id,
        "target_copies": request.target_copies,
        "expected_generation": request.expected_generation.to_string(),
        "capture": request.capture,
    })
}

impl RecursiveBackupCapture {
    /// Capture roots omit the configured scope, so later primary path changes
    /// can reuse the same immediate-upstream names without resampling mounts.
    pub(crate) fn follow_roots<'a>(
        &self,
        repository: &Repository,
        targets: impl Iterator<Item = &'a String>,
    ) -> Result<BTreeMap<String, String>> {
        let mut roots = BTreeMap::new();
        for target in targets {
            let mut names = Vec::new();
            let mut disk = self.disks.get(target).ok_or_else(changed)?;
            while disk.level > 1 {
                let source = disk.upstream_slot_id.as_ref().ok_or_else(changed)?;
                let parent = self.disks.get(source).ok_or_else(changed)?;
                if parent.level.checked_add(1) != Some(disk.level) {
                    return Err(changed());
                }
                names.push(parent.name.as_str());
                disk = parent;
            }
            if names.is_empty() {
                return Err(changed());
            }
            if !repository.file_manager_root {
                names.push(&repository.name);
            }
            let root = PortablePath::parse(&names.join("/"))?;
            roots.insert(target.clone(), root.as_str().to_owned());
        }
        Ok(roots)
    }

    /// The original capture remains request evidence, never a present write
    /// grant. Only the exact still-enabled identities and direct routes can move.
    pub(crate) fn verify_follow_routes<'a>(
        &self,
        current: &CurrentNamespaces,
        targets: impl Iterator<Item = &'a String>,
    ) -> Result<()> {
        let mut checked = BTreeSet::new();
        for target in targets {
            let mut slot = target;
            loop {
                if !checked.insert(slot) {
                    break;
                }
                let captured = self.disks.get(slot).ok_or_else(changed)?;
                let live = current.backup_configuration_disk(slot)?;
                if live.identity != captured.identity
                    || live.level != captured.level
                    || live.upstream_slot_id != captured.upstream_slot_id
                {
                    return Err(changed());
                }
                match &captured.upstream_slot_id {
                    Some(parent) => slot = parent,
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// A file's retained placements, not all roots of its enclosing policy,
    /// supply new routed-follow I/O obligations. Bindings remain exact.
    pub(crate) fn verify_follow_bindings(
        &self,
        current: &CurrentNamespaces,
        bindings: &BTreeMap<String, u64>,
    ) -> Result<()> {
        self.verify_follow_routes(current, bindings.keys())?;
        for (slot, generation) in bindings {
            if self
                .disks
                .get(slot)
                .ok_or_else(changed)?
                .identity
                .binding_generation
                != *generation
            {
                return Err(changed());
            }
        }
        Ok(())
    }

    pub(crate) fn validate_predecessor(
        &self,
        state: &State,
        registry: &DiskRegistryCommitment,
        definitions: impl Iterator<Item = (DiskRegistrationIdentity, u32, Option<String>)>,
    ) -> Result<()> {
        if self.format != FORMAT
            || self.source_sequence != state.sequence
            || self.source_business_generation != state.business_generation
            || &self.registry != registry
        {
            return Err(changed());
        }
        let mut count = 0;
        for (identity, level, upstream) in definitions {
            let captured = self.disks.get(&identity.slot_id).ok_or_else(changed)?;
            if captured.identity != identity
                || captured.level != level
                || captured.upstream_slot_id != upstream
            {
                return Err(changed());
            }
            name_policy::validate_name(&captured.name)?;
            count += 1;
        }
        if count != self.disks.len() {
            return Err(changed());
        }
        Ok(())
    }

    pub(super) fn extend_prefixes(
        &self,
        repository: &Repository,
        scope: &Scope,
        route: &[String],
        prefixes: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        // The native route is nonempty, starts at level one and follows each
        // exact direct upstream. Only names before the target enter its path.
        let mut source_prefix: Option<PortablePath> = None;
        for pair in route.windows(2) {
            let source = &self.disks[&pair[0]];
            let name = PortablePath::parse(&source.name)?;
            source_prefix = Some(match source_prefix {
                Some(previous) => previous.rebase(0, &name)?,
                None => name,
            });
            let mut prefix = source_prefix
                .as_ref()
                .expect("invariant: each target has a preceding source")
                .clone();
            if !repository.file_manager_root {
                prefix = PortablePath::parse(&repository.name)?.rebase(0, &prefix)?;
            }
            let scope_parent = if scope.kind == NamespaceEntryKind::File {
                scope.path.rsplit_once('/').map_or("", |(parent, _)| parent)
            } else {
                &scope.path
            };
            if !scope_parent.is_empty() {
                prefix = PortablePath::parse(scope_parent)?.rebase(0, &prefix)?;
            }
            let path = prefix.as_str().to_owned();
            if let Some(existing) = prefixes.get(&pair[1]) {
                if existing != &path {
                    return Err(Error::new(
                        ErrorCode::BackupPathConflict,
                        "one target cannot contain different recursive source paths in one file binding",
                    ));
                }
            } else {
                prefixes.insert(pair[1].clone(), path);
            }
        }
        Ok(())
    }
}

impl CurrentNamespaces {
    pub(crate) fn plan_recursive_backup_configuration(
        &self,
        state: &State,
        request: RecursiveBackupConfigurationRequest<'_>,
        mode: RecursiveBackupMode,
        fresh_policy: impl FnMut() -> Result<String>,
    ) -> Result<CurrentBackupConfigurationPlan> {
        self.verify_recursive_backup_capture(state, request.capture)?;
        let prefixes = BTreeMap::new();
        let mut plan = self.plan_backup_configuration_with_paths(
            state,
            ScopedBackupConfigurationRequest {
                repository_id: request.repository_id,
                path: request.path,
                expected_scope_id: Some(request.expected_scope_id),
                target_copies: request.target_copies,
                backup_prefixes: &prefixes,
                expected_generation: request.expected_generation,
                principal: request.principal,
                operation_id: request.operation_id,
            },
            Some(RecursivePaths {
                capture: request.capture,
                mode,
            }),
            fresh_policy,
        )?;
        plan.result["backup_request"] = request_json(request);
        Ok(plan)
    }
}
